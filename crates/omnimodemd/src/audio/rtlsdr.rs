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
use std::sync::Arc;
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
    fn from_u8(v: u8) -> DemodMode {
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

        // Read + validate the 12-byte greeting before anything else.
        let mut header = [0u8; 12];
        sock.read_exact(&mut header).map_err(|e| AudioError::Io(e.to_string()))?;
        parse_header(&header)?;

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
        let capture_rate = self.capture_rate;
        let deviation_hz = self.deviation_hz;

        std::thread::Builder::new()
            .name("omni-rtl-capture".into())
            .spawn(move || {
                let mut rx_chain = NbfmReceiver::new(
                    capture_rate,
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
                        rx_chain.retune(control.offset_hz());
                        rx_chain.set_squelch(control.effective_squelch());
                        let _ = apply_hardware(&mut cmd_sock, &control);
                    }

                    if let Some(tele) = telemetry.as_ref() {
                        emit_waterfall(
                            &mut stft, &iq, capture_rate, control.center_hz(), channel, tele,
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
