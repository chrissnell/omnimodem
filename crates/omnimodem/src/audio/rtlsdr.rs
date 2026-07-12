//! `rtl_tcp` SDR backend: connect to an `rtl_tcp` server (local or remote), tune
//! the dongle, read raw u8 IQ, demodulate to mono audio via the DSP crate's
//! `SdrDemod` (dispatching on the selected demod mode), and stream a wideband RF
//! waterfall — all behind the existing `AudioBackend` seam so every downstream
//! mode works unmodified.
//!
//! Wire protocol (`librtlsdr` `rtl_tcp.c`): on connect the server writes a
//! 12-byte header (magic `RTL0` + tuner type + gain count), then streams
//! interleaved unsigned-8-bit IQ. The client sends 5-byte commands back on the
//! same socket to tune/gain/correct the dongle.

use super::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use super::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH, MAX_SAMPLE_RATE};
use crate::core::event::TelemetryEvent;
use crate::ids::{ChannelId, DeviceId};
use omnimodem_dsp::frontend::complex_stft::ComplexStft;
use omnimodem_dsp::frontend::iq::u8_iq_to_cplx;
use omnimodem_dsp::frontend::sdr_demod::{DemodKind, SdrDemod};
use omnimodem_dsp::frontend::spectrum::{full_spectrum_dbfs, SpectrumPlan};
use omnimodem_dsp::frontend::squelch::PowerSquelch;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering,
};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Default dongle capture (sample) rate. 240 kHz captures a comfortable slice of
/// spectrum and decimates 5:1 to the 48 kHz audio channel rate.
pub const DEFAULT_CAPTURE_RATE: u32 = 240_000;
/// Default NBFM peak deviation used to scale the discriminator output. APRS/NBFM
/// sits around ±3–5 kHz; the exact value only affects audio gain, which the
/// downstream slicer is robust to.
pub const DEFAULT_DEVIATION_HZ: f32 = 5_000.0;
/// Squelch hysteresis (open threshold minus this closes).
const SQUELCH_HYSTERESIS_DB: f32 = 6.0;
/// FFT size for the wideband RF waterfall.
const WATERFALL_NFFT: usize = 1024;
/// Requested waterfall bin count (rendered uint8 line width).
const WATERFALL_BINS: usize = 256;
/// Scale applied to `RawMag` magnitude so the u8-IQ maximum (|±1 ±1j| = √2) fits in
/// [0,1] ahead of the i16 delivery clamp. The ADS-B PPM demod is scale-independent,
/// so this only prevents strong-pulse saturation.
const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Exponential reconnect backoff after a dropped `rtl_tcp` link. Mirrors
/// `cpal_backend::REBUILD_BACKOFF` (that module is `#[cfg(not(test))]`, so the
/// schedule is re-declared here rather than shared).
const RECONNECT_BACKOFF: &[Duration] = &[
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];
/// Clear the backoff after a connection that streamed stably this long.
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(60);
/// Bound the connect + header read so a half-open / stalled server cannot park the
/// capture thread indefinitely — the stop hook can only shut down the *live*
/// streaming socket, not one still mid-handshake, so these steps must self-limit
/// for `stop` to be honored promptly after a reconnect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Log at most one overrun warning per this many dropped chunks, so a persistent
/// lag does not flood the log while still surfacing the running total.
const OVERRUN_LOG_EVERY: u64 = 64;

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

/// The 12-byte `rtl_tcp` greeting: magic + tuner type + gain-table size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtlHeader {
    pub tuner_type: u32,
    pub tuner_gain_count: u32,
}

/// Parse and validate the 12-byte `rtl_tcp` header. Errors if the magic is wrong
/// (we connected to something that is not an `rtl_tcp` server).
pub fn parse_header(buf: &[u8; 12]) -> Result<RtlHeader, AudioError> {
    if &buf[0..4] != b"RTL0" {
        return Err(AudioError::Io(format!(
            "rtl_tcp: bad header magic {:02x?}, not an rtl_tcp server",
            &buf[0..4]
        )));
    }
    let tuner_type = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let tuner_gain_count = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
    Ok(RtlHeader { tuner_type, tuner_gain_count })
}

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

// ---------------------------------------------------------------------------
// Tuner capabilities
// ---------------------------------------------------------------------------

/// What a bound `rtl_tcp` tuner can do, derived from the connect-time header
/// (tuner type) plus per-tuner static tables — `rtl_tcp` reports the tuner and a
/// gain-table *count*, but not the frequency range or the gain values, so those
/// come from librtlsdr's known tables. Published into [`SdrControl`] by the
/// capture thread and read by `GetSdrCaps`.
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

/// Compose the published capabilities from a parsed header. Bias-tee support is
/// tuner-dependent; direct sampling is universal (an ADC feature).
pub fn caps_from_header(h: &RtlHeader) -> TunerCaps {
    let (freq_min_hz, freq_max_hz) = tuner_freq_range(h.tuner_type);
    TunerCaps {
        tuner: tuner_name(h.tuner_type).to_string(),
        freq_min_hz,
        freq_max_hz,
        sample_rates: supported_sample_rates(),
        gains_db: tuner_gains_db(h.tuner_type),
        bias_tee_supported: bias_tee_supported(h.tuner_type),
        direct_sampling_supported: direct_sampling_supported(h.tuner_type),
    }
}

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
// Commands
// ---------------------------------------------------------------------------

/// An `rtl_tcp` control command: a 1-byte opcode plus a 4-byte big-endian arg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtlCmd {
    /// 0x01 — set center frequency (Hz).
    CenterFreq(u32),
    /// 0x02 — set sample rate (Hz).
    SampleRate(u32),
    /// 0x03 — set gain mode: true = manual, false = automatic (hardware AGC).
    GainMode(bool),
    /// 0x04 — set tuner gain (tenths of a dB).
    TunerGain(i32),
    /// 0x05 — set frequency correction (ppm).
    FreqCorrection(i32),
    /// 0x09 — set direct sampling: 0 = off, 1 = I-branch, 2 = Q-branch.
    DirectSampling(u32),
    /// 0x0e — set bias-tee: true powers the inline LNA/antenna over coax.
    BiasTee(bool),
}

impl RtlCmd {
    /// Encode to the 5-byte `[opcode][u32 BE arg]` wire form.
    pub fn encode(&self) -> [u8; 5] {
        let (op, arg) = match *self {
            RtlCmd::CenterFreq(hz) => (0x01, hz),
            RtlCmd::SampleRate(hz) => (0x02, hz),
            RtlCmd::GainMode(manual) => (0x03, manual as u32),
            RtlCmd::TunerGain(tenths) => (0x04, tenths as u32),
            RtlCmd::FreqCorrection(ppm) => (0x05, ppm as u32),
            RtlCmd::DirectSampling(mode) => (0x09, mode),
            RtlCmd::BiasTee(on) => (0x0e, on as u32),
        };
        let b = arg.to_be_bytes();
        [op, b[0], b[1], b[2], b[3]]
    }
}

// ---------------------------------------------------------------------------
// Runtime control cell
// ---------------------------------------------------------------------------

/// Full-rate ADS-B capture rate (2.4 Msps): the dongle rate the [`DemodMode::RawMag`]
/// path streams so the RX worker can resample it to the `adsb` demod's 2 MHz native
/// rate. 2.4M is a rate every R820-class dongle accepts (see [`supported_sample_rates`]).
pub const ADSB_CAPTURE_RATE: u32 = 2_400_000;

/// ADS-B downlink frequency (1090.0 MHz). Unlike the audio SDR modes, ADS-B is
/// fixed-frequency — the operator never tunes it — so the daemon must tune the
/// dongle here itself when it binds an ADS-B channel. The magnitude envelope
/// `|I+jQ|` the [`DemodMode::RawMag`] path decodes is invariant to a residual
/// carrier offset, so [`plan_tune`] can (and does) place 1090 MHz a quarter-band
/// off hardware center to clear the R820T DC spike without hurting the decode.
pub const ADSB_FREQ_HZ: f64 = 1_090_000_000.0;

/// Selectable demodulator. The audio modes (NBFM/AM/WFM/SSB) are dispatched by the
/// capture thread via [`SdrDemod`], which tunes + channelizes to narrowband audio.
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

    /// Map to the DSP crate's mode enum, which drives [`SdrDemod`]'s back-end.
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
// Backend
// ---------------------------------------------------------------------------

/// An `rtl_tcp` SDR endpoint bound as an audio capture device. RX-only: dongles
/// cannot transmit, so `open_playback` reports `Unsupported`.
pub struct RtlTcpBackend {
    host: String,
    port: u16,
    capture_rate: u32,
    deviation_hz: f32,
    control: SdrControl,
    telemetry: Option<broadcast::Sender<TelemetryEvent>>,
    channel: ChannelId,
}

impl RtlTcpBackend {
    /// Construct a backend for `host:port` with default capture rate/deviation
    /// and a fresh control cell. The core replaces the control and wires the
    /// telemetry sink + channel via [`AudioBackend::attach_sdr_context`].
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        RtlTcpBackend {
            host: host.into(),
            port,
            capture_rate: DEFAULT_CAPTURE_RATE,
            deviation_hz: DEFAULT_DEVIATION_HZ,
            control: SdrControl::default(),
            telemetry: None,
            channel: ChannelId(0),
        }
    }

    /// The shared control cell (so the core can store a clone for gRPC to mutate).
    pub fn control(&self) -> SdrControl {
        self.control.clone()
    }

    /// Override the dongle capture (sample) rate. Kept a multiple of the audio
    /// channel rate so the complex decimator has an integer ratio.
    pub fn with_capture_rate(mut self, rate: u32) -> Self {
        self.capture_rate = rate;
        self
    }
}

/// Send the dongle's initial parameters from the current control snapshot: rate,
/// ppm, gain mode/level, then center frequency (order mirrors `rtl_tcp` clients).
fn send_initial_commands(
    sock: &mut TcpStream,
    capture_rate: u32,
    control: &SdrControl,
) -> Result<(), AudioError> {
    let io = |e: std::io::Error| AudioError::Io(e.to_string());
    sock.write_all(&RtlCmd::SampleRate(capture_rate).encode()).map_err(io)?;
    sock.write_all(&RtlCmd::FreqCorrection(control.ppm()).encode()).map_err(io)?;
    sock.write_all(&RtlCmd::DirectSampling(control.direct_sampling()).encode()).map_err(io)?;
    sock.write_all(&RtlCmd::BiasTee(control.bias_tee()).encode()).map_err(io)?;
    sock.write_all(&RtlCmd::GainMode(!control.gain_auto()).encode()).map_err(io)?;
    if !control.gain_auto() {
        let tenths = (control.gain_db() * 10.0).round() as i32;
        sock.write_all(&RtlCmd::TunerGain(tenths).encode()).map_err(io)?;
    }
    sock.write_all(&RtlCmd::CenterFreq(control.center_hz() as u32).encode()).map_err(io)?;
    Ok(())
}

/// Connect to `addr`, read and validate the 12-byte greeting, publish the tuner
/// capabilities it reveals, and send the dongle its initial parameters from the
/// current control snapshot. Returns the read socket plus a cloned command
/// socket. Reused for both the initial connect and every reconnect, so a
/// re-established link always comes up fully re-tuned from `SdrControl` (the
/// single source of truth that survives a dropped connection).
fn connect_and_handshake(
    addr: &str,
    control: &SdrControl,
) -> Result<(TcpStream, TcpStream), AudioError> {
    let mut sock = connect_with_timeout(addr, CONNECT_TIMEOUT)?;
    // Bound the header read: a server that accepts but never sends the greeting
    // must not park the thread (it isn't in the shutdown slot until handshake
    // completes, so the stop hook cannot reach it).
    sock.set_read_timeout(Some(HEADER_READ_TIMEOUT))
        .map_err(|e| AudioError::Io(e.to_string()))?;
    let mut header = [0u8; 12];
    sock.read_exact(&mut header).map_err(|e| AudioError::Io(e.to_string()))?;
    // Restore blocking reads for the streaming loop, which is instead unblocked by
    // the stop hook shutting the (now published) socket down.
    sock.set_read_timeout(None).map_err(|e| AudioError::Io(e.to_string()))?;
    let hdr = parse_header(&header)?;
    control.set_caps(caps_from_header(&hdr));
    send_initial_commands(&mut sock, control.capture_rate(), control)?;
    let cmd_sock = sock.try_clone().map_err(|e| AudioError::Io(e.to_string()))?;
    Ok((sock, cmd_sock))
}

/// Connect to `addr` with a bounded timeout, trying each resolved socket address.
/// `TcpStream::connect` blocks with no ceiling on a half-open host, which would
/// leave a reconnecting capture thread unable to observe `stop`; `connect_timeout`
/// needs a resolved `SocketAddr`, so resolve first and try them in turn.
fn connect_with_timeout(addr: &str, timeout: Duration) -> Result<TcpStream, AudioError> {
    let resolved = addr
        .to_socket_addrs()
        .map_err(|e| AudioError::Io(format!("rtl_tcp resolve {addr}: {e}")))?;
    let mut last_err: Option<std::io::Error> = None;
    for sa in resolved {
        match TcpStream::connect_timeout(&sa, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last_err = Some(e),
        }
    }
    Err(AudioError::Io(format!(
        "rtl_tcp connect {addr}: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no addresses resolved".to_string())
    )))
}

/// Sleep one reconnect-backoff step, waking every 100 ms to honor `stop`, then
/// advance the index. Mirrors `cpal_backend::backoff_wait`.
fn backoff_wait(idx: &mut usize, stop: &AtomicBool) {
    let dur = RECONNECT_BACKOFF[(*idx).min(RECONNECT_BACKOFF.len() - 1)];
    let mut waited = Duration::ZERO;
    while waited < dur && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
        waited += Duration::from_millis(100);
    }
    *idx = (*idx + 1).min(RECONNECT_BACKOFF.len() - 1);
}

/// Outcome of a non-blocking audio delivery attempt.
enum Delivery {
    /// The consumer is still connected (chunk queued, or dropped under overrun).
    Live,
    /// The consumer dropped its receiver — the capture is terminal.
    ConsumerGone,
}

/// Deliver `chunk` to the bounded consumer channel without ever blocking the
/// socket read. `backlog` stages whatever the channel won't accept right now; when
/// the *backlog* grows past `CHUNK_QUEUE_DEPTH` its oldest chunk is dropped (a live
/// modem wants fresh audio, not stale backlog) and counted on `control`. Note the
/// dropped chunk is the oldest *un-accepted* one — the strictly-oldest chunks are
/// already in the consumer channel — so worst-case buffering is the channel depth
/// plus the backlog depth (~2·`CHUNK_QUEUE_DEPTH`), which bounds latency without a
/// second thread. Returns `ConsumerGone` once the receiver is gone.
fn deliver_audio(
    tx: &SyncSender<AudioChunk>,
    backlog: &mut VecDeque<AudioChunk>,
    chunk: AudioChunk,
    control: &SdrControl,
) -> Delivery {
    backlog.push_back(chunk);
    // Push as much of the backlog as the consumer will take right now.
    while let Some(front) = backlog.pop_front() {
        match tx.try_send(front) {
            Ok(()) => {}
            Err(TrySendError::Full(front)) => {
                backlog.push_front(front);
                break;
            }
            Err(TrySendError::Disconnected(_)) => return Delivery::ConsumerGone,
        }
    }
    // Bound the staged backlog by dropping the oldest chunks the consumer is too
    // slow to accept, so latency stays bounded and capture keeps reading.
    while backlog.len() > CHUNK_QUEUE_DEPTH {
        backlog.pop_front();
        let total = control.incr_dropped();
        // Surface the onset (first-ever drop) immediately, then rate-limit so a
        // sustained lag reports its running total without flooding the log.
        if total == 1 || total.is_multiple_of(OVERRUN_LOG_EVERY) {
            tracing::warn!(
                dropped = total,
                "rtl_tcp capture overrun: consumer lagging, dropped oldest queued audio"
            );
        }
    }
    Delivery::Live
}

/// Re-apply hardware parameters after a control change: ppm, direct-sampling,
/// bias-tee, gain mode/level, and center frequency. NCO offset and squelch are
/// handled in-thread (no socket).
fn apply_hardware(
    sock: &mut TcpStream,
    control: &SdrControl,
) -> Result<(), AudioError> {
    let io = |e: std::io::Error| AudioError::Io(e.to_string());
    sock.write_all(&RtlCmd::FreqCorrection(control.ppm()).encode()).map_err(io)?;
    sock.write_all(&RtlCmd::DirectSampling(control.direct_sampling()).encode()).map_err(io)?;
    sock.write_all(&RtlCmd::BiasTee(control.bias_tee()).encode()).map_err(io)?;
    sock.write_all(&RtlCmd::GainMode(!control.gain_auto()).encode()).map_err(io)?;
    if !control.gain_auto() {
        let tenths = (control.gain_db() * 10.0).round() as i32;
        sock.write_all(&RtlCmd::TunerGain(tenths).encode()).map_err(io)?;
    }
    sock.write_all(&RtlCmd::CenterFreq(control.center_hz() as u32).encode()).map_err(io)?;
    Ok(())
}

/// Merge a carried odd byte (a split IQ pair left over from the previous read)
/// with a freshly read chunk into a whole-pair byte buffer, returning the paired
/// bytes and any new leftover byte to carry into the next read.
fn merge_iq_bytes(carry: Option<u8>, chunk: &[u8]) -> (Vec<u8>, Option<u8>) {
    let mut bytes = Vec::with_capacity(chunk.len() + 1);
    if let Some(c) = carry {
        bytes.push(c);
    }
    bytes.extend_from_slice(chunk);
    let next = if bytes.len() % 2 == 1 { bytes.pop() } else { None };
    (bytes, next)
}

/// Emit one RF-referenced waterfall line per complete STFT frame of the raw IQ.
/// `freq_start_hz` is absolute RF: bin[0] = hardware center − rate/2.
fn emit_waterfall(
    stft: &mut ComplexStft,
    iq: &[omnimodem_dsp::types::Cplx],
    capture_rate: u32,
    center_hz: f64,
    channel: ChannelId,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) {
    // Geometry is invariant for a given center; build it once per block, not per
    // FFT frame.
    let plan = SpectrumPlan::new_centered(
        WATERFALL_NFFT,
        capture_rate as f32,
        center_hz as f32,
        WATERFALL_BINS,
        -(capture_rate as f32) / 2.0,
        (capture_rate as f32) / 2.0,
    );
    for frame in stft.feed(iq) {
        let dbfs = full_spectrum_dbfs(&frame, stft.window_sum());
        let bins = plan.render(&dbfs);
        let timestamp_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let _ = telemetry.send(TelemetryEvent::SpectrumFrame {
            channel,
            timestamp_ns,
            freq_start_hz: plan.freq_start_hz,
            freq_step_hz: plan.freq_step_hz,
            db_floor: plan.db_floor,
            db_ceiling: plan.db_ceiling,
            bins,
            transmit: false,
        });
    }
}

impl AudioBackend for RtlTcpBackend {
    fn open_capture(&self, requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        // ADS-B binds the channel to the wideband `RawMag` path: it needs the full
        // magnitude envelope, not a channelized audio slice. The core sets the demod
        // mode before opening the capture, so detect it here and deliver samples at
        // the full 2.4 Msps ADS-B rate — bypassing the audio `MAX_SAMPLE_RATE` cap —
        // so the RX worker resamples 2.4M → the demod's 2 MHz native rate. Every
        // other mode keeps the decimate-to-audio path.
        let raw_mag = self.control.demod_mode() == DemodMode::RawMag;
        let seed_rate = if raw_mag { ADSB_CAPTURE_RATE } else { self.capture_rate };
        // The sample rate delivered downstream. Audio modes decimate to the (capped)
        // channel rate; `RawMag` delivers samples at the full capture rate.
        let channel_rate =
            if raw_mag { seed_rate } else { requested_rate.min(MAX_SAMPLE_RATE) };

        let addr = format!("{}:{}", self.host, self.port);

        // Seed the shared capture rate. The control cell is authoritative thereafter,
        // so `ConfigureSdr` can change the rate on a running (audio) capture.
        self.control.set_capture_rate(seed_rate);

        // Initial connect is synchronous so a bad address / non-rtl_tcp server
        // fails fast and the tuner caps publish before we return. This connection
        // becomes the supervisor loop's first iteration; every later drop
        // reconnects with backoff and re-applies the same params.
        let (sock, cmd_sock) = connect_and_handshake(&addr, &self.control)?;

        // The stop hook must unblock a thread parked in `sock.read()`, and it must
        // do so for whichever connection is currently live (reconnect swaps the
        // socket). Share a slot the capture thread refreshes on every (re)connect.
        let shutdown_slot: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(
            sock.try_clone().ok(),
        ));
        let shutdown_thread = shutdown_slot.clone();

        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();

        let control = self.control.clone();
        let telemetry = self.telemetry.clone();
        let channel = self.channel;
        let deviation_hz = self.deviation_hz;

        std::thread::Builder::new()
            .name("omni-rtl-capture".into())
            .spawn(move || {
                // Waterfall state, read buffer, overrun backlog, and reconnect
                // backoff all persist across reconnects (they track the consumer /
                // spectrum, not a single connection).
                let mut stft = ComplexStft::new(WATERFALL_NFFT, WATERFALL_NFFT);
                let mut buf = vec![0u8; WATERFALL_NFFT * 2];
                let mut backlog: VecDeque<AudioChunk> = VecDeque::new();
                let mut backoff = 0usize;
                // The established initial connection seeds the first iteration; the
                // supervisor reconnects thereafter.
                let mut seed = Some((sock, cmd_sock));

                'supervisor: while !stop_thread.load(Ordering::Relaxed) {
                    let (mut sock, mut cmd_sock) = match seed.take() {
                        Some(pair) => pair,
                        None => match connect_and_handshake(&addr, &control) {
                            Ok(pair) => pair,
                            Err(e) => {
                                tracing::warn!(%addr, error = %e, "rtl_tcp reconnect failed");
                                backoff_wait(&mut backoff, &stop_thread);
                                continue;
                            }
                        },
                    };
                    // Publish this connection's socket so the stop hook can shut it
                    // down, and reset the read boundary carry for the fresh stream.
                    *shutdown_thread.lock().unwrap() = sock.try_clone().ok();
                    let connected_at = Instant::now();

                    // Rebuild the RX chain from current control: params may have
                    // changed while the link was down, and the dongle was just
                    // re-commanded to match by `connect_and_handshake`.
                    let mut cur_rate = control.capture_rate();
                    let mut cur_mode = control.demod_mode();
                    // `RawMag` (ADS-B) bypasses the channelizing demod entirely and
                    // emits the full-rate magnitude envelope; build an `SdrDemod`
                    // only for the audio modes.
                    let mut raw_mag = cur_mode == DemodMode::RawMag;
                    let mut rx_chain = (!raw_mag).then(|| {
                        SdrDemod::new(
                            cur_mode.to_dsp(),
                            cur_rate,
                            channel_rate,
                            control.offset_hz(),
                            deviation_hz,
                            control.effective_squelch(),
                        )
                    });
                    let mut seen_gen = control.generation();
                    let mut carry: Option<u8> = None;

                    while !stop_thread.load(Ordering::Relaxed) {
                        let n = match sock.read(&mut buf) {
                            Ok(0) => break, // server closed — reconnect
                            Ok(n) => n,
                            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => break, // read error — reconnect
                        };

                        // Reassemble a whole-pair byte stream, carrying a split IQ
                        // pair across this read boundary.
                        let (bytes, next_carry) = merge_iq_bytes(carry.take(), &buf[..n]);
                        carry = next_carry;

                        let iq = u8_iq_to_cplx(&bytes);
                        if iq.is_empty() {
                            continue;
                        }

                        // Reconcile runtime control changes before demodulating.
                        let gen = control.generation();
                        if gen != seen_gen {
                            seen_gen = gen;
                            // A capture-rate or demod-mode change rebuilds the whole
                            // RX chain: the decimation ratio and NCO base rate depend
                            // on the rate, and the back-end (and WFM's wide IF) depend
                            // on the mode. A rate change also re-commands the dongle.
                            let want_rate = control.capture_rate();
                            let want_mode = control.demod_mode();
                            let rate_changed = want_rate != cur_rate && want_rate != 0;
                            if rate_changed || want_mode != cur_mode {
                                if rate_changed {
                                    cur_rate = want_rate;
                                }
                                cur_mode = want_mode;
                                // Rebuild the RX chain for the new rate/mode. Crossing
                                // the `RawMag` boundary at runtime changes the delivered
                                // sample rate, which a live capture can't re-negotiate;
                                // the core re-opens the capture on such a mode switch, so
                                // here we only track the flag and skip the audio demod.
                                raw_mag = cur_mode == DemodMode::RawMag;
                                rx_chain = (!raw_mag).then(|| {
                                    SdrDemod::new(
                                        cur_mode.to_dsp(),
                                        cur_rate,
                                        channel_rate,
                                        control.offset_hz(),
                                        deviation_hz,
                                        control.effective_squelch(),
                                    )
                                });
                                if rate_changed {
                                    let _ = cmd_sock.write_all(&RtlCmd::SampleRate(cur_rate).encode());
                                }
                            } else if let Some(rc) = rx_chain.as_mut() {
                                rc.retune(control.offset_hz());
                                rc.set_squelch(control.effective_squelch());
                            }
                            let _ = apply_hardware(&mut cmd_sock, &control);
                        }

                        if let Some(tele) = telemetry.as_ref() {
                            emit_waterfall(
                                &mut stft, &iq, cur_rate, control.center_hz(), channel, tele,
                            );
                        }

                        let audio = match rx_chain.as_mut() {
                            Some(rc) => rc.push_iq(&iq),
                            // `RawMag`: emit the full-rate magnitude envelope, scaled
                            // by 1/√2 so the u8-IQ maximum (|±1 ±1j| = √2) maps into
                            // [0,1] without clipping the i16 delivery path. The PPM
                            // demod is scale-independent, so the scale is otherwise
                            // free; it only keeps strong pulses from saturating.
                            None => iq.iter().map(|c| c.norm() * INV_SQRT2).collect(),
                        };
                        if audio.is_empty() {
                            continue;
                        }
                        let chunk: AudioChunk = audio
                            .iter()
                            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                            .collect();
                        // Never block the socket read: stage + drop-oldest on lag.
                        if let Delivery::ConsumerGone =
                            deliver_audio(&tx, &mut backlog, chunk, &control)
                        {
                            break 'supervisor; // consumer dropped — terminal
                        }
                    }

                    // The connection ended (not a consumer drop). Reset the backoff
                    // if it had streamed stably, then reconnect unless we're stopping.
                    if connected_at.elapsed() >= BACKOFF_RESET_AFTER {
                        backoff = 0;
                    }
                    if stop_thread.load(Ordering::Relaxed) {
                        break;
                    }
                    tracing::warn!(%addr, "rtl_tcp link dropped; reconnecting");
                    backoff_wait(&mut backoff, &stop_thread);
                }
            })
            .map_err(|e| AudioError::Io(e.to_string()))?;

        let stop_on_drop = stop;
        Ok(CaptureHandle::new(rx, channel_rate, move || {
            stop_on_drop.store(true, Ordering::Relaxed);
            // Unblock a read parked on a silent-but-open server by shutting down
            // whichever connection is currently live; a genuine EOF (Ok(0)) also
            // breaks the loop, so this is belt-and-suspenders.
            if let Some(s) = shutdown_slot.lock().unwrap().as_ref() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }))
    }

    fn open_playback(&self, _requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        // RTL dongles are receive-only.
        Err(AudioError::Unsupported)
    }

    fn device_id(&self) -> DeviceId {
        DeviceId::RtlTcp { host: self.host.clone(), port: self.port }
    }

    fn attach_sdr_context(
        &mut self,
        channel: ChannelId,
        telemetry: broadcast::Sender<TelemetryEvent>,
        control: SdrControl,
    ) {
        self.channel = channel;
        self.telemetry = Some(telemetry);
        self.control = control;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_parses_valid_greeting() {
        let mut buf = [0u8; 12];
        buf[0..4].copy_from_slice(b"RTL0");
        buf[4..8].copy_from_slice(&5u32.to_be_bytes()); // R820T
        buf[8..12].copy_from_slice(&29u32.to_be_bytes());
        let h = parse_header(&buf).unwrap();
        assert_eq!(h.tuner_type, 5);
        assert_eq!(h.tuner_gain_count, 29);
        assert_eq!(tuner_name(h.tuner_type), "R820T");
    }

    #[test]
    fn header_rejects_bad_magic() {
        let buf = [0u8; 12]; // magic is zeros, not "RTL0"
        assert!(parse_header(&buf).is_err());
    }

    #[test]
    fn tuner_name_unknown_fallback() {
        assert_eq!(tuner_name(999), "unknown");
    }

    #[test]
    fn merge_iq_bytes_carries_split_pair_across_reads() {
        // Even chunk, no carry: nothing left over.
        let (b, c) = merge_iq_bytes(None, &[10, 20, 30, 40]);
        assert_eq!(b, vec![10, 20, 30, 40]);
        assert_eq!(c, None);

        // Odd chunk: the trailing byte is held back for the next read.
        let (b, c) = merge_iq_bytes(None, &[10, 20, 30]);
        assert_eq!(b, vec![10, 20]);
        assert_eq!(c, Some(30));

        // The carried byte is prepended and re-paired on the next read; a new
        // odd length carries again.
        let (b, c) = merge_iq_bytes(Some(30), &[40, 50]);
        assert_eq!(b, vec![30, 40]);
        assert_eq!(c, Some(50));

        // Carry + odd chunk that makes an even total: fully consumed.
        let (b, c) = merge_iq_bytes(Some(1), &[2, 3, 4]);
        assert_eq!(b, vec![1, 2, 3, 4]);
        assert_eq!(c, None);
    }

    #[test]
    fn commands_encode_opcode_and_be_arg() {
        assert_eq!(RtlCmd::CenterFreq(144_390_000).encode(), {
            let a = 144_390_000u32.to_be_bytes();
            [0x01, a[0], a[1], a[2], a[3]]
        });
        assert_eq!(RtlCmd::SampleRate(240_000).encode(), {
            let a = 240_000u32.to_be_bytes();
            [0x02, a[0], a[1], a[2], a[3]]
        });
        assert_eq!(RtlCmd::GainMode(true).encode(), [0x03, 0, 0, 0, 1]);
        assert_eq!(RtlCmd::GainMode(false).encode(), [0x03, 0, 0, 0, 0]);
        assert_eq!(RtlCmd::TunerGain(496).encode(), [0x04, 0, 0, 0x01, 0xF0]);
        // -1 ppm two's-complement round-trips through the u32 BE arg.
        assert_eq!(RtlCmd::FreqCorrection(-1).encode(), [0x05, 0xFF, 0xFF, 0xFF, 0xFF]);
        // Phase C: direct sampling (0x09) and bias-tee (0x0e).
        assert_eq!(RtlCmd::DirectSampling(0).encode(), [0x09, 0, 0, 0, 0]);
        assert_eq!(RtlCmd::DirectSampling(1).encode(), [0x09, 0, 0, 0, 1]);
        assert_eq!(RtlCmd::DirectSampling(2).encode(), [0x09, 0, 0, 0, 2]);
        assert_eq!(RtlCmd::BiasTee(true).encode(), [0x0e, 0, 0, 0, 1]);
        assert_eq!(RtlCmd::BiasTee(false).encode(), [0x0e, 0, 0, 0, 0]);
    }

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

    use std::net::TcpListener;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    /// FM-modulate a `tone_hz` sine at `offset_hz` into u8 IQ at `rate`.
    fn fm_iq_u8(rate: f32, offset_hz: f32, tone_hz: f32, dev_hz: f32, n: usize) -> Vec<u8> {
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(n * 2);
        for k in 0..n {
            let t = k as f32 / rate;
            let inst = offset_hz + dev_hz * (std::f32::consts::TAU * tone_hz * t).sin();
            phase += std::f32::consts::TAU * inst / rate;
            let i = ((phase.cos() * 0.9 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
            let q = ((phase.sin() * 0.9 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
            out.push(i);
            out.push(q);
        }
        out
    }

    /// Start an in-process `rtl_tcp` server: write the 12-byte header, drain
    /// client commands on a side thread, stream `iq`, then close. Returns the
    /// bound port.
    fn spawn_fake_server(iq: Vec<u8>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut header = [0u8; 12];
            header[0..4].copy_from_slice(b"RTL0");
            header[4..8].copy_from_slice(&5u32.to_be_bytes());
            header[8..12].copy_from_slice(&29u32.to_be_bytes());
            sock.write_all(&header).unwrap();
            // Drain whatever commands the client sends without blocking the write.
            let mut drain = sock.try_clone().unwrap();
            std::thread::spawn(move || {
                let mut sink = [0u8; 64];
                while let Ok(n) = drain.read(&mut sink) {
                    if n == 0 {
                        break;
                    }
                }
            });
            sock.write_all(&iq).unwrap();
            // Signal EOF on the read side even though the drain clone lingers, so
            // the client's capture loop terminates.
            let _ = sock.shutdown(std::net::Shutdown::Write);
        });
        port
    }

    #[test]
    fn capture_reads_header_and_delivers_audio() {
        let iq = fm_iq_u8(DEFAULT_CAPTURE_RATE as f32, 30_000.0, 1_200.0, DEFAULT_DEVIATION_HZ, 48_000);
        let port = spawn_fake_server(iq);
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        backend.control.set_offset_hz(30_000.0);
        let cap = backend.open_capture(48_000).unwrap();
        assert_eq!(cap.sample_rate, 48_000);

        // Drain the burst. The server serves one connection then closes, and the
        // capture now *reconnects* (Phase D) rather than terminating, so we collect
        // until the stream goes quiet instead of waiting for a disconnect.
        let total = drain_burst(&cap.rx);
        assert!(total > 0, "expected demodulated audio samples, got none");
    }

    /// Accumulate delivered audio-sample counts until the stream is quiet (a short
    /// recv timeout) or the sender disconnects. Used by the single-burst fake-server
    /// tests, where the capture keeps running (and retrying) after the burst.
    fn drain_burst(rx: &std::sync::mpsc::Receiver<AudioChunk>) -> usize {
        let mut total = 0usize;
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(chunk) => total += chunk.len(),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        total
    }

    /// AM-modulate a `tone_hz` sine at `offset_hz` into u8 IQ at `rate`.
    fn am_iq_u8(rate: f32, offset_hz: f32, tone_hz: f32, m: f32, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n * 2);
        for k in 0..n {
            let t = k as f32 / rate;
            let env = 1.0 + m * (std::f32::consts::TAU * tone_hz * t).sin();
            let carr = std::f32::consts::TAU * offset_hz * t;
            let i = ((env * carr.cos() * 0.45 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
            let q = ((env * carr.sin() * 0.45 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
            out.push(i);
            out.push(q);
        }
        out
    }

    #[test]
    fn capture_am_mode_delivers_audio() {
        // With the demod mode set to AM before capture, an AM-modulated stream must
        // still produce audio through the `SdrDemod` dispatch (not just NBFM).
        let iq = am_iq_u8(DEFAULT_CAPTURE_RATE as f32, 30_000.0, 1_000.0, 0.5, 48_000);
        let port = spawn_fake_server(iq);
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        backend.control.set_offset_hz(30_000.0);
        backend.control.set_demod_mode(DemodMode::Am);
        let cap = backend.open_capture(48_000).unwrap();

        let total = drain_burst(&cap.rx);
        assert!(total > 0, "expected demodulated AM audio, got none");
    }

    #[test]
    fn raw_mag_capture_delivers_full_rate_magnitude() {
        // RawMag (ADS-B) must bypass the channelizer: deliver |I+jQ| at the full
        // 2.4 Msps capture rate (not the 48 kHz audio cap), scaled by 1/√2.
        // Constant near-full-scale I with zero Q → magnitude ≈ 1.0 → delivered ≈ 0.707.
        let iq: Vec<u8> = std::iter::repeat_n([255u8, 127u8], 4000).flatten().collect();
        let port = spawn_fake_server(iq);
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        backend.control.set_demod_mode(DemodMode::RawMag);
        let cap = backend.open_capture(48_000).unwrap();
        // The delivered rate is the full ADS-B capture rate, NOT the 48 kHz cap.
        assert_eq!(cap.sample_rate, ADSB_CAPTURE_RATE);

        // Collect the burst; every delivered sample is the scaled magnitude ≈ 0.707.
        let mut samples: Vec<i16> = Vec::new();
        while let Ok(chunk) = cap.rx.recv_timeout(Duration::from_millis(500)) {
            samples.extend(chunk);
        }
        assert!(!samples.is_empty(), "no magnitude samples delivered");
        let expect = (INV_SQRT2 * 32767.0) as i16; // ≈ 23170
        // Allow a few LSB slack for the 1/127.5 quantization of the u8 IQ.
        let within = samples.iter().filter(|&&s| (s - expect).abs() < 300).count();
        assert!(
            within as f32 > samples.len() as f32 * 0.9,
            "magnitude not ≈{expect}: {within}/{} within tol",
            samples.len(),
        );
    }

    #[test]
    fn capture_publishes_tuner_caps() {
        // The fake server advertises an R820T; caps must be published once the
        // header is parsed, so a later GetSdrCaps can answer.
        let iq = fm_iq_u8(DEFAULT_CAPTURE_RATE as f32, 30_000.0, 1_200.0, DEFAULT_DEVIATION_HZ, 4_000);
        let port = spawn_fake_server(iq);
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        let caps_cell = backend.control();
        assert!(caps_cell.caps().is_none());
        let _cap = backend.open_capture(48_000).unwrap();
        let caps = caps_cell.caps().expect("caps published after connect");
        assert_eq!(caps.tuner, "R820T");
        assert_eq!(caps_cell.capture_rate(), DEFAULT_CAPTURE_RATE);
    }

    /// A fake server that streams IQ continuously (so the capture loop keeps
    /// reading) and records every 5-byte command the client sends, until `stop`.
    fn spawn_recording_server() -> (u16, Arc<Mutex<Vec<u8>>>, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cmds = Arc::new(Mutex::new(Vec::<u8>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let cmds_srv = cmds.clone();
        let stop_srv = stop.clone();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut header = [0u8; 12];
            header[0..4].copy_from_slice(b"RTL0");
            header[4..8].copy_from_slice(&5u32.to_be_bytes());
            header[8..12].copy_from_slice(&29u32.to_be_bytes());
            sock.write_all(&header).unwrap();
            // Record the client's control commands on a side thread.
            let mut drain = sock.try_clone().unwrap();
            std::thread::spawn(move || {
                let mut buf = [0u8; 64];
                while let Ok(n) = drain.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    cmds_srv.lock().unwrap().extend_from_slice(&buf[..n]);
                }
            });
            // Keep feeding IQ so the capture loop wakes and reconciles control
            // changes (a parked read would never see a mid-stream rate change).
            let chunk = vec![127u8; 512];
            while !stop_srv.load(Ordering::Relaxed) {
                if sock.write_all(&chunk).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        (port, cmds, stop)
    }

    #[test]
    fn live_capture_rate_change_resends_sample_rate() {
        // A runtime capture-rate change must re-command the dongle's sample rate on
        // the running capture (and rebuild the decimation chain). Assert the new
        // SampleRate command reaches the server.
        let (port, cmds, stop) = spawn_recording_server();
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        let control = backend.control();
        let cap = backend.open_capture(48_000).unwrap();

        // Scan the recorded byte stream (a concatenation of 5-byte frames) for a
        // SampleRate (opcode 0x02) command carrying `rate`. The server records
        // commands asynchronously, so poll — draining audio each iteration both
        // paces the wait and keeps the capture loop's bounded queue unblocked.
        let saw_sample_rate = |rate: u32| -> bool {
            cmds.lock().unwrap().chunks_exact(5).any(|f| {
                f[0] == 0x02 && u32::from_be_bytes([f[1], f[2], f[3], f[4]]) == rate
            })
        };
        let wait_for_rate = |rate: u32, iters: usize| -> bool {
            for _ in 0..iters {
                if saw_sample_rate(rate) {
                    return true;
                }
                let _ = cap.rx.recv_timeout(Duration::from_millis(10));
            }
            saw_sample_rate(rate)
        };

        // The initial SampleRate(240000) is sent during `open_capture`.
        assert!(wait_for_rate(DEFAULT_CAPTURE_RATE, 200), "initial SampleRate not observed");

        // Wait until the capture thread has produced audio, proving it latched the
        // initial rate + generation before we change them. Without this, a
        // late-starting thread would initialise straight to the new rate and never
        // emit the runtime re-command.
        assert!(
            cap.rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "capture produced no audio at the initial rate"
        );

        // Change the rate at runtime; the capture thread must re-command it.
        control.set_capture_rate(1_024_000);
        let ok = wait_for_rate(1_024_000, 300);
        stop.store(true, Ordering::Relaxed);
        drop(cap);
        assert!(ok, "runtime SampleRate(1024000) command not observed");
    }

    #[test]
    fn live_bias_tee_and_direct_sampling_reach_the_dongle() {
        // Phase C: toggling bias-tee / direct-sampling on a running capture must
        // send the exact 0x0e / 0x09 command frames over the socket.
        let (port, cmds, stop) = spawn_recording_server();
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        let control = backend.control();
        let cap = backend.open_capture(48_000).unwrap();

        // Scan the concatenated 5-byte frame stream for an exact [op | u32 BE arg].
        let saw_cmd = |op: u8, arg: u32| -> bool {
            cmds.lock().unwrap().chunks_exact(5).any(|f| {
                f[0] == op && u32::from_be_bytes([f[1], f[2], f[3], f[4]]) == arg
            })
        };
        let wait_for = |op: u8, arg: u32, iters: usize| -> bool {
            for _ in 0..iters {
                if saw_cmd(op, arg) {
                    return true;
                }
                let _ = cap.rx.recv_timeout(Duration::from_millis(10));
            }
            saw_cmd(op, arg)
        };

        // Connect handshake seeds bias-tee off (0x0e,0) and direct-sampling off
        // (0x09,0).
        assert!(wait_for(0x0e, 0, 200), "initial BiasTee(off) not observed");
        assert!(wait_for(0x09, 0, 200), "initial DirectSampling(off) not observed");

        // Prove the capture thread latched the initial generation before we mutate.
        assert!(
            cap.rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "capture produced no audio before the control change"
        );

        control.set_bias_tee(true);
        control.set_direct_sampling(2);
        let bias_ok = wait_for(0x0e, 1, 300);
        let ds_ok = wait_for(0x09, 2, 300);
        stop.store(true, Ordering::Relaxed);
        drop(cap);
        assert!(bias_ok, "runtime BiasTee(on) command not observed");
        assert!(ds_ok, "runtime DirectSampling(Q) command not observed");
    }

    /// The 12-byte R820T greeting the fake servers all send.
    fn fake_header() -> [u8; 12] {
        let mut h = [0u8; 12];
        h[0..4].copy_from_slice(b"RTL0");
        h[4..8].copy_from_slice(&5u32.to_be_bytes());
        h[8..12].copy_from_slice(&29u32.to_be_bytes());
        h
    }

    /// True if the recorded 5-byte command stream contains `[op | u32 BE arg]`.
    fn cmd_present(cmds: &Arc<Mutex<Vec<u8>>>, op: u8, arg: u32) -> bool {
        cmds.lock().unwrap().chunks_exact(5).any(|f| {
            f[0] == op && u32::from_be_bytes([f[1], f[2], f[3], f[4]]) == arg
        })
    }

    /// A fake server that serves ONE connection (header + `iq0`) then drops it to
    /// simulate a link loss, then accepts a SECOND connection, records the
    /// re-applied commands into the returned buffer, and streams `iq1` until stop.
    fn spawn_reconnecting_server(
        iq0: Vec<u8>,
        iq1: Vec<u8>,
    ) -> (u16, Arc<Mutex<Vec<u8>>>, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cmds2 = Arc::new(Mutex::new(Vec::<u8>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let cmds2_srv = cmds2.clone();
        let stop_srv = stop.clone();
        std::thread::spawn(move || {
            // Connection 0: header + IQ, then hard-drop to force a reconnect.
            if let Ok((mut sock, _)) = listener.accept() {
                let _ = sock.write_all(&fake_header());
                let mut drain = sock.try_clone().unwrap();
                std::thread::spawn(move || {
                    let mut sink = [0u8; 64];
                    while let Ok(n) = drain.read(&mut sink) {
                        if n == 0 {
                            break;
                        }
                    }
                });
                let _ = sock.write_all(&iq0);
                // Let the client finish its handshake and demodulate the burst
                // before we hard-drop the link (a fast Both-shutdown would race the
                // client's initial command writes into a broken pipe).
                std::thread::sleep(Duration::from_millis(200));
                let _ = sock.shutdown(std::net::Shutdown::Both);
            }
            // Connection 1: record the re-applied commands, stream IQ until stop.
            if let Ok((mut sock, _)) = listener.accept() {
                let _ = sock.write_all(&fake_header());
                let mut drain = sock.try_clone().unwrap();
                let rec = cmds2_srv.clone();
                std::thread::spawn(move || {
                    let mut sink = [0u8; 64];
                    while let Ok(n) = drain.read(&mut sink) {
                        if n == 0 {
                            break;
                        }
                        rec.lock().unwrap().extend_from_slice(&sink[..n]);
                    }
                });
                while !stop_srv.load(Ordering::Relaxed) {
                    if sock.write_all(&iq1).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        });
        (port, cmds2, stop)
    }

    #[test]
    fn capture_reconnects_and_reapplies_params_after_link_drop() {
        // A transient link loss must not lose the operator's tune/gain: the
        // supervisor reconnects and re-applies every hardware param from control.
        let iq0 = fm_iq_u8(DEFAULT_CAPTURE_RATE as f32, 30_000.0, 1_200.0, DEFAULT_DEVIATION_HZ, 24_000);
        let iq1 = vec![127u8; 4096];
        let (port, cmds2, stop) = spawn_reconnecting_server(iq0, iq1);

        let backend = RtlTcpBackend::new("127.0.0.1", port);
        let control = backend.control();
        // Operator state that must survive the reconnect.
        control.set_center_hz(144_500_000.0);
        control.set_offset_hz(30_000.0);
        control.set_gain(false, 30.0);

        let cap = backend.open_capture(48_000).unwrap();

        // Audio flows on the first connection.
        assert!(
            cap.rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "no audio before the link drop"
        );

        // After the drop the supervisor reconnects (100 ms backoff) and re-applies
        // the tune. Poll the second connection's recorded commands, draining audio
        // each iteration to keep the bounded queue moving.
        let mut reconnected = false;
        for _ in 0..300 {
            if cmd_present(&cmds2, 0x01, 144_500_000) {
                reconnected = true;
                break;
            }
            let _ = cap.rx.recv_timeout(Duration::from_millis(20));
        }
        assert!(reconnected, "did not observe re-applied CenterFreq on the reconnect");

        // Manual gain mode + level were re-applied too (GainMode(manual)=0x03,1 and
        // TunerGain=0x04, 30.0 dB → 300 tenths).
        assert!(cmd_present(&cmds2, 0x03, 1), "GainMode(manual) not re-applied");
        assert!(cmd_present(&cmds2, 0x04, 300), "TunerGain(30 dB) not re-applied");

        // Audio resumes on the reconnected link.
        assert!(
            cap.rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "audio did not resume after reconnect"
        );

        stop.store(true, Ordering::Relaxed);
        drop(cap);
    }

    /// A fake server that blasts constant IQ as fast as the socket accepts it, so a
    /// non-draining consumer forces the capture into overrun.
    fn spawn_fast_stream_server() -> (u16, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_srv = stop.clone();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let _ = sock.write_all(&fake_header());
                let mut drain = sock.try_clone().unwrap();
                std::thread::spawn(move || {
                    let mut sink = [0u8; 64];
                    while let Ok(n) = drain.read(&mut sink) {
                        if n == 0 {
                            break;
                        }
                    }
                });
                let chunk = vec![127u8; 16 * 1024];
                while !stop_srv.load(Ordering::Relaxed) {
                    if sock.write_all(&chunk).is_err() {
                        break;
                    }
                }
            }
        });
        (port, stop)
    }

    #[test]
    fn overrun_drops_oldest_and_keeps_capturing() {
        // With the consumer never draining, the bounded queue fills and the
        // capture must drop-oldest and keep reading rather than block the socket.
        let (port, stop) = spawn_fast_stream_server();
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        let control = backend.control();
        control.set_offset_hz(30_000.0);
        let cap = backend.open_capture(48_000).unwrap();

        // Deliberately do NOT drain `cap.rx`. Drops must start accumulating.
        let mut saw_drops = false;
        for _ in 0..300 {
            if control.dropped_chunks() > 0 {
                saw_drops = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(saw_drops, "expected overrun drops with a stalled consumer");

        // The counter keeps climbing: the capture thread is still producing (it did
        // not block on the full channel).
        let d1 = control.dropped_chunks();
        std::thread::sleep(Duration::from_millis(100));
        let d2 = control.dropped_chunks();
        assert!(d2 > d1, "capture stopped producing under overrun (blocked?)");

        stop.store(true, Ordering::Relaxed);
        drop(cap);
    }

    #[test]
    fn bad_header_fails_capture() {
        // A server that sends a wrong magic must make open_capture error.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.write_all(&[0u8; 12]).unwrap();
        });
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        assert!(backend.open_capture(48_000).is_err());
    }

    #[test]
    fn stalled_header_times_out_instead_of_hanging() {
        // A server that accepts the connection but never sends the 12-byte greeting
        // must not park the capture indefinitely: the bounded header-read timeout
        // makes `open_capture` fail (well within the timeout budget) rather than
        // hang forever. Guards the reconnect-honors-stop invariant at the handshake.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hold = std::thread::spawn(move || {
            // Accept and then sit silent until the client gives up and closes.
            let (sock, _) = listener.accept().unwrap();
            let mut sink = [0u8; 64];
            let mut s = sock;
            let _ = s.read(&mut sink);
        });
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        let started = std::time::Instant::now();
        let result = backend.open_capture(48_000);
        let elapsed = started.elapsed();
        assert!(result.is_err(), "stalled header should fail open_capture");
        assert!(
            elapsed < HEADER_READ_TIMEOUT + Duration::from_secs(5),
            "open_capture hung past the header-read timeout ({elapsed:?})"
        );
        drop(hold);
    }

    #[test]
    fn playback_is_unsupported() {
        let backend = RtlTcpBackend::new("127.0.0.1", 1234);
        assert!(matches!(backend.open_playback(48_000), Err(AudioError::Unsupported)));
        assert_eq!(backend.device_id(), DeviceId::RtlTcp { host: "127.0.0.1".into(), port: 1234 });
    }
}
