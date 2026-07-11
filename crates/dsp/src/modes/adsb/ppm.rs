//! ADS-B (Mode S) pulse-position modulator/demodulator on a magnitude stream.
//!
//! Mode S extended squitter is 1 Mbit/s PPM at 1090 MHz: an 8 µs preamble
//! (pulses at 0.0, 1.0, 3.5, 4.5 µs) followed by 56- or 112-bit frames, each
//! data bit a 0.5 µs pulse in the first (`1`) or second (`0`) half of its
//! microsecond. Both sides work on a magnitude (envelope) stream — the value a
//! receiver recovers as `|I + jQ|` — so the modulator output feeds straight
//! back into the demodulator for loopback self-test.
//!
//! Samples are `f32` at `samples_per_us` samples per microsecond (even; 2 is
//! the dump1090 rate, i.e. omnimodem's 2 MHz `native_rate`). Slicing and
//! preamble correlation are scale-independent (relative comparisons), so the
//! magnitude need not be normalized.

use super::roster::{self, IcaoRoster};
use super::{crc, long_frame_df, ADSB_MIN_CONFIDENCE, DATA_SLOTS_PER_BIT, LONG_FRAME_BITS};
use super::{PREAMBLE_HIGH_SLOTS, PREAMBLE_QUIET_SLOTS, PREAMBLE_SLOTS, SHORT_FRAME_BITS};
use crate::frontend::detector::EnvelopeDetector;

// Noise-floor-relative preamble-detector parameters. Tuned on the KSLC 2.4 Msps
// reference recording (complex front end): the strict correlator these replace
// yielded 12 CRC-valid frames, this detector yields 16 (+33%). See GRA-325.
//
/// Envelope-follower attack/decay for the adaptive noise floor. Fast attack, slow
/// decay so the envelope rides pulse energy; the floor slews toward it only while
/// the squelch is closed, so it settles on the quiescent 1090 MHz noise between
/// bursts and freezes through one.
const DET_ATTACK: f32 = 0.4;
const DET_DECAY: f32 = 0.05;
/// Floor slew rate — deliberately slow so a weak burst that never trips the
/// squelch cannot drag the noise estimate up over its ~112 µs, while still
/// tracking receiver-gain drift across a recording.
const DET_FLOOR_COEFF: f32 = 0.001;
/// The detector's own squelch ratio: the floor freezes once the envelope clears
/// this multiple of it. Governs the floor estimate, not preamble acceptance.
const DET_SQUELCH_RATIO: f32 = 2.0;
/// Noise-floor-relative preamble gate: the mean pulse level must clear the floor
/// by this ratio. Real frames on the reference recording sit at ≥3× the floor, so
/// this leaves headroom while rejecting noise — the check the strict correlator
/// lacked.
const DET_GATE_RATIO: f32 = 2.0;
/// Guard-slot ceiling as a fraction of the mean pulse level. Above the strict
/// dump1090 `2/3` because the resampled envelope leaks pulse energy into the guard
/// slots; the check still rejects positions where a guard slot dominates the
/// pulses. Kept below the point where it stops discriminating.
const QUIET_CEIL_RATIO: f32 = 1.5;

/// A fresh adaptive noise-floor follower configured with this module's tuned
/// constants. The streaming demodulator keeps one running across the whole
/// stream (never restarted per buffer) so the floor stays continuous; the
/// one-shot [`PpmDemodulator::scan`] spins up its own over the given buffer.
pub(super) fn new_floor_detector() -> EnvelopeDetector {
    EnvelopeDetector::new(DET_ATTACK, DET_DECAY, DET_FLOOR_COEFF, DET_SQUELCH_RATIO)
}

/// A demodulated Mode S frame with its position in the fed stream.
#[derive(Clone, Debug, PartialEq)]
pub struct RawFrame {
    /// Raw message bytes (7 or 14).
    pub bytes: Vec<u8>,
    /// Downlink format (top 5 bits of byte 0).
    pub df: u8,
    /// CRC residual — `0` for a clean extended-squitter frame.
    pub crc_residual: u32,
    /// Sample offset of the preamble start within the scanned buffer.
    pub offset: usize,
    /// Soft-decision confidence in `[0, 1]`: the mean per-bit eye (matched-filter
    /// pulse metric normalized by a decision-feedback AGC) the R4 gate accepts on.
    /// See [`PpmDemodulator::soft_confidence`]. In the ensemble this is the eye of
    /// the earliest-offset surviving phase (the one [`DedupWindow`] keeps), not
    /// necessarily the highest-scoring phase — every phase's copy already cleared
    /// the gate, so it is a lower bound on the frame's best eye, not the max.
    pub confidence: f32,
    /// ICAO address recovered by the R5 roster gate for an address-overlaid DF
    /// (DF0/4/5/16/20/21), whose parity is XOR-folded with the address so
    /// [`crc_residual`](Self::crc_residual) equals it. `None` for a frame that
    /// carries its address in the clear or checksums to zero. A frame is *valid*
    /// (see [`Self::valid`]) when it checksums clean **or** its address was
    /// roster-confirmed here.
    pub recovered_icao: Option<u32>,
}

impl RawFrame {
    /// A frame whose parity checksummed to zero (a clean or single-bit-repaired
    /// frame). Address-overlaid frames recovered by the roster gate are *valid*
    /// but not `crc_ok` — their residual is the address, not zero.
    pub fn crc_ok(&self) -> bool {
        self.crc_residual == 0
    }

    /// A frame accepted as genuine: it checksummed clean (`crc_ok`) or an
    /// address-overlaid frame's address was confirmed against the ICAO roster.
    pub fn valid(&self) -> bool {
        self.crc_ok() || self.recovered_icao.is_some()
    }
}

/// Builds Mode S PPM magnitude waveforms.
#[derive(Clone, Copy, Debug)]
pub struct PpmModulator {
    samples_per_us: usize,
    high: f32,
    low: f32,
}

impl PpmModulator {
    /// Modulator at `samples_per_us` (must be even) with unit-amplitude pulses.
    pub fn new(samples_per_us: usize) -> Self {
        assert!(
            samples_per_us >= 2 && samples_per_us.is_multiple_of(2),
            "samples_per_us must be even and >= 2"
        );
        Self { samples_per_us, high: 1.0, low: 0.0 }
    }

    fn slot_len(&self) -> usize {
        self.samples_per_us / 2
    }

    fn push_slot(&self, out: &mut Vec<f32>, high: bool) {
        let v = if high { self.high } else { self.low };
        for _ in 0..self.slot_len() {
            out.push(v);
        }
    }

    /// Modulate a raw Mode S frame (7 or 14 bytes, parity already present) into
    /// a magnitude waveform: preamble + PPM-encoded bits.
    pub fn modulate(&self, frame: &[u8]) -> Vec<f32> {
        let nbits = frame.len() * 8;
        let total_slots = PREAMBLE_SLOTS + nbits * DATA_SLOTS_PER_BIT;
        let mut out = Vec::with_capacity(total_slots * self.slot_len());

        for slot in 0..PREAMBLE_SLOTS {
            self.push_slot(&mut out, PREAMBLE_HIGH_SLOTS.contains(&slot));
        }
        for &byte in frame {
            for bit in (0..8).rev() {
                let one = (byte >> bit) & 1 == 1;
                self.push_slot(&mut out, one);
                self.push_slot(&mut out, !one);
            }
        }
        out
    }

    /// Modulate with `lead`/`trail` microseconds of silence so a demodulator
    /// scanning a longer buffer has quiet margins to lock onto.
    pub fn modulate_padded(&self, frame: &[u8], lead_us: usize, trail_us: usize) -> Vec<f32> {
        let mut out = vec![self.low; lead_us * self.samples_per_us];
        out.extend(self.modulate(frame));
        out.extend(std::iter::repeat_n(self.low, trail_us * self.samples_per_us));
        out
    }
}

/// Decodes Mode S PPM from a magnitude stream.
#[derive(Clone, Copy, Debug)]
pub struct PpmDemodulator {
    samples_per_us: usize,
    /// Accept only frames whose extended-squitter CRC is zero. When false,
    /// frames are returned regardless of residual (DF0/4/5/11/20/21 overlay
    /// their parity with an address).
    pub require_crc: bool,
    /// Soft-decision accept/reject gate (R4): a CRC-valid candidate is dropped
    /// unless its mean per-bit eye ([`Self::soft_confidence`]) clears this. `0.0`
    /// disables the gate — every parity-clean frame is kept. See
    /// [`ADSB_MIN_CONFIDENCE`](super::ADSB_MIN_CONFIDENCE).
    pub min_confidence: f32,
    /// R5 single-bit CRC repair: when set, a parity-failing candidate is retried
    /// after correcting a unique single-bit error (see [`crc::locate_single_bit_error`]).
    /// Off by default — the confidence gate still applies to the repaired frame.
    pub repair: bool,
}

impl PpmDemodulator {
    pub fn new(samples_per_us: usize) -> Self {
        assert!(
            samples_per_us >= 2 && samples_per_us.is_multiple_of(2),
            "samples_per_us must be even and >= 2"
        );
        Self { samples_per_us, require_crc: true, min_confidence: ADSB_MIN_CONFIDENCE, repair: false }
    }

    fn slot_len(&self) -> usize {
        self.samples_per_us / 2
    }

    /// Mean magnitude over the half-microsecond slot beginning at `sample`.
    fn slot_mag(&self, mag: &[f32], sample: usize) -> f32 {
        let len = self.slot_len();
        let mut sum = 0.0f32;
        for k in 0..len {
            sum += mag[sample + k];
        }
        sum / len as f32
    }

    /// Preamble match at offset `i`, dump1090-style and relative to the adaptive
    /// noise floor `noise` (the quiescent-noise envelope entering slot `i`).
    ///
    /// Three gates, replacing the old strict "weakest pulse beats strongest
    /// quiet slot" correlation that a single noisy guard slot could veto:
    ///   1. Each of the four pulses must individually clear the noise floor, so
    ///      a lone spike can't fake the whole preamble through the mean.
    ///   2. The mean pulse level must exceed the floor by [`DET_GATE_RATIO`] —
    ///      the noise-floor-relative presence gate.
    ///   3. The guard slots ([`PREAMBLE_QUIET_SLOTS`], the non-pulse-adjacent
    ///      ones) must sit below [`QUIET_CEIL_RATIO`]× the mean pulse level —
    ///      dump1090's tolerant ceiling, relaxed for the resampled envelope.
    fn preamble_ok(&self, mag: &[f32], noise: f32, i: usize) -> bool {
        let slot = self.slot_len();
        let mut high_sum = 0.0f32;
        for &s in &PREAMBLE_HIGH_SLOTS {
            let p = self.slot_mag(mag, i + s * slot);
            if p <= noise {
                return false;
            }
            high_sum += p;
        }
        let high_mean = high_sum / PREAMBLE_HIGH_SLOTS.len() as f32;
        if high_mean <= noise * DET_GATE_RATIO {
            return false;
        }
        let ceil = high_mean * QUIET_CEIL_RATIO;
        for &s in &PREAMBLE_QUIET_SLOTS {
            if self.slot_mag(mag, i + s * slot) >= ceil {
                return false;
            }
        }
        true
    }

    /// Slice `nbits` PPM data bits following the preamble at offset `i`.
    fn slice_bits(&self, mag: &[f32], i: usize, nbits: usize) -> Vec<u8> {
        let slot = self.slot_len();
        let data_start = i + PREAMBLE_SLOTS * slot;
        let mut bytes = vec![0u8; nbits / 8];
        for j in 0..nbits {
            let base = data_start + j * DATA_SLOTS_PER_BIT * slot;
            let first = self.slot_mag(mag, base);
            let second = self.slot_mag(mag, base + slot);
            if first > second {
                bytes[j / 8] |= 1 << (7 - (j % 8));
            }
        }
        bytes
    }

    /// Mean per-bit **eye**: the soft-decision confidence the R4 gate accepts or
    /// rejects on. Each PPM bit is a 0.5 µs pulse in one of its two half-µs slots,
    /// so `pulse - trough` (the matched filter for a ±pulse bit template) measures
    /// how cleanly the bit resolves. A decision-feedback AGC tracks the running
    /// pulse amplitude and normalizes each bit against it, so the score is
    /// amplitude-independent and follows a fading carrier through the frame; the
    /// per-bit eye is `(pulse - trough) / agc` clamped to `[0, 1]`, and the frame
    /// confidence is their mean.
    ///
    /// A genuine transmission drives one slot to the pulse level and leaves the
    /// other near noise, so the eye is wide; a CRC-lucky ghost that an
    /// over-interpolated ensemble phase slices out of noise has near-equal slots
    /// and a mushy eye. On the KSLC reference the two classes separate cleanly —
    /// real frames ≥ 0.39, the lone ghost at 0.26 — which is what makes the gate
    /// safe (see [`ADSB_MIN_CONFIDENCE`](super::ADSB_MIN_CONFIDENCE)).
    ///
    /// The empty slot carries the noise floor, not zero, so with the AGC tracking
    /// the pulse level the eye is effectively `1 - noise/signal` — an SNR measure.
    /// That is deliberate: it is exactly what separates a clean frame from a
    /// noise-sliced ghost, and it is why the metric is *not* noise-subtracted
    /// (cancelling the floor out of both slots would leave the numerator
    /// unchanged; normalizing it out of the denominator would flatten every real
    /// frame to ~1 and erase the discrimination). The trade-off is that a genuine
    /// but very low-SNR frame scores low and can be gated — acceptable because the
    /// threshold sits well under the weakest real frame observed (0.39), but the
    /// reason the gate rejects weak frames when it does.
    fn soft_confidence(&self, mag: &[f32], i: usize, nbits: usize) -> f32 {
        let slot = self.slot_len();
        let data_start = i + PREAMBLE_SLOTS * slot;
        // DFB-AGC seeded from the first bit's pulse; slews at 0.1/bit so it rides
        // a fading amplitude without chasing a single strong bit.
        let mut agc = 0.0f32;
        let mut sum = 0.0f32;
        for j in 0..nbits {
            let base = data_start + j * DATA_SLOTS_PER_BIT * slot;
            let first = self.slot_mag(mag, base);
            let second = self.slot_mag(mag, base + slot);
            let pulse = first.max(second);
            let trough = first.min(second);
            if agc == 0.0 {
                agc = pulse.max(f32::MIN_POSITIVE);
            }
            sum += ((pulse - trough) / agc).clamp(0.0, 1.0);
            agc += (pulse - agc) * 0.1;
        }
        sum / nbits as f32
    }

    /// Samples spanned by a full frame (preamble + `nbits` data).
    fn frame_samples(&self, nbits: usize) -> usize {
        (PREAMBLE_SLOTS + nbits * DATA_SLOTS_PER_BIT) * self.slot_len()
    }

    /// Scan a self-contained buffer, building a fresh adaptive noise floor over
    /// it. Convenience for one-shot decodes in tests; the streaming path uses
    /// [`scan_with_floor`](Self::scan_with_floor) with a floor that persists
    /// across calls so it never re-warms mid-stream.
    #[cfg(test)]
    pub fn scan(&self, mag: &[f32], flush: bool) -> (Vec<RawFrame>, usize) {
        let mut det = new_floor_detector();
        let mut noise = Vec::with_capacity(mag.len());
        for &m in mag {
            noise.push(det.floor());
            det.push(m);
        }
        self.scan_with_floor(mag, &noise, None, flush)
    }

    /// Scan `mag` for frames against a caller-supplied adaptive noise floor
    /// `noise` (per-sample, the floor entering each sample). Returns the accepted
    /// frames plus the number of samples consumed; `mag[consumed..]` is the
    /// unscanned tail the caller retains for the next call. When `flush` is false
    /// the scan stops one long frame short of the end so a frame straddling the
    /// buffer boundary is deferred rather than truncated.
    pub fn scan_with_floor(
        &self,
        mag: &[f32],
        noise: &[f32],
        roster: Option<&IcaoRoster>,
        flush: bool,
    ) -> (Vec<RawFrame>, usize) {
        let long_span = self.frame_samples(LONG_FRAME_BITS);
        let short_span = self.frame_samples(SHORT_FRAME_BITS);
        let limit = if flush { short_span } else { long_span };
        let mut frames = Vec::new();
        let mut i = 0usize;
        while i + limit <= mag.len() {
            if !self.preamble_ok(mag, noise[i], i) {
                i += 1;
                continue;
            }
            // Read the DF from a short slice, then extend to a long frame when
            // the format demands it (and the buffer allows).
            let short_bytes = self.slice_bits(mag, i, SHORT_FRAME_BITS);
            let df = short_bytes[0] >> 3;
            let nbits = if long_frame_df(df) && i + long_span <= mag.len() {
                LONG_FRAME_BITS
            } else {
                SHORT_FRAME_BITS
            };
            let mut bytes = if nbits == SHORT_FRAME_BITS {
                short_bytes
            } else {
                self.slice_bits(mag, i, LONG_FRAME_BITS)
            };

            let residual = crc::checksum(&bytes);
            // An all-zero message has a zero CRC residual by construction, so a
            // flat stretch (e.g. a DC region before the noise floor has warmed
            // up) would otherwise slice to a spurious "valid" DF0 frame. A real
            // Mode S transmission is never all zeros; reject it.
            let degenerate = bytes.iter().all(|&b| b == 0);
            let Some((residual, recovered_icao)) =
                self.classify(&mut bytes, df, residual, degenerate, roster)
            else {
                i += 1;
                continue;
            };
            // R4 soft-decision gate: an eligible candidate — clean, single-bit
            // repaired, or roster-confirmed — is still rejected if its eye is too
            // mushy to be a real transmission. This is what makes a wide slicer
            // ensemble (and R5's recovery) safe: the extra phases and the repair /
            // roster levers recover more weak frames, and the gate discards the
            // CRC-lucky ghosts they would otherwise admit.
            let confidence = self.soft_confidence(mag, i, nbits);
            if confidence >= self.min_confidence {
                let df = bytes[0] >> 3;
                frames.push(RawFrame {
                    bytes,
                    df,
                    crc_residual: residual,
                    offset: i,
                    confidence,
                    recovered_icao,
                });
                i += self.frame_samples(nbits);
            } else {
                i += 1;
            }
        }
        (frames, i)
    }

    /// Decide whether a sliced candidate is eligible for acceptance, applying the
    /// R5 recovery levers. Returns `Some((residual, recovered_icao))` for an
    /// eligible frame — possibly with `bytes` corrected in place by a single-bit
    /// repair — or `None` to reject it before the confidence gate.
    ///
    /// Eligibility, in order: a clean parity (`residual == 0`); with
    /// [`Self::repair`], a candidate a unique single-bit flip makes clean; with a
    /// `roster`, an address-overlaid DF whose residual (its ICAO address) is on the
    /// roster. A degenerate all-zero slice is never eligible. When
    /// [`Self::require_crc`] is false the parity check is skipped entirely and
    /// every non-degenerate candidate is eligible, so the recovery levers only
    /// widen acceptance in the parity-gated (production) path.
    ///
    /// `df` is the downlink format the caller read *before* any repair — it fixed
    /// the frame length (`nbits`) already sliced. A single-bit repair may land in
    /// the DF field; a flip that changes the short/long class would leave the
    /// repaired frame inconsistent with the samples sliced for it, so such a repair
    /// is rejected rather than trusted (it is far more likely a mis-slice than a
    /// real frame). A flip within the same length class is fine.
    fn classify(
        &self,
        bytes: &mut [u8],
        df: u8,
        residual: u32,
        degenerate: bool,
        roster: Option<&IcaoRoster>,
    ) -> Option<(u32, Option<u32>)> {
        if degenerate {
            return None;
        }
        if !self.require_crc {
            return Some((residual, None));
        }
        if residual == 0 {
            return Some((0, None));
        }
        if self.repair {
            if let Some(p) = crc::locate_single_bit_error(residual, bytes.len() * 8) {
                // The DF is byte 0 bits 0..4, so a flip there (p < 5) can change
                // the format. Compute the repaired DF without mutating, and only
                // apply the flip when it keeps the same short/long length class the
                // slice was cut for — otherwise leave `bytes` pristine for the
                // roster branch and reject.
                let repaired_df = if p < 5 { (bytes[0] ^ (1 << (7 - p))) >> 3 } else { df };
                if long_frame_df(repaired_df) == long_frame_df(df) {
                    bytes[p / 8] ^= 1 << (7 - (p % 8));
                    return Some((0, None));
                }
            }
        }
        if let Some(roster) = roster {
            if roster::is_address_overlaid(df) && roster.contains(residual) {
                return Some((residual, Some(residual)));
            }
        }
        None
    }
}

/// Multi-phase slicer ensemble — the R3 "hydra" (GRA-326).
///
/// At the 2 MHz working rate each PPM half-microsecond slot is a single sample,
/// so [`PpmDemodulator`] can only read slot centers on the integer grid. A frame
/// whose true bit timing lands between samples (the common case off-air, after
/// the 2.4M→2.0M resample) slices cleanly on only some fractional offset; on the
/// integer grid its late bits drift toward the slot boundary and the CRC fails.
///
/// The ensemble runs the same demodulator over `phases` sub-sample views of the
/// stream — phase `k` linearly interpolated to a `k/phases`-sample offset — and
/// unions the decodes, so whichever phase best aligns with each frame recovers
/// it. This is dump1090's finer-timing advantage approximated at 2 MHz, and the
/// same union-of-diverse-decoders mechanism that lets graywolf out-decode
/// Direwolf on AFSK. A [`DedupWindow`] collapses the copies of one frame that
/// several phases decode into a single emission.
#[derive(Clone, Debug)]
pub struct ParallelDemodulator {
    demod: PpmDemodulator,
    phases: usize,
    /// Reused fractional-shift scratch buffer, so the per-phase interpolation
    /// does not allocate on every scan.
    shifted: Vec<f32>,
}

impl ParallelDemodulator {
    /// Ensemble of `phases` sub-sample slicers (`phases >= 1`; `1` is exactly the
    /// single-phase [`PpmDemodulator`]).
    pub fn new(samples_per_us: usize, phases: usize) -> Self {
        assert!(phases >= 1, "phases must be >= 1");
        Self { demod: PpmDemodulator::new(samples_per_us), phases, shifted: Vec::new() }
    }

    /// Override the R4 soft-decision accept/reject threshold every phase applies
    /// (`0.0` disables the gate). The offline `adsb_bench` uses this to sweep the
    /// threshold and measure the accept/reject split.
    pub fn set_min_confidence(&mut self, min_confidence: f32) {
        self.demod.min_confidence = min_confidence;
    }

    /// Enable or disable R5 single-bit CRC repair on every phase (off by default).
    pub fn set_repair(&mut self, repair: bool) {
        self.demod.repair = repair;
    }

    /// Interpolate `mag` to a `frac`-sample fractional offset into `self.shifted`.
    /// The last sample has no successor to interpolate against and is copied; it
    /// falls in the unscanned tail (`scan_with_floor` stops a frame short), so it
    /// is never sliced.
    fn interpolate(&mut self, mag: &[f32], frac: f32) {
        self.shifted.clear();
        self.shifted.reserve(mag.len());
        if mag.is_empty() {
            return;
        }
        for w in mag.windows(2) {
            self.shifted.push(w[0] + frac * (w[1] - w[0]));
        }
        self.shifted.push(mag[mag.len() - 1]);
    }

    /// Scan every phase against the shared adaptive noise floor and return the
    /// deduplicated union. The noise floor is deliberately not re-interpolated
    /// per phase: it slews at [`DET_FLOOR_COEFF`] (0.001/sample), so a sub-sample
    /// shift moves it by nothing that matters to the ratio gates.
    ///
    /// `consumed` is the minimum consumed across phases, so no phase's unscanned
    /// tail is discarded; only frames wholly inside that common prefix are
    /// returned, so a frame a faster phase found in the few-sample zone past the
    /// shared boundary is deferred to the next call rather than emitted twice.
    pub fn scan_with_floor(
        &mut self,
        mag: &[f32],
        noise: &[f32],
        roster: Option<&IcaoRoster>,
        flush: bool,
    ) -> (Vec<RawFrame>, usize) {
        let (mut frames, mut consumed) = self.demod.scan_with_floor(mag, noise, roster, flush);
        for k in 1..self.phases {
            let frac = k as f32 / self.phases as f32;
            self.interpolate(mag, frac);
            let (more, ph_consumed) = self.demod.scan_with_floor(&self.shifted, noise, roster, flush);
            consumed = consumed.min(ph_consumed);
            frames.extend(more);
        }
        let window = self.demod.frame_samples(LONG_FRAME_BITS);
        let out = DedupWindow::new(window).collapse(frames, consumed);
        (out, consumed)
    }

    /// One-shot scan over a self-contained buffer with a fresh floor, mirroring
    /// [`PpmDemodulator::scan`] for tests.
    #[cfg(test)]
    pub fn scan(&mut self, mag: &[f32], flush: bool) -> (Vec<RawFrame>, usize) {
        let mut det = new_floor_detector();
        let mut noise = Vec::with_capacity(mag.len());
        for &m in mag {
            noise.push(det.floor());
            det.push(m);
        }
        self.scan_with_floor(mag, &noise, None, flush)
    }
}

/// Collapses duplicate decodes of one physical frame that different slicer phases
/// (or the same phase across a boundary) produce, while preserving genuine
/// retransmissions of identical content — those are milliseconds apart, far
/// beyond the window, whereas cross-phase copies sit within a sample or two.
struct DedupWindow {
    window: usize,
}

impl DedupWindow {
    fn new(window: usize) -> Self {
        Self { window }
    }

    /// Keep the earliest decode of each frame whose start is inside `[0, limit)`,
    /// dropping any later copy with identical bytes within `window` samples.
    fn collapse(&self, mut frames: Vec<RawFrame>, limit: usize) -> Vec<RawFrame> {
        frames.sort_by_key(|f| f.offset);
        let mut kept: Vec<RawFrame> = Vec::with_capacity(frames.len());
        for f in frames {
            if f.offset >= limit {
                continue;
            }
            let dup = kept
                .iter()
                .rev()
                .take_while(|k| f.offset - k.offset <= self.window)
                .any(|k| k.bytes == f.bytes);
            if !dup {
                kept.push(f);
            }
        }
        kept
    }
}
