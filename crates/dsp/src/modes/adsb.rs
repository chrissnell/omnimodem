//! ADS-B (Mode S) mode: 1090 MHz extended squitter demod + loopback modulator.
//!
//! Unlike the audio modes, ADS-B is a wideband SDR mode. It consumes a
//! **magnitude** stream at a 2 MHz `native_rate` — the daemon computes
//! `|I + jQ|` from the rtl_tcp capture and feeds it here, no channelization.
//! `feed` emits each decoded Mode S frame as a `Packet` payload carrying the
//! raw 7/14 message bytes; the daemon decodes those into typed aircraft state
//! (see [`message`]) for the event stream and the TUI flights table.
//!
//! Transmit is loopback/self-test only: 1090 MHz is protected aeronautical
//! spectrum, so the modulator exists to exercise the demodulator end-to-end and
//! to render offline test vectors, never to key a radio.
//!
//! Building blocks live in submodules: [`crc`] (Mode S parity), [`ppm`] (the
//! magnitude PPM mod/demod), [`message`] (field/CPR/altitude/velocity decode +
//! DF17 frame construction), and [`tracker`] (ICAO-keyed per-aircraft state).

mod crc;
mod message;
mod ppm;
mod roster;
mod tracker;

#[cfg(test)]
mod tests;

pub use message::{
    cpr_decode_airborne, encode_all_call_reply, encode_identification, AirbornePosition,
    AirborneVelocity, ModeS, CA_LEVEL2,
};
pub use ppm::RawFrame;
pub use roster::IcaoRoster;
pub use tracker::{Aircraft, AircraftTracker, Ingest};

use crate::frontend::detector::EnvelopeDetector;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

/// ADS-B working rate: 2 Msps (dump1090's 2 MHz convention).
pub const ADSB_RATE: u32 = 2_000_000;
/// Samples per microsecond at [`ADSB_RATE`].
const SAMPLES_PER_US: usize = (ADSB_RATE / 1_000_000) as usize;

/// R5 native-rate working option: 4 Msps. The 2.4 Msps capture cannot be decoded
/// at a working rate of exactly 2.4 MHz — the slicer needs an even integer number
/// of samples per microsecond (a half-µs PPM slot is [`slot_len`] whole samples),
/// and 2.4 is not integer. Downsampling to the 2.0 MHz [`ADSB_RATE`] instead
/// band-limits away the sharp 0.5 µs pulse edges (its anti-alias cutoff sits at
/// the 1.0 MHz Nyquist, below the pulse's spectral content), which costs weak- and
/// short-frame sensitivity — the measured DF11 gap to dump1090, which demodulates
/// at the native 2.4 MHz. Resampling *up* to 4 MHz (the smallest even-integer
/// samples/µs rate above the capture rate) instead **preserves** the full captured
/// bandwidth — its 2.0 MHz Nyquist clears the ±1.2 MHz signal — so the slicer sees
/// the un-smeared pulse, two samples per slot. This is the "or a higher common
/// rate" the R5 plan calls for; the offline `adsb_bench --work-rate 4000000`
/// measures it against the 2.0 MHz default on the reference capture.
pub const ADSB_NATIVE_RATE: u32 = 4_000_000;

/// Samples per microsecond at working rate `rate`, which must be a whole even
/// number of MHz (2, 4, 6, …) so a half-µs PPM slot is a whole number of samples.
pub fn samples_per_us(rate: u32) -> usize {
    assert!(
        rate.is_multiple_of(1_000_000) && (rate / 1_000_000).is_multiple_of(2),
        "ADS-B working rate must be an even whole number of MHz, got {rate}"
    );
    (rate / 1_000_000) as usize
}
/// Microseconds of quiet padding around a modulated frame.
const PAD_US: usize = 8;

/// Short Mode S frame length in bits (DF 0/4/5/11/…).
pub const SHORT_FRAME_BITS: usize = 56;
/// Long (extended squitter) frame length in bits (DF 16/17/18/…).
pub const LONG_FRAME_BITS: usize = 112;

/// Half-microsecond slots in the 8 µs preamble.
pub const PREAMBLE_SLOTS: usize = 16;
/// Half-microsecond slots per data bit (one pulse-position pair).
pub const DATA_SLOTS_PER_BIT: usize = 2;

/// Sub-sample slicer phases the streaming demod runs (R3 multi-phase ensemble,
/// R4 gate). Each phase (0, 1/N, 2/N, … sample) recovers frames whose bit timing
/// lands off the 2 MHz integer grid; see [`ppm::ParallelDemodulator`].
///
/// Chosen by sweeping the KSLC 2.4 Msps reference recording (complex front end).
/// R3 capped this at 4 because 6+ phases began surfacing a CRC-lucky false
/// positive. Its ghost signature is address-independent: a lone DF18 hit (one in
/// the whole recording, where every real aircraft squitters 8–13 times and forms
/// a track) with a reserved control field (CF 7, which no real ADS-B/TIS-B
/// transmitter emits) and the lowest soft eye by a wide margin — and absent from
/// readsb. R4's soft-decision gate ([`ADSB_MIN_CONFIDENCE`]) removes the ceiling:
/// it rejects that low-eye slice on confidence alone, never on its ICAO address,
/// so the ensemble can widen to recover more weak/fading-aircraft frames safely.
/// Real CRC-valid yield vs phase count, ghost gated out:
///   4 → 25 (the R3 baseline)  ·  8 → 29  ·  12 → 31 (+24%)  ·  16 → 31.
/// Yield plateaus at 31 by 12 phases, and the aircraft set stays exactly readsb's
/// three, so 12 captures the gain without paying for 16's redundant phases.
///
/// This is a decode-yield choice, not a latency one: each phase is a full
/// interpolate-and-scan pass, so 12 costs ~3× the R3 baseline's per-buffer slicing
/// in the real-time daemon. 8 phases keeps 29/31 of the gain at 2/3 the cost — the
/// fallback if a future run is CPU-bound rather than yield-bound.
pub const ADSB_SLICER_PHASES: usize = 12;

/// R4 soft-decision accept/reject threshold: a parity-clean candidate frame is
/// kept only if its mean per-bit eye ([`ppm`] `soft_confidence`) clears this. The
/// eye is a matched-filter-plus-DFB-AGC measure of how cleanly each bit's pulse
/// resolves; real transmissions sit well above it, CRC-lucky ghosts below.
///
/// Placed between the two classes on the KSLC reference — the lone ghost scores
/// 0.26, the weakest real frame across all phase counts 0.39 — near their
/// midpoint, which rejects the ghost and every future one like it while leaving
/// ~0.06 of headroom under the weakest genuine frame. As with the R2/R3 constants
/// this is tuned on the single available reference recording; widen the margin if
/// a second capture shows a real frame dipping toward it.
///
/// The eye is SNR-like (see [`ppm`] `soft_confidence`), so the gate is load-bearing:
/// it is what makes the 12-phase ensemble safe, and the cost it trades for that is
/// a genuine very-low-SNR frame that scores under the threshold. The gate reads
/// only the eye, never the address, so a strong signal passes regardless of origin
/// — an international widebody overhead squitters at full strength and clears the
/// gate exactly like nearby traffic. The exposure is purely SNR: a distant or weak
/// aircraft (which an overflight can be) near the threshold. The 0.06 headroom held
/// across 4/8/12/16 phases on the reference, but the daemon discards rejects
/// silently — surfacing near-threshold rejections is the follow-up that would make
/// this caveat observable rather than only documented.
pub const ADSB_MIN_CONFIDENCE: f32 = 0.32;

/// Preamble slots that carry a pulse — pulses at 0.0, 1.0, 3.5, 4.5 µs.
pub const PREAMBLE_HIGH_SLOTS: [usize; 4] = [0, 2, 7, 9];
/// Preamble guard slots the detector tests for quiet: the gap between the two
/// pulse pairs (4, 5) and the gap before the data starts (11–14). The slots
/// immediately adjacent to a pulse (1, 3, 6, 8, 10, 15) are deliberately *not*
/// tested — on a real off-air envelope out-of-phase pulse energy leaks into
/// them, and requiring them quiet is what made the old strict correlator reject
/// valid preambles (dump1090 skips them for the same reason).
pub const PREAMBLE_QUIET_SLOTS: [usize; 6] = [4, 5, 11, 12, 13, 14];

/// True when downlink format `df` denotes a 112-bit (long) frame.
pub fn long_frame_df(df: u8) -> bool {
    matches!(df, 16 | 17 | 18 | 19 | 20 | 21 | 24 | 25 | 26 | 27 | 28 | 29 | 30 | 31)
}

fn payload_kind(p: &FramePayload) -> &'static str {
    match p {
        FramePayload::Packet(_) => "packet",
        FramePayload::Text(_) => "text",
        FramePayload::Message77(_) => "message77",
        FramePayload::Vocoder(_) => "vocoder",
        FramePayload::Image { .. } => "image",
    }
}

/// Streaming ADS-B demodulator. Buffers magnitude samples across `feed` calls so
/// a frame straddling a chunk boundary is not lost.
pub struct AdsbDemod {
    demod: ppm::ParallelDemodulator,
    /// Adaptive noise-floor follower advanced once per fed sample. Persisting it
    /// across `feed` calls keeps the floor continuous, so a frame landing near a
    /// chunk boundary is gated against the true noise level rather than a floor
    /// that was just reset and is still warming up.
    det: EnvelopeDetector,
    buf: Vec<f32>,
    /// Per-sample noise floor entering each sample of `buf` (kept aligned with it).
    floor: Vec<f32>,
    /// Absolute sample index of `buf[0]` in the fed stream.
    base: u64,
    /// R5 roster gate: recently-seen ICAO addresses from clean DF11/17/18 decodes.
    /// Consulted only when [`roster_enabled`](Self::roster_enabled) is set, and
    /// built one buffer behind the frames it validates — a real aircraft squitters
    /// its address in the clear throughout its pass, so a ~1 s lag is immaterial.
    roster: roster::IcaoRoster,
    /// Whether to consult and build the roster (R5 address-overlaid recovery). Off
    /// by default, so the shipping decoder is unchanged.
    roster_enabled: bool,
}

impl AdsbDemod {
    pub fn new() -> Self {
        Self::with_phases(ADSB_SLICER_PHASES)
    }

    /// Construct with an explicit slicer-phase count (`>= 1`). `1` is the
    /// single-phase decoder; the offline `adsb_bench` uses this to sweep phases
    /// and reproduce the pre-R3 baseline. The R4 confidence gate keeps its
    /// [`ADSB_MIN_CONFIDENCE`] default; use [`with_phases_min_conf`] to override.
    pub fn with_phases(phases: usize) -> Self {
        Self::with_phases_min_conf(phases, ADSB_MIN_CONFIDENCE)
    }

    /// Construct with an explicit phase count and soft-decision threshold at the
    /// default [`ADSB_RATE`] working rate. `0.0` disables the R4 gate (accept
    /// every parity-clean frame); the offline `adsb_bench` uses this to measure
    /// the accept/reject split.
    pub fn with_phases_min_conf(phases: usize, min_confidence: f32) -> Self {
        Self::with_rate_phases_min_conf(ADSB_RATE, phases, min_confidence)
    }

    /// Construct at an explicit working `rate` (R5 Lever 1). `rate` must be an
    /// even whole number of MHz (see [`samples_per_us`]); [`ADSB_RATE`] is the
    /// shipping 2 MHz rate, [`ADSB_NATIVE_RATE`] the 4 MHz native-preserving
    /// option. `adsb_bench` resamples the capture to `rate` and builds the demod
    /// here to match, so the slicer runs at the same rate the front end delivers.
    pub fn with_rate_phases_min_conf(rate: u32, phases: usize, min_confidence: f32) -> Self {
        let mut demod = ppm::ParallelDemodulator::new(samples_per_us(rate), phases);
        demod.set_min_confidence(min_confidence);
        AdsbDemod {
            demod,
            det: ppm::new_floor_detector(),
            buf: Vec::new(),
            floor: Vec::new(),
            base: 0,
            roster: roster::IcaoRoster::default(),
            roster_enabled: false,
        }
    }

    /// Enable or disable R5 single-bit CRC repair (Lever 2a). Off by default; the
    /// confidence gate still applies to a repaired frame.
    pub fn set_repair(&mut self, repair: bool) {
        self.demod.set_repair(repair);
    }

    /// Enable or disable the R5 ICAO-roster gate (Lever 2b) — accept an
    /// address-overlaid DF0/4/5/16/20/21 frame only when its recovered address is
    /// on the roster of recently-seen clean DF11/17/18 addresses. Off by default.
    pub fn set_roster(&mut self, enabled: bool) {
        self.roster_enabled = enabled;
    }

    fn emit(&self, raw: ppm::RawFrame) -> Frame {
        // `crc_ok` marks a frame accepted as genuine: it checksummed clean, was
        // single-bit repaired to clean, or is an address-overlaid frame whose
        // address the roster confirmed (see [`RawFrame::valid`]). Recovery is off
        // by default, so with the shipping config this is exactly `residual == 0`.
        let crc_ok = raw.valid();
        Frame {
            payload: FramePayload::Packet(raw.bytes),
            meta: FrameMeta {
                crc_ok,
                sample_offset: self.base + raw.offset as u64,
                decoder: Some("adsb".to_string()),
                confidence: Some(raw.confidence),
                ..Default::default()
            },
        }
    }

    fn drain(&mut self, frames: Vec<ppm::RawFrame>, consumed: usize) -> Vec<Frame> {
        let out: Vec<Frame> = frames.into_iter().map(|r| self.emit(r)).collect();
        self.buf.drain(..consumed);
        self.floor.drain(..consumed);
        self.base += consumed as u64;
        out
    }

    /// Feed the ICAO addresses of clean DF11/17/18 frames into the roster — the
    /// addresses that arrive in the clear and checksum to zero, so they are known
    /// to be real. These are what a later address-overlaid frame is validated
    /// against. Address-overlaid recovered frames are *not* re-noted: their
    /// address was already on the roster (that is why they were accepted).
    fn note_addresses(roster: &mut roster::IcaoRoster, frames: &[ppm::RawFrame]) {
        for f in frames {
            if f.crc_ok() && matches!(f.df, 11 | 17 | 18) {
                roster.note(ModeS::new(&f.bytes).icao());
            }
        }
    }

    /// Run the ensemble over the working buffer, threading the roster when the
    /// R5 gate is enabled and updating it from this pass's clean addresses.
    fn scan(&mut self, flush: bool) -> (Vec<ppm::RawFrame>, usize) {
        let roster = self.roster_enabled.then_some(&self.roster);
        let (frames, consumed) = self.demod.scan_with_floor(&self.buf, &self.floor, roster, flush);
        if self.roster_enabled {
            Self::note_addresses(&mut self.roster, &frames);
        }
        (frames, consumed)
    }

    /// Advance the persistent floor over the newly fed samples, recording the
    /// floor entering each one, then append them to the working buffer.
    fn ingest(&mut self, samples: &[Sample]) {
        self.floor.reserve(samples.len());
        for &s in samples {
            self.floor.push(self.det.floor());
            self.det.push(s);
        }
        self.buf.extend_from_slice(samples);
    }
}

impl Default for AdsbDemod {
    fn default() -> Self {
        Self::new()
    }
}

impl Demodulator for AdsbDemod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: ADSB_RATE,
            bandwidth_hz: 2_000_000.0,
            tx: false,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.ingest(samples);
        let (frames, consumed) = self.scan(false);
        self.drain(frames, consumed)
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.floor.clear();
        self.det = ppm::new_floor_detector();
        self.base = 0;
        self.roster.clear();
    }

    fn flush(&mut self) -> Vec<Frame> {
        let (frames, consumed) = self.scan(true);
        self.drain(frames, consumed)
    }
}

/// Loopback ADS-B modulator: a `Packet` payload (7 or 14 Mode S bytes, parity
/// already present) becomes a magnitude PPM waveform at [`ADSB_RATE`].
pub struct AdsbMod {
    modu: ppm::PpmModulator,
}

impl AdsbMod {
    pub fn new() -> Self {
        AdsbMod { modu: ppm::PpmModulator::new(SAMPLES_PER_US) }
    }
}

impl Default for AdsbMod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for AdsbMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: ADSB_RATE,
            bandwidth_hz: 2_000_000.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let bytes = match &frame.payload {
            FramePayload::Packet(b) => b,
            other => return Err(ModError::UnsupportedPayload(payload_kind(other))),
        };
        if bytes.len() != 7 && bytes.len() != 14 {
            return Err(ModError::Encode(format!(
                "adsb frame must be 7 or 14 bytes, got {}",
                bytes.len()
            )));
        }
        Ok(self.modu.modulate_padded(bytes, PAD_US, PAD_US))
    }
}
