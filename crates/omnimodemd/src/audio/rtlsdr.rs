//! `rtl_tcp` SDR backend: connect to an `rtl_tcp` server (local or remote), tune
//! the dongle, read raw u8 IQ, demodulate to mono audio via the DSP crate's
//! `NbfmReceiver`, and stream a wideband RF waterfall — all behind the existing
//! `AudioBackend` seam so every downstream mode works unmodified.
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
use omnimodem_dsp::frontend::nbfm::NbfmReceiver;
use omnimodem_dsp::frontend::spectrum::{full_spectrum_dbfs, SpectrumPlan};
use omnimodem_dsp::frontend::squelch::PowerSquelch;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering,
};
use std::sync::{Arc, Mutex};
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

/// Compose the published capabilities from a parsed header. Bias-tee and
/// direct-sampling are Phase C, so both are reported unsupported for now.
pub fn caps_from_header(h: &RtlHeader) -> TunerCaps {
    let (freq_min_hz, freq_max_hz) = tuner_freq_range(h.tuner_type);
    TunerCaps {
        tuner: tuner_name(h.tuner_type).to_string(),
        freq_min_hz,
        freq_max_hz,
        sample_rates: supported_sample_rates(),
        gains_db: tuner_gains_db(h.tuner_type),
        bias_tee_supported: false,
        direct_sampling_supported: false,
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
        };
        let b = arg.to_be_bytes();
        [op, b[0], b[1], b[2], b[3]]
    }
}

// ---------------------------------------------------------------------------
// Runtime control cell
// ---------------------------------------------------------------------------

/// Selectable demodulator. NBFM is implemented in Phase A; the rest ship in the
/// enum so the control surface is stable, and return "unimplemented" until their
/// phase lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DemodMode {
    Nbfm = 0,
    Am = 1,
    Wfm = 2,
    Usb = 3,
    Lsb = 4,
}

impl DemodMode {
    pub fn from_u8(v: u8) -> DemodMode {
        match v {
            1 => DemodMode::Am,
            2 => DemodMode::Wfm,
            3 => DemodMode::Usb,
            4 => DemodMode::Lsb,
            _ => DemodMode::Nbfm,
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
    /// Demod mode (`DemodMode as u8`).
    demod_mode: AtomicU8,
    /// Dongle capture (sample) rate (Hz). Set by `ConfigureSdr`; the capture
    /// thread re-applies it live. Also read by the tune split to size the band.
    capture_rate: AtomicU32,
    /// Tuner capabilities, published by the capture thread once the header is
    /// parsed. `None` until a capture has connected.
    caps: Mutex<Option<TunerCaps>>,
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
                demod_mode: AtomicU8::new(DemodMode::Nbfm as u8),
                capture_rate: AtomicU32::new(DEFAULT_CAPTURE_RATE),
                caps: Mutex::new(None),
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
    sock.write_all(&RtlCmd::GainMode(!control.gain_auto()).encode()).map_err(io)?;
    if !control.gain_auto() {
        let tenths = (control.gain_db() * 10.0).round() as i32;
        sock.write_all(&RtlCmd::TunerGain(tenths).encode()).map_err(io)?;
    }
    sock.write_all(&RtlCmd::CenterFreq(control.center_hz() as u32).encode()).map_err(io)?;
    Ok(())
}

/// Re-apply hardware parameters after a control change: gain mode/level, ppm, and
/// center frequency. NCO offset and squelch are handled in-thread (no socket).
fn apply_hardware(
    sock: &mut TcpStream,
    control: &SdrControl,
) -> Result<(), AudioError> {
    let io = |e: std::io::Error| AudioError::Io(e.to_string());
    sock.write_all(&RtlCmd::FreqCorrection(control.ppm()).encode()).map_err(io)?;
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
        // The audio channel rate delivered downstream (capped like the soundcard
        // path). The dongle streams at `self.capture_rate`; we decimate to this.
        let channel_rate = requested_rate.min(MAX_SAMPLE_RATE);

        let addr = format!("{}:{}", self.host, self.port);
        let mut sock = TcpStream::connect(&addr)
            .map_err(|e| AudioError::Io(format!("rtl_tcp connect {addr}: {e}")))?;

        // Read + validate the 12-byte greeting before anything else. Publish the
        // tuner capabilities it reveals so `GetSdrCaps` can answer once bound.
        let mut header = [0u8; 12];
        sock.read_exact(&mut header).map_err(|e| AudioError::Io(e.to_string()))?;
        let hdr = parse_header(&header)?;
        self.control.set_caps(caps_from_header(&hdr));

        // Seed the shared capture rate from this backend's configured default
        // (unity in production); the control cell is authoritative thereafter, so
        // `ConfigureSdr` can change the rate on a running capture.
        self.control.set_capture_rate(self.capture_rate);
        send_initial_commands(&mut sock, self.capture_rate, &self.control)?;

        // A second handle for the capture thread to issue retune/gain commands.
        let mut cmd_sock = sock
            .try_clone()
            .map_err(|e| AudioError::Io(e.to_string()))?;
        // A third handle for the stop hook: dropping the CaptureHandle must
        // unblock a thread parked in `sock.read()`. Setting the flag alone is not
        // enough — a still-open but silent server would leave the read parked
        // indefinitely — so the hook also shuts the socket down.
        let shutdown_sock = sock
            .try_clone()
            .map_err(|e| AudioError::Io(e.to_string()))?;

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
                // The dongle rate is authoritative from the control cell so
                // `ConfigureSdr` can change it live; it starts at the seeded value.
                let mut cur_rate = control.capture_rate();
                let mut rx_chain = NbfmReceiver::new(
                    cur_rate,
                    channel_rate,
                    control.offset_hz(),
                    deviation_hz,
                    control.effective_squelch(),
                );
                let mut stft = ComplexStft::new(WATERFALL_NFFT, WATERFALL_NFFT);
                let mut seen_gen = control.generation();
                // Read buffer sized for ~one waterfall frame of IQ (2 bytes/sample).
                let mut buf = vec![0u8; WATERFALL_NFFT * 2];
                // Carry a split IQ pair across TCP read boundaries.
                let mut carry: Option<u8> = None;

                while !stop_thread.load(Ordering::Relaxed) {
                    let n = match sock.read(&mut buf) {
                        Ok(0) => break, // server closed
                        Ok(n) => n,
                        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
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
                        // A capture-rate change rebuilds the whole RX chain (the
                        // decimation ratio and NCO base rate both depend on it) and
                        // re-commands the dongle's sample rate.
                        let want_rate = control.capture_rate();
                        if want_rate != cur_rate && want_rate != 0 {
                            cur_rate = want_rate;
                            rx_chain = NbfmReceiver::new(
                                cur_rate,
                                channel_rate,
                                control.offset_hz(),
                                deviation_hz,
                                control.effective_squelch(),
                            );
                            let _ = cmd_sock.write_all(&RtlCmd::SampleRate(cur_rate).encode());
                        } else {
                            rx_chain.retune(control.offset_hz());
                            rx_chain.set_squelch(control.effective_squelch());
                        }
                        let _ = apply_hardware(&mut cmd_sock, &control);
                    }

                    if let Some(tele) = telemetry.as_ref() {
                        emit_waterfall(
                            &mut stft, &iq, cur_rate, control.center_hz(), channel, tele,
                        );
                    }

                    let audio = rx_chain.push_iq(&iq);
                    if audio.is_empty() {
                        continue;
                    }
                    let chunk: AudioChunk = audio
                        .iter()
                        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                        .collect();
                    if tx.send(chunk).is_err() {
                        break; // consumer dropped
                    }
                }
            })
            .map_err(|e| AudioError::Io(e.to_string()))?;

        let stop_on_drop = stop;
        Ok(CaptureHandle::new(rx, channel_rate, move || {
            stop_on_drop.store(true, Ordering::Relaxed);
            // Unblock a read parked on a silent-but-open server; a genuine EOF
            // (Ok(0)) also breaks the loop, so this is belt-and-suspenders.
            let _ = shutdown_sock.shutdown(std::net::Shutdown::Both);
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
    fn caps_default_none_then_visible_through_clone() {
        let c = SdrControl::default();
        let worker = c.clone();
        assert!(c.caps().is_none());
        c.set_caps(caps_from_header(&RtlHeader { tuner_type: 5, tuner_gain_count: 29 }));
        let caps = worker.caps().expect("caps published");
        assert_eq!(caps.tuner, "R820T");
        assert_eq!(caps.gains_db.len(), 29);
        assert!(!caps.bias_tee_supported);
        assert!(caps.sample_rates.contains(&DEFAULT_CAPTURE_RATE));
        assert!(caps.freq_min_hz < caps.freq_max_hz);
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

        let mut total = 0usize;
        loop {
            match cap.rx.recv_timeout(Duration::from_secs(2)) {
                Ok(chunk) => total += chunk.len(),
                Err(RecvTimeoutError::Timeout) => panic!("capture stalled"),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(total > 0, "expected demodulated audio samples, got none");
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
    fn playback_is_unsupported() {
        let backend = RtlTcpBackend::new("127.0.0.1", 1234);
        assert!(matches!(backend.open_playback(48_000), Err(AudioError::Unsupported)));
        assert_eq!(backend.device_id(), DeviceId::RtlTcp { host: "127.0.0.1".into(), port: 1234 });
    }
}
