//! The `rtl_tcp` transport: connect to an `rtl_tcp` server (local or remote), tune
//! the dongle, and read raw u8 IQ over a socket, behind the internal
//! [`SdrTransport`] seam so the shared DSP capture [`pipeline`](super::pipeline)
//! drives it unchanged.
//!
//! Wire protocol (`librtlsdr` `rtl_tcp.c`): on connect the server writes a
//! 12-byte header (magic `RTL0` + tuner type + gain count), then streams
//! interleaved unsigned-8-bit IQ. The client sends 5-byte commands back on the
//! same socket to tune/gain/correct the dongle. A dropped link is recovered
//! transparently inside [`RtlTcpTransport::read_iq`] (reconnect + re-apply from
//! [`SdrControl`], the single source of truth), so the pipeline sees one
//! continuous stream.

use super::pipeline;
use super::{
    bias_tee_supported, direct_sampling_supported, supported_sample_rates, tuner_freq_range,
    tuner_gains_db, tuner_name, DemodMode, SdrControl, SdrTransport, TunerCaps, ADSB_CAPTURE_RATE,
    DEFAULT_CAPTURE_RATE, DEFAULT_DEVIATION_HZ,
};
use crate::audio::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use crate::audio::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH, MAX_SAMPLE_RATE};
use crate::core::event::TelemetryEvent;
use crate::ids::{ChannelId, DeviceId};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

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

/// Send the dongle its hardware parameters from the current control snapshot:
/// (optionally) sample rate, then ppm, direct-sampling, bias-tee, gain mode/level,
/// and center frequency (order mirrors `rtl_tcp` clients). Used both at connect and
/// on every runtime control change, so a re-established or re-tuned link always
/// matches `SdrControl` — the single source of truth that survives a dropped
/// connection. `send_rate` gates the `SET_SAMPLE_RATE` (0x02) command: it is sent at
/// connect and only when the rate actually changed, never on an unrelated tune/gain
/// change, because reprogramming the resampler mid-stream needlessly glitches audio.
fn send_hardware(
    sock: &mut TcpStream,
    control: &SdrControl,
    send_rate: bool,
) -> Result<(), AudioError> {
    let io = |e: std::io::Error| AudioError::Io(e.to_string());
    if send_rate {
        sock.write_all(&RtlCmd::SampleRate(control.capture_rate()).encode()).map_err(io)?;
    }
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

/// Connect to `addr`, read and validate the 12-byte greeting, and send the dongle
/// its parameters from the current control snapshot. Returns the read socket, a
/// cloned command socket, and the tuner capabilities the header reveals. Reused for
/// both the initial connect and every reconnect.
fn connect_and_handshake(
    addr: &str,
    control: &SdrControl,
) -> Result<(TcpStream, TcpStream, TunerCaps), AudioError> {
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
    let caps = caps_from_header(&hdr);
    // A fresh connection always (re)programs the sample rate, matching the
    // pre-refactor `send_initial_commands`.
    send_hardware(&mut sock, control, true)?;
    let cmd_sock = sock.try_clone().map_err(|e| AudioError::Io(e.to_string()))?;
    Ok((sock, cmd_sock, caps))
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

/// Merge a carried odd byte (a split IQ pair left over from the previous read)
/// with a freshly read chunk into a whole-pair byte buffer, returning the paired
/// bytes and any new leftover byte to carry into the next read. The transport owns
/// this so a hidden reconnect can drop a stale carry (a fresh stream is re-aligned
/// from byte 0) and the pipeline only ever sees whole IQ pairs.
fn merge_iq_bytes(carry: Option<u8>, chunk: &[u8]) -> (Vec<u8>, Option<u8>) {
    let mut bytes = Vec::with_capacity(chunk.len() + 1);
    if let Some(c) = carry {
        bytes.push(c);
    }
    bytes.extend_from_slice(chunk);
    let next = if bytes.len() % 2 == 1 { bytes.pop() } else { None };
    (bytes, next)
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// The `rtl_tcp` [`SdrTransport`]: owns the live sockets, hides reconnect/backoff,
/// and re-applies hardware from [`SdrControl`] on every reconnect. A transient link
/// loss is recovered inside [`read_iq`](SdrTransport::read_iq) so the pipeline sees
/// one continuous IQ stream; only a signalled stop ends it.
struct RtlTcpTransport {
    addr: String,
    /// Read socket for the live connection (swapped on reconnect).
    sock: TcpStream,
    /// Command socket (a clone of `sock`) the pipeline writes tune/gain to.
    cmd_sock: TcpStream,
    /// Shared control snapshot, re-applied to the dongle on every reconnect.
    control: SdrControl,
    /// Capabilities from the last handshake's header.
    caps: TunerCaps,
    /// The live socket, published for the stop hook to shut down. Refreshed on
    /// every reconnect.
    shutdown_slot: Arc<Mutex<Option<TcpStream>>>,
    /// Set by the stop hook; ends the reconnect loop and the streaming read.
    stop: Arc<AtomicBool>,
    /// Reconnect-backoff index, advanced per failed attempt, reset after a stable
    /// connection.
    backoff: usize,
    /// When the current connection was established, for the backoff reset.
    connected_at: Instant,
    /// Odd IQ byte held back from the last read to pair with the next one. Reset to
    /// `None` on every (re)connect so a fresh, byte-0-aligned stream never inherits a
    /// stale half-pair (which would swap I/Q for the whole new connection).
    carry: Option<u8>,
    /// Reusable socket read buffer, sized to the caller's `buf` on first read.
    scratch: Vec<u8>,
    /// Last sample rate commanded to the dongle. `apply_hardware` re-sends
    /// `SET_SAMPLE_RATE` only when this changes, so a routine tune/gain change does
    /// not needlessly reprogram the resampler.
    last_rate: Option<u32>,
}

impl RtlTcpTransport {
    /// Establish the first connection synchronously (so a bad address / non-rtl_tcp
    /// server fails fast) and capture the handshake's caps.
    fn connect(addr: String, control: SdrControl) -> Result<Self, AudioError> {
        let (sock, cmd_sock, caps) = connect_and_handshake(&addr, &control)?;
        let shutdown_slot = Arc::new(Mutex::new(sock.try_clone().ok()));
        // The handshake already programmed the sample rate.
        let last_rate = Some(control.capture_rate());
        Ok(RtlTcpTransport {
            addr,
            sock,
            cmd_sock,
            control,
            caps,
            shutdown_slot,
            stop: Arc::new(AtomicBool::new(false)),
            backoff: 0,
            connected_at: Instant::now(),
            carry: None,
            scratch: Vec::new(),
            last_rate,
        })
    }

    /// Reconnect after a dropped link: reset the backoff if the prior connection
    /// streamed stably, then back off (honoring `stop`) and re-handshake until it
    /// succeeds. Publishes the new live socket for the stop hook. Returns `false`
    /// once `stop` is observed, so the caller terminates instead of retrying.
    fn reconnect(&mut self) -> bool {
        if self.connected_at.elapsed() >= BACKOFF_RESET_AFTER {
            self.backoff = 0;
        }
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            backoff_wait(&mut self.backoff, &self.stop);
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            match connect_and_handshake(&self.addr, &self.control) {
                Ok((sock, cmd_sock, caps)) => {
                    *self.shutdown_slot.lock().unwrap() = sock.try_clone().ok();
                    self.sock = sock;
                    self.cmd_sock = cmd_sock;
                    // Republish caps (matches the pre-refactor per-connect publish)
                    // and reset the stream-boundary state for the fresh connection:
                    // the handshake re-programmed the rate, and the new stream is
                    // aligned from byte 0 so any half-pair carry must be dropped.
                    self.control.set_caps(caps.clone());
                    self.caps = caps;
                    self.last_rate = Some(self.control.capture_rate());
                    self.carry = None;
                    self.connected_at = Instant::now();
                    return true;
                }
                Err(e) => {
                    tracing::warn!(addr = %self.addr, error = %e, "rtl_tcp reconnect failed");
                }
            }
        }
    }
}

impl SdrTransport for RtlTcpTransport {
    fn read_iq(&mut self, buf: &mut [u8]) -> Result<usize, AudioError> {
        if self.scratch.len() != buf.len() {
            self.scratch.resize(buf.len(), 0);
        }
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(0);
            }
            match self.sock.read(&mut self.scratch[..buf.len()]) {
                Ok(0) => {} // server closed — reconnect
                Ok(n) => {
                    // Hand the pipeline only whole IQ pairs, carrying a split pair
                    // across this read boundary. A single leftover byte (empty even
                    // result) is not a terminal stop, so read more instead of
                    // returning 0.
                    let (bytes, carry) = merge_iq_bytes(self.carry.take(), &self.scratch[..n]);
                    self.carry = carry;
                    if bytes.is_empty() {
                        continue;
                    }
                    buf[..bytes.len()].copy_from_slice(&bytes);
                    return Ok(bytes.len());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {} // read error — reconnect
            }
            // The link dropped. Recover it transparently unless we're stopping, so
            // the pipeline never sees the seam.
            if self.stop.load(Ordering::Relaxed) {
                return Ok(0);
            }
            tracing::warn!(addr = %self.addr, "rtl_tcp link dropped; reconnecting");
            if !self.reconnect() {
                return Ok(0);
            }
        }
    }

    fn apply_hardware(&mut self, control: &SdrControl) -> Result<(), AudioError> {
        // Only reprogram the sample rate when it actually changed (the resampler is
        // disruptive to reprogram); a routine tune/gain/squelch change must not.
        let rate = control.capture_rate();
        let send_rate = self.last_rate != Some(rate);
        send_hardware(&mut self.cmd_sock, control, send_rate)?;
        if send_rate {
            self.last_rate = Some(rate);
        }
        Ok(())
    }

    fn caps(&self) -> TunerCaps {
        self.caps.clone()
    }

    fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send> {
        let stop = self.stop.clone();
        let slot = self.shutdown_slot.clone();
        Box::new(move || {
            stop.store(true, Ordering::Relaxed);
            // Unblock a read parked on a silent-but-open server by shutting down
            // whichever connection is currently live; a genuine EOF (Ok(0)) also
            // breaks the loop, so this is belt-and-suspenders.
            if let Some(s) = slot.lock().unwrap().as_ref() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        })
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

        // Initial connect is synchronous so a bad address / non-rtl_tcp server fails
        // fast and the tuner caps publish before we return. The transport then hides
        // every later reconnect from the pipeline.
        let transport = RtlTcpTransport::connect(addr, self.control.clone())?;
        self.control.set_caps(transport.caps());
        let shutdown = transport.shutdown_handle();

        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let control = self.control.clone();
        let telemetry = self.telemetry.clone();
        let channel = self.channel;
        let deviation_hz = self.deviation_hz;

        std::thread::Builder::new()
            .name("omni-rtl-capture".into())
            .spawn(move || {
                pipeline::run_capture(
                    transport,
                    control,
                    telemetry,
                    channel,
                    deviation_hz,
                    channel_rate,
                    tx,
                );
            })
            .map_err(|e| AudioError::Io(e.to_string()))?;

        Ok(CaptureHandle::new(rx, channel_rate, shutdown))
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
    use crate::audio::sdr::pipeline::INV_SQRT2;
    use std::net::TcpListener;
    use std::sync::mpsc::RecvTimeoutError;

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
    fn read_iq_drops_half_pair_across_reconnect() {
        // Regression: a link drop while an odd byte is carried must NOT shift the
        // fresh stream. The transport drops the stale half-pair so I/Q stays aligned
        // from byte 0 of the new connection; a retained carry would swap I/Q for the
        // whole reconnected stream (spectrum mirrored, FM discriminator inverted).
        let iq0 = vec![1u8, 2, 3]; // odd → byte 3 is carried at the drop
        let iq1 = vec![10u8, 20, 30, 40, 50, 60, 70, 80];
        let (port, _cmds2, stop) = spawn_reconnecting_server(iq0, iq1);
        let mut transport =
            RtlTcpTransport::connect(format!("127.0.0.1:{port}"), SdrControl::default()).unwrap();

        let mut got = Vec::new();
        let mut buf = [0u8; 8];
        for _ in 0..20 {
            match transport.read_iq(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
            if got.len() >= 4 {
                break;
            }
        }
        stop.store(true, Ordering::Relaxed);
        // iq0 delivered [1,2] (3 carried, then dropped at reconnect); iq1 arrives
        // aligned as [10,20,…]. A leaked carry would have produced [1,2,3,10,…].
        assert!(got.len() >= 4, "transport delivered too few bytes: {got:?}");
        assert_eq!(&got[..4], &[1, 2, 10, 20], "half-pair carry leaked across reconnect");
    }

    #[test]
    fn control_change_does_not_resend_sample_rate() {
        // Regression: a non-rate change (bias-tee) must reprogram only that field —
        // never re-send SET_SAMPLE_RATE (0x02), which would reset the resampler and
        // glitch audio on every routine tune/gain toggle.
        let (port, cmds, stop) = spawn_recording_server();
        let backend = RtlTcpBackend::new("127.0.0.1", port);
        let control = backend.control();
        let cap = backend.open_capture(48_000).unwrap();

        let count_rate =
            || cmds.lock().unwrap().chunks_exact(5).filter(|f| f[0] == 0x02).count();
        let saw_bias_on = || {
            cmds.lock().unwrap().chunks_exact(5).any(|f| {
                f[0] == 0x0e && u32::from_be_bytes([f[1], f[2], f[3], f[4]]) == 1
            })
        };

        // Exactly one connect-time SampleRate, then audio flowing.
        let mut initial = false;
        for _ in 0..200 {
            if count_rate() >= 1 {
                initial = true;
                break;
            }
            let _ = cap.rx.recv_timeout(Duration::from_millis(10));
        }
        assert!(initial, "connect-time SampleRate not observed");
        assert!(
            cap.rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "no audio before the control change"
        );
        let rate_frames_before = count_rate();

        // A pure bias-tee change must not touch the sample rate.
        control.set_bias_tee(true);
        let mut bias_ok = false;
        for _ in 0..300 {
            if saw_bias_on() {
                bias_ok = true;
                break;
            }
            let _ = cap.rx.recv_timeout(Duration::from_millis(10));
        }
        let rate_frames_after = count_rate();
        stop.store(true, Ordering::Relaxed);
        drop(cap);
        assert!(bias_ok, "BiasTee(on) not applied");
        assert_eq!(
            rate_frames_after, rate_frames_before,
            "a non-rate control change re-sent SampleRate ({rate_frames_before} -> {rate_frames_after})"
        );
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
