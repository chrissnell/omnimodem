//! SDR audio source, split across a transport seam so the DSP capture pipeline is
//! shared by every dongle transport (`rtl_tcp` today, native USB to come).
//!
//! - [`mod.rs`](self) — the transport-agnostic surface: the [`SdrControl`] runtime
//!   cell, [`TunerCaps`] + the per-tuner static tables, the [`DemodMode`] selector,
//!   frequency-plan split ([`plan_tune`]), and the internal [`SdrTransport`] seam.
//! - [`rtl_tcp`] — the `rtl_tcp` wire protocol: the 12-byte greeting, the `RtlCmd`
//!   command encoding, and [`RtlTcpTransport`] / [`RtlTcpBackend`].
//! - [`pipeline`] — the reusable DSP capture body ([`run_capture`](pipeline::run_capture)):
//!   channel-select NCO, demod, decimation, squelch, waterfall, and overrun-safe
//!   delivery — driven by any [`SdrTransport`].
//!
//! Everything above the transport is transport-independent: it operates on raw u8
//! IQ and the shared control snapshot, so a second transport reuses the whole
//! channelizer / demod / waterfall / control surface unchanged.

use super::AudioError;
use omnimodem_dsp::frontend::sdr_demod::DemodKind;
use omnimodem_dsp::frontend::squelch::PowerSquelch;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering,
};
use std::sync::{Arc, Mutex};

pub mod pipeline;
pub mod rtl_tcp;
pub mod usb;
pub(crate) mod usb_regs;

// The public rtl_tcp surface stays reachable at `audio::sdr::*`, so callers (and
// the existing integration tests via the `audio::rtlsdr` alias) need no path
// churn.
pub use rtl_tcp::{caps_from_header, parse_header, RtlCmd, RtlHeader, RtlTcpBackend};

/// Default dongle capture (sample) rate. 240 kHz captures a comfortable slice of
/// spectrum and decimates 5:1 to the 48 kHz audio channel rate.
pub const DEFAULT_CAPTURE_RATE: u32 = 240_000;
/// Default NBFM peak deviation used to scale the discriminator output. APRS/NBFM
/// sits around ±3–5 kHz; the exact value only affects audio gain, which the
/// downstream slicer is robust to.
pub const DEFAULT_DEVIATION_HZ: f32 = 5_000.0;
/// Squelch hysteresis (open threshold minus this closes).
const SQUELCH_HYSTERESIS_DB: f32 = 6.0;

// ---------------------------------------------------------------------------
// Tuner capabilities
// ---------------------------------------------------------------------------

/// Human-readable tuner name from the `rtl_tcp` tuner-type code (`rtlsdr.h`
/// `rtlsdr_tuner`). Surfaced later via `GetSdrCaps`.
pub fn tuner_name(t: u32) -> &'static str {
    match t {
        1 => "E4000",
        2 => "FC0012",
        3 => "FC0013",
        4 => "FC2580",
        5 => "R820T",
        6 => "R828D",
        _ => "unknown",
    }
}

/// What a bound SDR tuner can do, derived from the tuner type plus per-tuner
/// static tables — `rtl_tcp` reports the tuner and a gain-table *count*, but not
/// the frequency range or the gain values, so those come from librtlsdr's known
/// tables (a native USB probe reuses the same tables). Published into
/// [`SdrControl`] by the capture thread and read by `GetSdrCaps`.
#[derive(Debug, Clone, PartialEq)]
pub struct TunerCaps {
    pub tuner: String,
    pub freq_min_hz: f64,
    pub freq_max_hz: f64,
    pub sample_rates: Vec<u32>,
    pub gains_db: Vec<f32>,
    pub bias_tee_supported: bool,
    pub direct_sampling_supported: bool,
}

/// Tunable RF range (Hz) for a tuner-type code. Conservative published ranges
/// from librtlsdr; the default covers the common R820-class span.
pub fn tuner_freq_range(t: u32) -> (f64, f64) {
    match t {
        1 => (52_000_000.0, 2_200_000_000.0),   // E4000 (has an internal gap)
        2 | 3 => (22_000_000.0, 1_100_000_000.0), // FC0012/FC0013
        4 => (146_000_000.0, 924_000_000.0),     // FC2580
        5 | 6 => (24_000_000.0, 1_766_000_000.0), // R820T / R828D
        _ => (24_000_000.0, 1_766_000_000.0),
    }
}

/// Discrete tuner gain table (dB) for a tuner-type code — the values librtlsdr
/// exposes for manual gain. `SetSdrGain` snaps a requested gain to the nearest
/// entry.
pub fn tuner_gains_db(t: u32) -> Vec<f32> {
    match t {
        // R820T / R828D: the canonical 29-step table (librtlsdr `r82xx`).
        5 | 6 => vec![
            0.0, 0.9, 1.4, 2.7, 3.7, 7.7, 8.7, 12.5, 14.4, 15.7, 16.6, 19.7, 20.7,
            22.9, 25.4, 28.0, 29.7, 32.8, 33.8, 36.4, 37.2, 38.6, 40.2, 42.1, 43.4,
            43.9, 44.5, 48.0, 49.6,
        ],
        // E4000: librtlsdr's 14-step table.
        1 => vec![
            -1.0, 1.5, 4.0, 6.5, 9.0, 11.5, 14.0, 16.5, 19.0, 21.5, 24.0, 29.0,
            34.0, 42.0,
        ],
        // FC0012/FC0013/FC2580 and unknown: a coarse fallback ramp.
        _ => vec![0.0, 5.0, 10.0, 15.0, 20.0, 25.0],
    }
}

/// The capture (sample) rates `rtl_tcp`/librtlsdr accepts, plus omnimodem's
/// 240 kHz default. `ConfigureSdr` validates a requested rate against this set.
pub fn supported_sample_rates() -> Vec<u32> {
    vec![
        DEFAULT_CAPTURE_RATE,
        250_000,
        1_024_000,
        1_536_000,
        1_792_000,
        1_920_000,
        2_048_000,
        2_160_000,
        2_400_000,
        2_560_000,
        2_880_000,
        3_200_000,
    ]
}

/// Whether the tuner exposes a switchable bias-tee. The bias-tee is a board
/// feature gated on the R820-class tuners librtlsdr drives it for (R820T/R820T2
/// report type 5; R828D/R860 report type 6).
pub fn bias_tee_supported(tuner_type: u32) -> bool {
    matches!(tuner_type, 5 | 6)
}

/// Direct sampling bypasses the tuner and samples the RTL2832U ADC directly to
/// reach HF, so every dongle supports it regardless of tuner chip.
pub fn direct_sampling_supported(_tuner_type: u32) -> bool {
    true
}

/// Highest frequency (Hz) reachable in direct-sampling mode: the RTL2832U's
/// ~28.8 MHz ADC clock sets the Nyquist ceiling.
pub const DIRECT_SAMPLING_MAX_HZ: f64 = 28_800_000.0;

/// Direct-sampling Q-branch mode. RTL-SDR Blog V3 (and most consumer HF-capable
/// dongles) wire the HF input to the Q ADC, so the `ConfigureSdr` `bool` maps to
/// this branch when enabled.
pub const DIRECT_SAMPLING_Q_BRANCH: u32 = 2;

/// Nearest entry in a discrete gain table to `want` (dB). Returns `want`
/// unchanged when the table is empty (unknown tuner). Used by `SetSdrGain` to
/// report the gain the dongle will actually snap to. A non-finite `want` is not
/// expected here — the gRPC handler rejects it — so the `Equal` fallback (which
/// would return the first entry) is a harmless belt-and-suspenders default.
pub fn snap_gain(table: &[f32], want: f32) -> f32 {
    table
        .iter()
        .copied()
        .min_by(|a, b| {
            (a - want).abs().partial_cmp(&(b - want).abs()).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(want)
}

// ---------------------------------------------------------------------------
// Tune planning (the "gqrx" center + NCO split)
// ---------------------------------------------------------------------------

/// Fraction of the captured band, measured from center, the NCO offset may reach
/// before a hardware re-center is forced. Keeping to the central 80% (±0.4·rate)
/// stays clear of the anti-alias roll-off at the band edges.
const TUNE_MAX_OFFSET_FRAC: f64 = 0.4;
/// Where a re-center places the signal: a quarter-band above the new hardware
/// center, so it avoids the dongle's DC spike (I/Q imbalance) at exact center.
const TUNE_RECENTER_OFFSET_FRAC: f64 = 0.25;

/// Split an absolute demod frequency into (hardware center, NCO offset) for the
/// gqrx wideband model. When the target already sits within the usable span of
/// the current center, only the NCO moves (instant, lossless); otherwise the
/// hardware re-centers so the signal lands a quarter-band up, clear of the DC
/// spike. `center + offset == target` always holds.
pub fn plan_tune(center_hz: f64, capture_rate: u32, target_hz: f64) -> (f64, f32) {
    let rate = capture_rate as f64;
    let max_offset = rate * TUNE_MAX_OFFSET_FRAC;
    if center_hz != 0.0 && (target_hz - center_hz).abs() <= max_offset {
        // In band: keep the hardware where it is, retune only the NCO.
        (center_hz, (target_hz - center_hz) as f32)
    } else {
        // Out of band (or first tune from cold): re-center the hardware.
        let offset = rate * TUNE_RECENTER_OFFSET_FRAC;
        (target_hz - offset, offset as f32)
    }
}

// ---------------------------------------------------------------------------
// Runtime control cell
// ---------------------------------------------------------------------------

/// Full-rate ADS-B capture rate (2.4 Msps): the dongle rate the [`DemodMode::RawMag`]
/// path streams. The R6 native demod (`Demod2400`) runs at this rate directly, so the
/// RX worker feeds it the magnitude with no resample. 2.4M is a rate every R820-class
/// dongle accepts (see [`supported_sample_rates`]).
pub const ADSB_CAPTURE_RATE: u32 = 2_400_000;

/// ADS-B downlink frequency (1090.0 MHz). Unlike the audio SDR modes, ADS-B is
/// fixed-frequency — the operator never tunes it — so the daemon must tune the
/// dongle here itself when it binds an ADS-B channel. The magnitude envelope
/// `|I+jQ|` the [`DemodMode::RawMag`] path decodes is invariant to a residual
/// carrier offset, so [`plan_tune`] can (and does) place 1090 MHz a quarter-band
/// off hardware center to clear the R820T DC spike without hurting the decode.
pub const ADSB_FREQ_HZ: f64 = 1_090_000_000.0;

/// Selectable demodulator. The audio modes (NBFM/AM/WFM/SSB) are dispatched by the
/// capture thread via `SdrDemod`, which tunes + channelizes to narrowband audio.
/// `RawMag` is the odd one out: it bypasses `SdrDemod` entirely and emits the
/// full-rate magnitude envelope `|I+jQ|` for the wideband `adsb` mode — the daemon
/// selects it internally when a channel's mode is ADS-B (never via `ConfigureSdr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DemodMode {
    Nbfm = 0,
    Am = 1,
    Wfm = 2,
    Usb = 3,
    Lsb = 4,
    RawMag = 5,
}

impl DemodMode {
    pub fn from_u8(v: u8) -> DemodMode {
        match v {
            1 => DemodMode::Am,
            2 => DemodMode::Wfm,
            3 => DemodMode::Usb,
            4 => DemodMode::Lsb,
            5 => DemodMode::RawMag,
            _ => DemodMode::Nbfm,
        }
    }

    /// Map to the DSP crate's mode enum, which drives `SdrDemod`'s back-end.
    /// `RawMag` has no `SdrDemod` equivalent — the capture thread bypasses the
    /// channelizing demod for it — so this maps it to a harmless default that is
    /// never actually built (guarded by the `raw_mag` branch in the capture loop).
    pub fn to_dsp(self) -> DemodKind {
        match self {
            DemodMode::Nbfm | DemodMode::RawMag => DemodKind::Nbfm,
            DemodMode::Am => DemodKind::Am,
            DemodMode::Wfm => DemodKind::Wfm,
            DemodMode::Usb => DemodKind::Usb,
            DemodMode::Lsb => DemodKind::Lsb,
        }
    }
}

/// Squelch threshold sentinel: at or below this the squelch is disabled (always
/// open). Matches the gRPC `squelch_db <= -200 disables` convention.
pub const SQUELCH_DISABLED_DB: f32 = -200.0;

struct SdrControlInner {
    /// Bumped on every setter so the capture thread detects changes with one load.
    generation: AtomicU64,
    /// Hardware center frequency (Hz), stored as f64 bits.
    center_hz: AtomicU64,
    /// NCO channel-select offset from center (Hz), f32 bits.
    offset_hz: AtomicU32,
    /// True = automatic hardware AGC; false = manual `gain_db`.
    gain_auto: AtomicBool,
    /// Manual tuner gain (dB), f32 bits. Ignored while `gain_auto`.
    gain_db: AtomicU32,
    /// Power-squelch open threshold (dBFS), f32 bits. `<= SQUELCH_DISABLED_DB`
    /// disables the squelch.
    squelch_db: AtomicU32,
    /// Frequency correction (ppm).
    ppm: AtomicI32,
    /// Bias-tee enable: powers an inline LNA/antenna over the coax.
    bias_tee: AtomicBool,
    /// Direct-sampling mode: 0 = off, 1 = I-branch, 2 = Q-branch. Non-zero
    /// bypasses the tuner to reach HF.
    direct_sampling: AtomicU32,
    /// Demod mode (`DemodMode as u8`).
    demod_mode: AtomicU8,
    /// Dongle capture (sample) rate (Hz). Set by `ConfigureSdr`; the capture
    /// thread re-applies it live. Also read by the tune split to size the band.
    capture_rate: AtomicU32,
    /// Tuner capabilities, published by the capture thread once the header is
    /// parsed. `None` until a capture has connected.
    caps: Mutex<Option<TunerCaps>>,
    /// Cumulative count of audio chunks the capture thread dropped because the
    /// consumer (modem) fell behind. Observability only — not part of
    /// `generation` (a drop is not a control change). See the drop-oldest
    /// overrun policy in the capture thread.
    dropped_chunks: AtomicU64,
}

/// A clonable handle to one channel's SDR runtime control — the RX analogue of
/// [`crate::core::AudioGain`]. The core (writer, from gRPC in Plan 3) and the
/// backend capture thread (reader) share the same `Arc` of atomics, so tuning,
/// gain, squelch, and demod-mode changes reach a *running* capture with no
/// respawn.
#[derive(Clone)]
pub struct SdrControl {
    inner: Arc<SdrControlInner>,
}

impl Default for SdrControl {
    fn default() -> Self {
        SdrControl {
            inner: Arc::new(SdrControlInner {
                generation: AtomicU64::new(0),
                center_hz: AtomicU64::new(0.0f64.to_bits()),
                offset_hz: AtomicU32::new(0.0f32.to_bits()),
                gain_auto: AtomicBool::new(true),
                gain_db: AtomicU32::new(0.0f32.to_bits()),
                squelch_db: AtomicU32::new(SQUELCH_DISABLED_DB.to_bits()),
                ppm: AtomicI32::new(0),
                bias_tee: AtomicBool::new(false),
                direct_sampling: AtomicU32::new(0),
                demod_mode: AtomicU8::new(DemodMode::Nbfm as u8),
                capture_rate: AtomicU32::new(DEFAULT_CAPTURE_RATE),
                caps: Mutex::new(None),
                dropped_chunks: AtomicU64::new(0),
            }),
        }
    }
}

impl SdrControl {
    fn bump(&self) {
        self.inner.generation.fetch_add(1, Ordering::Release);
    }

    /// Current generation counter (one relaxed load; cheap to poll per block).
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }

    pub fn center_hz(&self) -> f64 {
        f64::from_bits(self.inner.center_hz.load(Ordering::Relaxed))
    }
    pub fn set_center_hz(&self, hz: f64) {
        self.inner.center_hz.store(hz.to_bits(), Ordering::Relaxed);
        self.bump();
    }

    pub fn offset_hz(&self) -> f32 {
        f32::from_bits(self.inner.offset_hz.load(Ordering::Relaxed))
    }
    pub fn set_offset_hz(&self, hz: f32) {
        self.inner.offset_hz.store(hz.to_bits(), Ordering::Relaxed);
        self.bump();
    }

    pub fn gain_auto(&self) -> bool {
        self.inner.gain_auto.load(Ordering::Relaxed)
    }
    pub fn gain_db(&self) -> f32 {
        f32::from_bits(self.inner.gain_db.load(Ordering::Relaxed))
    }
    /// Set gain: `auto` engages hardware AGC; otherwise `gain_db` is the manual
    /// tuner gain.
    pub fn set_gain(&self, auto: bool, gain_db: f32) {
        self.inner.gain_auto.store(auto, Ordering::Relaxed);
        self.inner.gain_db.store(gain_db.to_bits(), Ordering::Relaxed);
        self.bump();
    }

    pub fn squelch_db(&self) -> f32 {
        f32::from_bits(self.inner.squelch_db.load(Ordering::Relaxed))
    }
    pub fn set_squelch_db(&self, db: f32) {
        self.inner.squelch_db.store(db.to_bits(), Ordering::Relaxed);
        self.bump();
    }

    pub fn ppm(&self) -> i32 {
        self.inner.ppm.load(Ordering::Relaxed)
    }
    pub fn set_ppm(&self, ppm: i32) {
        self.inner.ppm.store(ppm, Ordering::Relaxed);
        self.bump();
    }

    pub fn bias_tee(&self) -> bool {
        self.inner.bias_tee.load(Ordering::Relaxed)
    }
    pub fn set_bias_tee(&self, on: bool) {
        self.inner.bias_tee.store(on, Ordering::Relaxed);
        self.bump();
    }

    /// Direct-sampling mode (0 = off, 1 = I-branch, 2 = Q-branch).
    pub fn direct_sampling(&self) -> u32 {
        self.inner.direct_sampling.load(Ordering::Relaxed)
    }
    pub fn set_direct_sampling(&self, mode: u32) {
        self.inner.direct_sampling.store(mode, Ordering::Relaxed);
        self.bump();
    }

    pub fn demod_mode(&self) -> DemodMode {
        DemodMode::from_u8(self.inner.demod_mode.load(Ordering::Relaxed))
    }
    pub fn set_demod_mode(&self, mode: DemodMode) {
        self.inner.demod_mode.store(mode as u8, Ordering::Relaxed);
        self.bump();
    }

    pub fn capture_rate(&self) -> u32 {
        self.inner.capture_rate.load(Ordering::Relaxed)
    }
    pub fn set_capture_rate(&self, rate: u32) {
        self.inner.capture_rate.store(rate, Ordering::Relaxed);
        self.bump();
    }

    /// The tuner capabilities the capture thread published at connect (`None`
    /// until a capture has connected and parsed the header).
    pub fn caps(&self) -> Option<TunerCaps> {
        self.inner.caps.lock().unwrap().clone()
    }
    pub fn set_caps(&self, caps: TunerCaps) {
        *self.inner.caps.lock().unwrap() = Some(caps);
    }

    /// Cumulative audio chunks dropped under consumer overrun (drop-oldest).
    pub fn dropped_chunks(&self) -> u64 {
        self.inner.dropped_chunks.load(Ordering::Relaxed)
    }
    /// Record one dropped chunk; returns the new cumulative total.
    pub fn incr_dropped(&self) -> u64 {
        self.inner.dropped_chunks.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Build a `PowerSquelch` from the current threshold (disabled at/under the
    /// sentinel).
    pub fn effective_squelch(&self) -> PowerSquelch {
        let db = self.squelch_db();
        if !db.is_finite() || db <= SQUELCH_DISABLED_DB {
            PowerSquelch::disabled()
        } else {
            PowerSquelch::new(db, SQUELCH_HYSTERESIS_DB)
        }
    }
}

// ---------------------------------------------------------------------------
// Transport seam
// ---------------------------------------------------------------------------

/// What the DSP capture pipeline needs from "the dongle", regardless of whether it
/// is reached over TCP (`rtl_tcp`) or, in a later phase, directly over USB. Kept
/// internal to the audio module — never part of the gRPC surface.
///
/// The transport owns its own liveness: [`read_iq`](SdrTransport::read_iq) blocks
/// for the next raw u8 IQ and, for a transport that can transparently recover a
/// dropped link (`rtl_tcp`), reconnects and re-applies hardware internally so the
/// pipeline sees one continuous stream. A transport with no recovery (a local USB
/// drop) instead returns a terminal result.
pub(crate) trait SdrTransport: Send {
    /// Fill `buf` with raw interleaved u8 IQ; blocks until data or a terminal
    /// stop. Returns the byte count, or `Ok(0)` once shutdown has been signalled
    /// (via the [`shutdown_handle`](SdrTransport::shutdown_handle) hook).
    fn read_iq(&mut self, buf: &mut [u8]) -> Result<usize, AudioError>;

    /// Apply the current hardware parameters from the control snapshot (center
    /// freq, sample rate, gain mode/level, ppm, bias-tee, direct-sampling).
    /// Called by the pipeline whenever the control generation changes.
    fn apply_hardware(&mut self, control: &SdrControl) -> Result<(), AudioError>;

    /// Capabilities discovered at open (tuner type, ranges, gain table).
    fn caps(&self) -> TunerCaps;

    /// A closure that unblocks a thread parked in [`read_iq`](SdrTransport::read_iq)
    /// so a `stop` is honored promptly. Obtained before the transport is moved
    /// into the capture thread; stored on the `CaptureHandle`.
    fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send>;
}

/// Lets a runtime-selected transport (chosen only at `open_capture`, so its
/// concrete type is erased to a trait object) drive the generic
/// [`run_capture`](pipeline::run_capture) unchanged. The USB backend picks its
/// transport — the real dongle in production, a fake in tests — behind a
/// `Box<dyn SdrTransport>`, which this forwards verbatim.
impl SdrTransport for Box<dyn SdrTransport> {
    fn read_iq(&mut self, buf: &mut [u8]) -> Result<usize, AudioError> {
        (**self).read_iq(buf)
    }
    fn apply_hardware(&mut self, control: &SdrControl) -> Result<(), AudioError> {
        (**self).apply_hardware(control)
    }
    fn caps(&self) -> TunerCaps {
        (**self).caps()
    }
    fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send> {
        (**self).shutdown_handle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_bias_tee_and_direct_sampling_visible_and_bump() {
        let c = SdrControl::default();
        let worker = c.clone();
        // Defaults are off.
        assert!(!worker.bias_tee());
        assert_eq!(worker.direct_sampling(), 0);

        let g0 = worker.generation();
        c.set_bias_tee(true);
        assert!(worker.bias_tee());
        assert_ne!(worker.generation(), g0);

        let g1 = worker.generation();
        c.set_direct_sampling(2);
        assert_eq!(worker.direct_sampling(), 2);
        assert_ne!(worker.generation(), g1);
    }

    #[test]
    fn control_defaults() {
        let c = SdrControl::default();
        assert_eq!(c.center_hz(), 0.0);
        assert_eq!(c.offset_hz(), 0.0);
        assert!(c.gain_auto());
        assert_eq!(c.demod_mode(), DemodMode::Nbfm);
        // Default squelch is disabled → always open.
        assert!(c.effective_squelch().is_open());
    }

    #[test]
    fn control_set_visible_through_clone_and_bumps_generation() {
        let c = SdrControl::default();
        let worker = c.clone();
        let g0 = worker.generation();
        c.set_offset_hz(30_000.0);
        assert_eq!(worker.offset_hz(), 30_000.0);
        assert_ne!(worker.generation(), g0);

        let g1 = worker.generation();
        c.set_gain(false, 24.0);
        assert!(!worker.gain_auto());
        assert_eq!(worker.gain_db(), 24.0);
        assert_ne!(worker.generation(), g1);
    }

    #[test]
    fn squelch_threshold_builds_gated_squelch() {
        let c = SdrControl::default();
        c.set_squelch_db(-20.0);
        // A finite threshold yields a real (initially closed) squelch.
        assert!(!c.effective_squelch().is_open());
        c.set_squelch_db(SQUELCH_DISABLED_DB);
        assert!(c.effective_squelch().is_open());
    }

    #[test]
    fn capture_rate_defaults_and_set_bumps_generation() {
        let c = SdrControl::default();
        let worker = c.clone();
        assert_eq!(c.capture_rate(), DEFAULT_CAPTURE_RATE);
        let g0 = worker.generation();
        c.set_capture_rate(1_024_000);
        assert_eq!(worker.capture_rate(), 1_024_000);
        assert_ne!(worker.generation(), g0);
    }

    #[test]
    fn dropped_chunks_counter_starts_zero_and_increments_through_clone() {
        let c = SdrControl::default();
        let worker = c.clone();
        assert_eq!(c.dropped_chunks(), 0);
        // A drop is observability, not a control change: generation must not move.
        let g0 = worker.generation();
        assert_eq!(worker.incr_dropped(), 1);
        assert_eq!(worker.incr_dropped(), 2);
        assert_eq!(c.dropped_chunks(), 2);
        assert_eq!(worker.generation(), g0);
    }

    #[test]
    fn caps_default_none_then_visible_through_clone() {
        let c = SdrControl::default();
        let worker = c.clone();
        assert!(c.caps().is_none());
        c.set_caps(caps_from_header(&RtlHeader { tuner_type: 5, tuner_gain_count: 29 }));
        let caps = worker.caps().expect("caps published");
        assert_eq!(caps.tuner, "R820T");
        assert_eq!(caps.gains_db.len(), 29);
        // R820T is a bias-tee-capable, direct-sampling-capable tuner.
        assert!(caps.bias_tee_supported);
        assert!(caps.direct_sampling_supported);
        assert!(caps.sample_rates.contains(&DEFAULT_CAPTURE_RATE));
        assert!(caps.freq_min_hz < caps.freq_max_hz);
    }

    #[test]
    fn caps_bias_tee_gated_on_tuner_direct_sampling_universal() {
        // R820T (5) / R828D (6): bias-tee supported. E4000 (1): not.
        assert!(bias_tee_supported(5));
        assert!(bias_tee_supported(6));
        assert!(!bias_tee_supported(1));
        assert!(!bias_tee_supported(0));
        // Direct sampling is an ADC feature — every dongle has it.
        assert!(direct_sampling_supported(1));
        assert!(direct_sampling_supported(5));
        assert!(direct_sampling_supported(0));
    }

    #[test]
    fn snap_gain_picks_nearest_table_entry() {
        let table = tuner_gains_db(5); // R820T
        assert_eq!(snap_gain(&table, 0.0), 0.0);
        assert_eq!(snap_gain(&table, 100.0), 49.6); // clamps to the top entry
        assert_eq!(snap_gain(&table, 20.0), 19.7); // nearest of 19.7/20.7
        // Empty table (unknown tuner) returns the request unchanged.
        assert_eq!(snap_gain(&[], 13.3), 13.3);
    }

    #[test]
    fn plan_tune_first_tune_recenters_and_hits_target() {
        // Cold (center 0): re-center places the signal a quarter-band up.
        let (center, offset) = plan_tune(0.0, 240_000, 144_390_000.0);
        assert_eq!(offset, 60_000.0); // 0.25 * 240k
        assert!(((center + offset as f64) - 144_390_000.0).abs() < 1.0);
    }

    #[test]
    fn plan_tune_in_band_moves_only_the_nco() {
        // Already centered at 144.42M; ask for 144.39M (30 kHz away, in band).
        let center0 = 144_420_000.0;
        let (center, offset) = plan_tune(center0, 240_000, 144_390_000.0);
        assert_eq!(center, center0); // hardware unchanged
        assert_eq!(offset, -30_000.0);
    }

    #[test]
    fn plan_tune_overshoot_recenters() {
        // 200 kHz away exceeds the ±0.4·rate (±96 kHz) usable span → re-center.
        let center0 = 144_390_000.0;
        let (center, offset) = plan_tune(center0, 240_000, 144_590_000.0);
        assert_ne!(center, center0);
        assert_eq!(offset, 60_000.0);
        assert!(((center + offset as f64) - 144_590_000.0).abs() < 1.0);
    }
}
