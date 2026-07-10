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
}
