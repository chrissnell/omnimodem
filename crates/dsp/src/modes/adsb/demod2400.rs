//! Native 2.4 Msps correlating Mode S demodulator — R6.
//!
//! The R1–R5 core downsamples the 2.4 Msps capture to a 2.0 MHz working rate,
//! slices each half-µs PPM slot with a 2-tap slot mean, and recovers off-grid
//! timing with a 12-phase interpolation ensemble. readsb/dump1090-fa instead
//! demodulate **natively at 2.4 Msps** with a **matched-filter correlating
//! slicer**: five hand-tuned FIR functions ([`slice_phase0`]..[`slice_phase4`])
//! sample the un-decimated pulse (2.4 samples per bit, 12 samples per 5 bits),
//! a 6:5 phase walk ([`slice_byte`]) reads a whole message byte at a time, and
//! the best-scoring of five preamble phases is kept. Decoding at the capture
//! rate keeps the sharp 0.5 µs pulse edges a 2.0 MHz anti-alias lowpass throws
//! away — the structural advantage behind readsb's weak-/short-frame yield.
//!
//! This is a faithful port of readsb `demod_2400.c` (`slice_phase*`, `slice_byte`,
//! the preamble correlation, `demodulate2400`) with readsb's `scoreModesMessage`
//! acceptance model ([`score_candidate`]): a candidate is accepted on a fused
//! CRC-plus-address-roster score, not CRC alone. That is what makes the
//! address-overlaid DFs (DF0/4/5/16/20/21) and single-bit-corrected frames
//! recoverable **without** admitting the shift-ghosts a CRC-only gate would (the
//! Mode S CRC is shift-invariant, so a 1-bit-misaligned slice of a real frame
//! frequently also checksums clean and lands on an address-overlaid DF; the roster
//! gate rejects it because its recovered "address" was never seen in the clear).
//!
//! It works on an `f32` magnitude (envelope) stream at [`super::ADSB_CAPTURE_RATE`]
//! (2.4 Msps): the same `|I + jQ|` the daemon already computes, but fed at the
//! capture rate with **no resample**. The FIR functions and the preamble
//! correlation are all sign/ratio comparisons, so the magnitude need not be
//! scaled or normalized.

use super::roster::{is_address_overlaid, IcaoRoster};
use super::ADSB_CAPTURE_RATE;
use super::{crc, LONG_FRAME_BITS, SHORT_FRAME_BITS};
use crate::mode::{DemodShape, Demodulator, Duplex, ModeCaps};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

/// Preamble-correlation threshold, readsb's `PREAMBLE_THRESHOLD_DEFAULT`. The
/// winning preamble correlation must clear `base_noise * THRESHOLD / 32` for a
/// position to be sliced (readsb shifts right by 5, i.e. divides by 32).
const PREAMBLE_THRESHOLD: f32 = 58.0;

/// Samples the scan holds back from the end of the working buffer so a preamble
/// found near the tail always has a whole long frame's worth of samples to slice
/// (preamble + 14 bytes at ~19–20 samples/byte, plus the FIR read-ahead). The
/// streaming buffer retains this tail across `feed` calls; `flush` zero-pads by
/// this much so the real tail is scanned. Generous by ~25%; the cost is holding
/// ~167 µs of samples, negligible on a multi-second capture.
const GUARD: usize = 400;

/// Recently-seen-ICAO roster capacity. Comfortably above the aircraft count in one
/// receiver's coverage, so no live aircraft is evicted while still transmitting;
/// eviction is least-recently-noted (an approximate age-out). See [`IcaoRoster`].
const ROSTER_CAP: usize = 4096;

/// The five hand-tuned FIR correlators from readsb `demod_2400.c`. Each returns a
/// signed matched-filter response for one bit at one of the five sub-sample
/// phases; the bit is a `1` when the response is positive. The coefficients were
/// iteratively tuned by readsb on real `--ifile` captures; [`slice_phase2`] and
/// [`slice_phase3`]/[`slice_phase4`] are deliberately not perfectly DC-balanced.
#[inline]
fn slice_phase0(m: &[f32]) -> f32 {
    18.0 * m[0] - 15.0 * m[1] - 3.0 * m[2]
}
#[inline]
fn slice_phase1(m: &[f32]) -> f32 {
    14.0 * m[0] - 5.0 * m[1] - 9.0 * m[2]
}
#[inline]
fn slice_phase2(m: &[f32]) -> f32 {
    16.0 * m[0] + 5.0 * m[1] - 20.0 * m[2]
}
#[inline]
fn slice_phase3(m: &[f32]) -> f32 {
    7.0 * m[0] + 11.0 * m[1] - 18.0 * m[2]
}
#[inline]
fn slice_phase4(m: &[f32]) -> f32 {
    4.0 * m[0] + 15.0 * m[1] - 20.0 * m[2] + 1.0 * m[3]
}

#[inline]
fn bit(v: f32) -> u8 {
    (v > 0.0) as u8
}

/// Extract one message byte from the magnitude buffer starting at `*ptr`, using
/// the FIR correlator schedule for the current `*phase`, then advance both.
///
/// Port of readsb's `slice_byte`. Eight bits span 8 × 2.4 = 19.2 samples, so the
/// byte stride alternates 19/19/19/19/20 across the five phases (the 6:5 sample:
/// symbol walk) and the phase cycles `0→1→2→3→4→0`. The per-bit correlator and
/// its sample offset within the byte are fixed tables in each `phase` arm.
fn slice_byte(m: &[f32], ptr: &mut usize, phase: &mut usize) -> u8 {
    let p = *ptr;
    match *phase {
        0 => {
            *phase = 1;
            *ptr += 19;
            (bit(slice_phase0(&m[p..])) << 7)
                | (bit(slice_phase2(&m[p + 2..])) << 6)
                | (bit(slice_phase4(&m[p + 4..])) << 5)
                | (bit(slice_phase1(&m[p + 7..])) << 4)
                | (bit(slice_phase3(&m[p + 9..])) << 3)
                | (bit(slice_phase0(&m[p + 12..])) << 2)
                | (bit(slice_phase2(&m[p + 14..])) << 1)
                | bit(slice_phase4(&m[p + 16..]))
        }
        1 => {
            *phase = 2;
            *ptr += 19;
            (bit(slice_phase1(&m[p..])) << 7)
                | (bit(slice_phase3(&m[p + 2..])) << 6)
                | (bit(slice_phase0(&m[p + 5..])) << 5)
                | (bit(slice_phase2(&m[p + 7..])) << 4)
                | (bit(slice_phase4(&m[p + 9..])) << 3)
                | (bit(slice_phase1(&m[p + 12..])) << 2)
                | (bit(slice_phase3(&m[p + 14..])) << 1)
                | bit(slice_phase0(&m[p + 17..]))
        }
        2 => {
            *phase = 3;
            *ptr += 19;
            (bit(slice_phase2(&m[p..])) << 7)
                | (bit(slice_phase4(&m[p + 2..])) << 6)
                | (bit(slice_phase1(&m[p + 5..])) << 5)
                | (bit(slice_phase3(&m[p + 7..])) << 4)
                | (bit(slice_phase0(&m[p + 10..])) << 3)
                | (bit(slice_phase2(&m[p + 12..])) << 2)
                | (bit(slice_phase4(&m[p + 14..])) << 1)
                | bit(slice_phase1(&m[p + 17..]))
        }
        3 => {
            *phase = 4;
            *ptr += 19;
            (bit(slice_phase3(&m[p..])) << 7)
                | (bit(slice_phase0(&m[p + 3..])) << 6)
                | (bit(slice_phase2(&m[p + 5..])) << 5)
                | (bit(slice_phase4(&m[p + 7..])) << 4)
                | (bit(slice_phase1(&m[p + 10..])) << 3)
                | (bit(slice_phase3(&m[p + 12..])) << 2)
                | (bit(slice_phase0(&m[p + 15..])) << 1)
                | bit(slice_phase2(&m[p + 17..]))
        }
        _ => {
            *phase = 0;
            *ptr += 20;
            (bit(slice_phase4(&m[p..])) << 7)
                | (bit(slice_phase1(&m[p + 3..])) << 6)
                | (bit(slice_phase3(&m[p + 5..])) << 5)
                | (bit(slice_phase0(&m[p + 8..])) << 4)
                | (bit(slice_phase2(&m[p + 10..])) << 3)
                | (bit(slice_phase4(&m[p + 12..])) << 2)
                | (bit(slice_phase1(&m[p + 15..])) << 1)
                | bit(slice_phase3(&m[p + 17..]))
        }
    }
}

/// Message length (bits) for a downlink format the native slicer decodes, or
/// `None` for a DF it will not attempt. Matches readsb's default DF bitsets:
/// short {0,4,5,11}, long {16,17,18,20,21}. The DF fixes the frame length before
/// the rest of the bytes are sliced; [`score_candidate`] then decides acceptance.
fn candidate_msg_bits(df: u8) -> Option<usize> {
    match df {
        0 | 4 | 5 | 11 => Some(SHORT_FRAME_BITS),
        16 | 17 | 18 | 20 | 21 => Some(LONG_FRAME_BITS),
        _ => None,
    }
}

/// 24-bit ICAO address carried in the clear (bits 8..32) of DF11/17/18.
fn clear_icao(bytes: &[u8]) -> u32 {
    ((bytes[1] as u32) << 16) | ((bytes[2] as u32) << 8) | bytes[3] as u32
}

/// Correct a unique single-bit error **outside the 5-bit DF field** in place.
/// readsb excludes the DF bits from its fix table — a corrected DF would
/// reinterpret the frame length — so a repair located there is declined. Returns
/// whether a repair was applied.
fn repair_single_bit(bytes: &mut [u8]) -> bool {
    match crc::locate_single_bit_error(crc::checksum(bytes), bytes.len() * 8) {
        Some(p) if p >= 5 => {
            bytes[p / 8] ^= 1 << (7 - (p % 8));
            true
        }
        _ => false,
    }
}

/// A scored, accepted candidate (readsb `scoreModesMessage`).
struct Scored {
    /// The accepted bytes, possibly single-bit-corrected.
    bytes: Vec<u8>,
    /// Higher = more confident; used to keep the best of the sliced phases.
    score: i32,
    /// `true` when the accepted bytes checksum to zero (clean or repaired).
    /// Address-overlaid frames validated by the roster are accepted but not
    /// CRC-zero.
    crc_ok: bool,
    /// Address to add to the roster because this frame proves it in the clear —
    /// a clean DF17 or a clean DF11 acquisition squitter (IID 0). `None` otherwise.
    seed_icao: Option<u32>,
}

/// Score a sliced candidate the way readsb's `scoreModesMessage` does: fuse the
/// CRC (and single-bit error correction) with membership in the recently-seen
/// ICAO roster. Returns `None` to reject.
///
/// - **DF17/18** (extended squitter, address in the clear): accepted clean, or
///   after a unique single-bit repair; a *clean DF17* also seeds the roster
///   (DF18 is TIS-B / non-transponder, so it never seeds).
/// - **DF11** (all-call reply): the low 7 bits of the residual are the interrogator
///   id, so a residual confined to them is clean; a clean IID-0 reply seeds the
///   roster. A residual beyond the IID bits is single-bit-repaired only when the
///   corrected address is already known.
/// - **DF0/4/5/16/20/21** (address-overlaid, `AP = parity XOR ICAO`): the residual
///   *is* the transmitting aircraft's address; accepted only when it is on the
///   roster. This rejects the shift-ghosts of real frames while recovering genuine
///   surveillance replies from aircraft already seen via a clean DF11/17.
fn score_candidate(mut bytes: Vec<u8>, roster: &IcaoRoster) -> Option<Scored> {
    // An all-zero slice checksums clean by construction (a flat/quiet stretch);
    // never a real transmission.
    if bytes.iter().all(|&b| b == 0) {
        return None;
    }
    let df = bytes[0] >> 3;
    let residual = crc::checksum(&bytes);
    match df {
        17 | 18 => {
            if residual == 0 {
                let icao = clear_icao(&bytes);
                let score = if roster.contains(icao) { 1800 } else { 1400 };
                let seed_icao = (df == 17).then_some(icao);
                Some(Scored { bytes, score, crc_ok: true, seed_icao })
            } else if repair_single_bit(&mut bytes) {
                let score = if roster.contains(clear_icao(&bytes)) { 900 } else { 700 };
                Some(Scored { bytes, score, crc_ok: true, seed_icao: None })
            } else {
                None
            }
        }
        11 => {
            let icao = clear_icao(&bytes);
            if residual & 0xffff80 == 0 {
                // Clean: only the interrogator-id bits may be set.
                let iid = residual & 0x7f;
                if iid == 0 {
                    let score = if roster.contains(icao) { 1600 } else { 750 };
                    Some(Scored { bytes, score, crc_ok: true, seed_icao: Some(icao) })
                } else if roster.contains(icao) {
                    Some(Scored { bytes, score: 1000, crc_ok: true, seed_icao: None })
                } else {
                    None
                }
            } else if repair_single_bit(&mut bytes) && roster.contains(clear_icao(&bytes)) {
                Some(Scored { bytes, score: 900, crc_ok: true, seed_icao: None })
            } else {
                None
            }
        }
        _ if is_address_overlaid(df) => {
            if roster.contains(residual) {
                Some(Scored { bytes, score: 1000, crc_ok: false, seed_icao: None })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Slice one preamble phase (readsb `try_phase`) and score it. `i` is the preamble
/// start; `try_phase` selects the data start offset and initial byte phase exactly
/// as readsb's `pa + 19 + try_phase/5`, `try_phase % 5`. Returns the scored
/// candidate and the buffer index one past the slice (where the scan resumes).
fn slice_and_score(m: &[f32], roster: &IcaoRoster, i: usize, try_phase: usize) -> Option<(Scored, usize)> {
    let mut ptr = i + 19 + try_phase / 5;
    let mut phase = try_phase % 5;
    let b0 = slice_byte(m, &mut ptr, &mut phase);
    let nbytes = candidate_msg_bits(b0 >> 3)? / 8;
    let mut bytes = vec![0u8; nbytes];
    bytes[0] = b0;
    for b in bytes.iter_mut().skip(1) {
        *b = slice_byte(m, &mut ptr, &mut phase);
    }
    score_candidate(bytes, roster).map(|s| (s, ptr))
}

/// Core scan: walk preamble positions in `m[..stop]`, and at each one that clears
/// the noise-relative preamble correlation, slice every admitted phase and keep the
/// highest-scoring frame. Clean DF17 / DF11-IID0 addresses seed `roster` in stream
/// order, so a later address-overlaid frame from a seen aircraft is recovered.
/// Returns the accepted frames and the buffer index the scan reached.
fn scan_buf(m: &[f32], roster: &mut IcaoRoster, base: u64, stop: usize) -> (Vec<Frame>, usize) {
    let mut frames = Vec::new();
    let mut i = 0usize;
    while i < stop {
        // readsb's cheap preamble pre-check: a spike at the first pulse over the
        // first quiet slot, and the fifth-pulse region over the guard.
        if !(m[i + 1] > m[i + 7] && m[i + 12] > m[i + 14] && m[i + 12] > m[i + 15]) {
            i += 1;
            continue;
        }
        // Noise reference from five quiet preamble slots, and the correlation gate:
        // winning preamble energy must clear base_noise * 58 / 32. readsb works in
        // integers and truncates (`>> 5`); the f32 port keeps the fractional part.
        let base_noise = m[i + 5] + m[i + 8] + m[i + 16] + m[i + 17] + m[i + 18];
        let ref_level = base_noise * PREAMBLE_THRESHOLD / 32.0;

        let diff_2_3 = m[i + 2] - m[i + 3];
        let sum_1_4 = m[i + 1] + m[i + 4];
        let diff_10_11 = m[i + 10] - m[i + 11];
        let common3456 = sum_1_4 - diff_2_3 + m[i + 9] + m[i + 12];

        // Each cleared preamble correlation admits one or two sub-sample data
        // phases to slice (readsb's phases 4..8); the best-scoring wins.
        let mut phases: [Option<usize>; 5] = [None; 5];
        let mut n = 0;
        if common3456 - diff_10_11 >= ref_level {
            phases[n] = Some(4);
            phases[n + 1] = Some(5);
            n += 2;
        }
        if common3456 + diff_10_11 >= ref_level {
            phases[n] = Some(6);
            phases[n + 1] = Some(7);
            n += 2;
        }
        if sum_1_4 + 2.0 * diff_2_3 + diff_10_11 + m[i + 12] >= ref_level {
            phases[n] = Some(8);
        }

        let mut best: Option<(Scored, usize)> = None;
        for tp in phases.into_iter().flatten() {
            if let Some((s, end)) = slice_and_score(m, roster, i, tp) {
                if best.as_ref().is_none_or(|(b, _)| s.score > b.score) {
                    best = Some((s, end));
                }
            }
        }

        if let Some((s, end)) = best {
            if let Some(icao) = s.seed_icao {
                roster.note(icao);
            }
            frames.push(Frame {
                payload: FramePayload::Packet(s.bytes),
                meta: FrameMeta {
                    crc_ok: s.crc_ok,
                    sample_offset: base + i as u64,
                    decoder: Some("adsb-native".to_string()),
                    ..Default::default()
                },
            });
            // Skip over the accepted message so its own bits can't seed a duplicate
            // preamble hit.
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    (frames, i)
}

/// Streaming native-2.4 Msps Mode S demodulator. Buffers the magnitude stream
/// across `feed` calls so a frame straddling a chunk boundary is not lost, and
/// emits each accepted frame as a `Packet` [`Frame`] carrying the raw 7/14 Mode S
/// bytes — the same payload the R1–R5 core emits, so downstream decode is
/// unchanged.
pub struct Demod2400 {
    buf: Vec<f32>,
    /// Absolute sample index of `buf[0]` in the fed stream, so emitted frames
    /// carry a stream-global `sample_offset`.
    base: u64,
    /// Recently-seen ICAO addresses (from clean DF17 / DF11-IID0), the trust anchor
    /// for accepting address-overlaid and error-corrected frames. Persists across
    /// `feed` calls; cleared on `reset`.
    roster: IcaoRoster,
}

impl Demod2400 {
    pub fn new() -> Self {
        Self { buf: Vec::new(), base: 0, roster: IcaoRoster::new(ROSTER_CAP) }
    }

    /// Scan `buf[..stop]`, drain the consumed prefix, and advance `base`.
    fn scan_and_drain(&mut self, stop: usize) -> Vec<Frame> {
        let base = self.base;
        let (frames, consumed) = scan_buf(&self.buf, &mut self.roster, base, stop);
        self.buf.drain(..consumed);
        self.base += consumed as u64;
        frames
    }
}

impl Default for Demod2400 {
    fn default() -> Self {
        Self::new()
    }
}

impl Demodulator for Demod2400 {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: ADSB_CAPTURE_RATE,
            bandwidth_hz: 2_400_000.0,
            tx: false,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    /// Feed the next chunk of 2.4 Msps magnitude samples, returning the frames
    /// wholly contained in what can be safely scanned. Samples near the tail are
    /// retained for the next call.
    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.buf.extend_from_slice(samples);
        let stop = self.buf.len().saturating_sub(GUARD);
        self.scan_and_drain(stop)
    }

    /// Scan the retained tail to completion. Zero-pads by [`GUARD`] so every real
    /// sample falls inside the scannable region (padding never false-positives —
    /// an all-zero slice is rejected).
    fn flush(&mut self) -> Vec<Frame> {
        let real = self.buf.len();
        self.buf.resize(real + GUARD, 0.0);
        // Scan directly (not via `scan_and_drain`, which advances `base` by the
        // consumed prefix): flush retires *all* `real` samples, so the buffer is
        // cleared and `base` advances by `real` exactly once — keeping
        // `sample_offset` correct if the demod is reused after a flush.
        let base = self.base;
        let (frames, _consumed) = scan_buf(&self.buf, &mut self.roster, base, real);
        self.buf.clear();
        self.base += real as u64;
        frames
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.base = 0;
        self.roster.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::super::message::{encode_all_call_reply, encode_identification};
    use super::super::ppm::PpmModulator;
    use super::super::CA_LEVEL2;
    use super::*;

    /// Fine oversampling used to synthesize a native-rate waveform: modulate at
    /// 12 samples/µs (whole slots), then decimate by 5 to land at 2.4 samples/µs
    /// — the capture rate readsb/[`Demod2400`] demodulate at. `offset` picks the
    /// decimation sub-sample phase, so different offsets exercise different
    /// preamble/data phases of the correlating slicer.
    const FINE_SPU: usize = 12;
    const DECIM: usize = 5;

    /// Round the square-pulse edges with a short moving average, so the 0.5 µs
    /// pulse spreads across the 2.4-sample grid the FIR correlators expect.
    fn round_edges(fine: &[f32], win: usize) -> Vec<f32> {
        (0..fine.len())
            .map(|i| {
                let lo = i.saturating_sub(win / 2);
                let hi = (i + win / 2).min(fine.len() - 1);
                fine[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32
            })
            .collect()
    }

    /// Synthesize a 2.4 Msps magnitude waveform for `frame` at decimation phase
    /// `offset` (`0..DECIM`), with quiet lead/trail so the demod has margins.
    fn synth_2400(frame: &[u8], offset: usize) -> Vec<f32> {
        let fine = PpmModulator::new(FINE_SPU).modulate_padded(frame, 8, 8);
        let rounded = round_edges(&fine, FINE_SPU / 2);
        rounded.iter().skip(offset).step_by(DECIM).copied().collect()
    }

    /// Concatenate several synthesized frames (varying decimation phase per frame)
    /// into one 2.4 Msps stream — models an aircraft squittering repeatedly, and
    /// lets an earlier clean DF17/DF11 seed the roster for a later overlaid frame.
    fn synth_stream(frames: &[&[u8]]) -> Vec<f32> {
        let mut out = Vec::new();
        for (k, f) in frames.iter().enumerate() {
            out.extend(synth_2400(f, k % DECIM));
        }
        out
    }

    /// Decode `mag` end-to-end through a fresh streaming demod (feed + flush).
    fn decode(mag: &[f32]) -> Vec<Vec<u8>> {
        let mut d = Demod2400::new();
        let mut frames = d.feed(mag);
        frames.extend(d.flush());
        frames
            .into_iter()
            .filter_map(|f| match f.payload {
                FramePayload::Packet(b) => Some(b),
                _ => None,
            })
            .collect()
    }

    /// An address-overlaid DF20 (Comm-B) frame with arbitrary content, and the
    /// address it checksums to. For AP-overlaid DFs the CRC residual *is* the
    /// transmitting aircraft's address (`AP = parity XOR ICAO`), so rather than
    /// solve the overlay we take whatever the frame checksums to as the address the
    /// roster must hold — exactly the value the demod recovers.
    fn overlaid_df20() -> (Vec<u8>, u32) {
        let mut f = vec![0u8; 14];
        f[0] = 20 << 3; // DF20
        f[1] = 0x11;
        f[2] = 0x22;
        f[4] = 0x20; // some MB payload so it isn't degenerate
        f[5] = 0xA5;
        let addr = crc::checksum(&f);
        (f, addr)
    }

    #[test]
    fn slice_phase_sign_matches_reference() {
        let one = [10.0, 0.0, 0.0, 0.0];
        let zero = [0.0, 0.0, 10.0, 0.0];
        for f in [slice_phase0, slice_phase1, slice_phase2, slice_phase3, slice_phase4] {
            assert!(f(&one) > 0.0);
            assert!(f(&zero) < 0.0);
        }
    }

    #[test]
    fn recovers_long_frame_across_all_decimation_phases() {
        let frame = encode_identification(0x4840D6, "KLM1023").to_vec();
        let mut recovered = 0;
        for offset in 0..DECIM {
            if decode(&synth_2400(&frame, offset)).contains(&frame) {
                recovered += 1;
            }
        }
        assert_eq!(recovered, DECIM, "frame should decode at every decimation phase");
    }

    #[test]
    fn recovers_short_all_call_reply() {
        let frame = encode_all_call_reply(0x3C6444, CA_LEVEL2).to_vec();
        assert_eq!(frame.len(), 7);
        assert!(decode(&synth_2400(&frame, 0)).contains(&frame), "DF11 not recovered");
    }

    #[test]
    fn emits_frame_exactly_once() {
        let frame = encode_identification(0x4840D6, "KLM1023").to_vec();
        let hits = decode(&synth_2400(&frame, 0)).into_iter().filter(|b| *b == frame).count();
        assert_eq!(hits, 1);
    }

    #[test]
    fn no_frames_from_silence_or_noise() {
        assert!(decode(&vec![0.0f32; 4000]).is_empty(), "silence must not decode");
        let ramp: Vec<f32> = (0..4000).map(|i| (i % 7) as f32 * 0.01).collect();
        assert!(decode(&ramp).is_empty(), "structureless noise must not decode");
    }

    #[test]
    fn recovers_frame_split_across_feeds() {
        let frame = encode_identification(0x4840D6, "KLM1023").to_vec();
        let mag = synth_2400(&frame, 2);
        let cut = mag.len() / 2;
        let mut d = Demod2400::new();
        let mut out = d.feed(&mag[..cut]);
        out.extend(d.feed(&mag[cut..]));
        out.extend(d.flush());
        let hits = out
            .iter()
            .filter(|f| matches!(&f.payload, FramePayload::Packet(b) if *b == frame))
            .count();
        assert_eq!(hits, 1, "split frame should be recovered exactly once");
    }

    // --- Stage B: score-based acceptance ------------------------------------

    #[test]
    fn single_bit_error_is_repaired() {
        // A DF17 with one flipped ME bit is corrected back to the clean frame and
        // emitted (readsb's default 1-bit fix).
        let clean = encode_identification(0x4CA111, "REPAIR1").to_vec();
        let mut corrupt = clean.clone();
        corrupt[6] ^= 0x04; // one bit outside the DF field
        assert_ne!(crc::checksum(&corrupt), 0);
        let out = decode(&synth_2400(&corrupt, 0));
        assert!(out.contains(&clean), "1-bit error should be repaired to the clean frame");
        assert!(!out.contains(&corrupt), "the corrupted bytes must not be emitted");
    }

    #[test]
    fn two_bit_error_is_rejected() {
        // Two flips have no unique single-bit explanation, so the frame is dropped
        // rather than mis-corrected (the false-positive discipline).
        let mut frame = encode_identification(0x4CA222, "REJECT2").to_vec();
        frame[5] ^= 0x20;
        frame[9] ^= 0x01;
        assert!(decode(&synth_2400(&frame, 0)).iter().all(|b| b != &frame));
    }

    #[test]
    fn ghost_shift_of_df11_is_not_admitted_as_df5() {
        // The Mode S CRC is shift-invariant, so a bit-misaligned slice of a real
        // DF11 can checksum clean and read as an address-overlaid DF (e.g. DF5).
        // With the roster gate, no such ghost is emitted from a lone DF11 whose
        // address is the only thing on the roster — any DF5/DF4 output would be a
        // fabricated aircraft.
        let df11 = encode_all_call_reply(0x3C6444, CA_LEVEL2).to_vec();
        let out = decode(&synth_2400(&df11, 0));
        assert!(out.contains(&df11), "the real DF11 should decode");
        for b in &out {
            let df = b[0] >> 3;
            assert!(
                matches!(df, 11 | 17 | 18),
                "only self-validating DFs may be emitted without a prior sighting; got DF{df}: {b:02X?}"
            );
        }
    }

    #[test]
    fn address_overlaid_frame_recovered_only_after_the_address_is_seen() {
        let (df20, addr) = overlaid_df20();
        assert_ne!(addr, 0);
        // A clean DF17 for the same address seeds the roster.
        let df17 = encode_identification(addr, "SEEN1").to_vec();

        // Alone, the DF20 has no corroborating sighting → rejected.
        assert!(
            !decode(&synth_2400(&df20, 0)).contains(&df20),
            "an address-overlaid frame with no prior sighting must be rejected"
        );

        // After the DF17 seeds the roster, the DF20 is recovered.
        let out = decode(&synth_stream(&[&df17, &df20]));
        assert!(out.contains(&df17), "the seeding DF17 should decode");
        assert!(out.contains(&df20), "the DF20 should be recovered once its address is known");
    }
}
