//! Native 2.4 Msps correlating Mode S demodulator — R6 Stage A.
//!
//! The R1–R5 core downsamples the 2.4 Msps capture to a 2.0 MHz working rate,
//! slices each half-µs PPM slot with a 2-tap slot mean, and recovers off-grid
//! timing with a 12-phase interpolation ensemble. readsb/dump1090-fa instead
//! demodulate **natively at 2.4 Msps** with a **matched-filter correlating
//! slicer**: five hand-tuned FIR functions ([`slice_phase0`]..[`slice_phase4`])
//! sample the un-decimated pulse (2.4 samples per bit, 12 samples per 5 bits),
//! a 6:5 phase walk ([`slice_byte`]) reads a whole message byte at a time, and
//! the best-aligned of five preamble phases is kept. Decoding at the capture
//! rate keeps the sharp 0.5 µs pulse edges a 2.0 MHz anti-alias lowpass throws
//! away — the structural advantage behind readsb's weak-/short-frame yield.
//!
//! This module is a faithful port of readsb `demod_2400.c`
//! (`slice_phase*`, `slice_byte`, the preamble correlation, `demodulate2400`),
//! with **Stage A acceptance = CRC-clean only**: a candidate is emitted when its
//! Mode S parity checksums to zero (DF11/17/18 in practice). The score-based
//! acceptance that also recovers the address-overlaid DFs (DF0/4/5/20/21) and
//! 1-bit-corrected frames is Stage B, layered on top of this slicer.
//!
//! It works on an `f32` magnitude (envelope) stream at [`super::ADSB_CAPTURE_RATE`]
//! (2.4 Msps): the same `|I + jQ|` the daemon already computes, but fed at the
//! capture rate with **no resample**. The five FIR functions and the preamble
//! correlation are all sign/ratio comparisons, so the magnitude need not be
//! scaled or normalized.

use super::crc;
use super::{LONG_FRAME_BITS, SHORT_FRAME_BITS};
use crate::types::{Frame, FrameMeta, FramePayload};

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
/// the rest of the bytes are sliced; Stage A then accepts only the ones that
/// checksum clean (DF11/17/18 in practice — the address-overlaid DFs are Stage B).
fn candidate_msg_bits(df: u8) -> Option<usize> {
    match df {
        0 | 4 | 5 | 11 => Some(SHORT_FRAME_BITS),
        16 | 17 | 18 | 20 | 21 => Some(LONG_FRAME_BITS),
        _ => None,
    }
}

/// A CRC-clean candidate the slicer accepted at one preamble phase.
struct Candidate {
    bytes: Vec<u8>,
    /// Buffer index one past the last sample the slice consumed — where the scan
    /// resumes after emitting this frame (readsb's "skip over the message").
    end: usize,
}

/// Streaming native-2.4 Msps Mode S demodulator. Buffers the magnitude stream
/// across `feed` calls so a frame straddling a chunk boundary is not lost, and
/// emits each CRC-clean frame as a `Packet` [`Frame`] carrying the raw 7/14
/// Mode S bytes — the same payload the R1–R5 core emits, so downstream decode is
/// unchanged.
#[derive(Default)]
pub struct Demod2400 {
    buf: Vec<f32>,
    /// Absolute sample index of `buf[0]` in the fed stream, so emitted frames
    /// carry a stream-global `sample_offset`.
    base: u64,
}

impl Demod2400 {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the next chunk of 2.4 Msps magnitude samples, returning the CRC-clean
    /// frames wholly contained in what can be safely scanned. Samples near the
    /// tail are retained for the next call.
    pub fn feed(&mut self, mag: &[f32]) -> Vec<Frame> {
        self.buf.extend_from_slice(mag);
        let stop = self.buf.len().saturating_sub(GUARD);
        self.scan_and_drain(stop)
    }

    /// Scan the retained tail to completion. Zero-pads by [`GUARD`] so every real
    /// sample falls inside the scannable region (padding never false-positives —
    /// an all-zero slice is rejected).
    pub fn flush(&mut self) -> Vec<Frame> {
        let real = self.buf.len();
        self.buf.resize(real + GUARD, 0.0);
        let frames = self.scan_and_drain(real);
        self.buf.clear();
        self.base += real as u64;
        frames
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.base = 0;
    }

    /// Scan `buf[..stop]` for frames, then drain the consumed prefix and advance
    /// `base`. `stop` bounds the first preamble position considered; a frame may
    /// legitimately extend past `stop` into the guard tail.
    fn scan_and_drain(&mut self, stop: usize) -> Vec<Frame> {
        let (frames, consumed) = self.scan(stop);
        self.buf.drain(..consumed);
        self.base += consumed as u64;
        frames
    }

    /// Core scan: walk preamble positions in `buf[..stop]`, and at each one that
    /// clears the noise-relative preamble correlation, slice every eligible phase
    /// and keep the first CRC-clean frame. Returns the accepted frames and the
    /// buffer index the scan reached (the prefix that can be drained).
    fn scan(&self, stop: usize) -> (Vec<Frame>, usize) {
        let m = &self.buf;
        let mut frames = Vec::new();
        let mut i = 0usize;
        while i < stop {
            // readsb's cheap preamble pre-check: a spike at the first pulse over
            // the first quiet slot, and the fifth-pulse region over the guard.
            if !(m[i + 1] > m[i + 7] && m[i + 12] > m[i + 14] && m[i + 12] > m[i + 15]) {
                i += 1;
                continue;
            }
            // Noise reference from five quiet preamble slots, and the correlation
            // gate: winning preamble energy must clear base_noise * 58 / 32.
            let base_noise = m[i + 5] + m[i + 8] + m[i + 16] + m[i + 17] + m[i + 18];
            let ref_level = base_noise * PREAMBLE_THRESHOLD / 32.0;

            let diff_2_3 = m[i + 2] - m[i + 3];
            let sum_1_4 = m[i + 1] + m[i + 4];
            let diff_10_11 = m[i + 10] - m[i + 11];
            let common3456 = sum_1_4 - diff_2_3 + m[i + 9] + m[i + 12];

            // Each preamble correlation, when it clears the gate, admits one or two
            // sub-sample data phases to actually slice (readsb's phases 4..8).
            let mut best: Option<Candidate> = None;
            if common3456 - diff_10_11 >= ref_level {
                self.try_phase(i, 4, &mut best);
                self.try_phase(i, 5, &mut best);
            }
            if common3456 + diff_10_11 >= ref_level {
                self.try_phase(i, 6, &mut best);
                self.try_phase(i, 7, &mut best);
            }
            if sum_1_4 + 2.0 * diff_2_3 + diff_10_11 + m[i + 12] >= ref_level {
                self.try_phase(i, 8, &mut best);
            }

            if let Some(c) = best {
                frames.push(Frame {
                    payload: FramePayload::Packet(c.bytes),
                    meta: FrameMeta {
                        crc_ok: true,
                        sample_offset: self.base + i as u64,
                        decoder: Some("adsb-native".to_string()),
                        ..Default::default()
                    },
                });
                // Skip over the accepted message so its own bits can't seed a
                // duplicate preamble hit.
                i = c.end.max(i + 1);
            } else {
                i += 1;
            }
        }
        (frames, i)
    }

    /// Slice one preamble phase (readsb `try_phase`), and if it yields a CRC-clean
    /// frame, record it in `best` (keeping the first — clean copies agree). `i` is
    /// the preamble start; `try_phase` selects the data start offset and the
    /// initial byte phase exactly as readsb's `pa + 19 + try_phase/5`, `try_phase%5`.
    fn try_phase(&self, i: usize, try_phase: usize, best: &mut Option<Candidate>) {
        if best.is_some() {
            return;
        }
        let m = &self.buf;
        let mut ptr = i + 19 + try_phase / 5;
        let mut phase = try_phase % 5;

        let b0 = slice_byte(m, &mut ptr, &mut phase);
        let Some(nbits) = candidate_msg_bits(b0 >> 3) else {
            return;
        };
        let nbytes = nbits / 8;
        let mut bytes = vec![0u8; nbytes];
        bytes[0] = b0;
        for b in bytes.iter_mut().skip(1) {
            *b = slice_byte(m, &mut ptr, &mut phase);
        }

        // Stage A: accept only a parity-clean frame. Reject the all-zero slice a
        // flat/quiet stretch produces — its DF0 checksums to zero by construction
        // but is never a real transmission.
        if crc::checksum(&bytes) == 0 && bytes.iter().any(|&x| x != 0) {
            *best = Some(Candidate { bytes, end: ptr });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::message::{encode_all_call_reply, encode_identification};
    use super::super::ppm::PpmModulator;
    use super::super::CA_LEVEL2;
    use super::*;
    use crate::types::FramePayload;

    /// Fine oversampling used to synthesize a native-rate waveform: modulate at
    /// 12 samples/µs (whole slots), then decimate by 5 to land at 2.4 samples/µs
    /// — the capture rate readsb/[`Demod2400`] demodulate at. `offset` picks the
    /// decimation sub-sample phase, so different offsets exercise different
    /// preamble/data phases of the correlating slicer.
    const FINE_SPU: usize = 12;
    const DECIM: usize = 5;

    /// Round the square-pulse edges with a short moving average, so the 0.5 µs
    /// pulse spreads across the 2.4-sample grid the FIR correlators expect —
    /// pure squares put full energy in a single sample and don't model an
    /// off-air (band-limited) pulse.
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
        rounded
            .iter()
            .skip(offset)
            .step_by(DECIM)
            .copied()
            .collect()
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

    #[test]
    fn slice_phase_sign_matches_reference() {
        // A pulse concentrated in the leading sample gives a positive correlation
        // (bit 1); one concentrated in the trailing samples gives negative (bit 0).
        let one = [10.0, 0.0, 0.0, 0.0];
        let zero = [0.0, 0.0, 10.0, 0.0];
        for f in [
            slice_phase0,
            slice_phase1,
            slice_phase2,
            slice_phase3,
            slice_phase4,
        ] {
            assert!(f(&one) > 0.0);
            assert!(f(&zero) < 0.0);
        }
    }

    #[test]
    fn recovers_long_frame_across_all_decimation_phases() {
        // The core Stage A claim: a DF17 frame captured at 2.4 Msps decodes at
        // every sub-sample decimation phase, because the slicer searches all five
        // preamble phases and keeps the aligned one.
        let frame = encode_identification(0x4840D6, "KLM1023").to_vec();
        let mut recovered = 0;
        for offset in 0..DECIM {
            let mag = synth_2400(&frame, offset);
            if decode(&mag).iter().any(|b| *b == frame) {
                recovered += 1;
            }
        }
        assert_eq!(
            recovered, DECIM,
            "frame should decode at every decimation phase"
        );
    }

    #[test]
    fn recovers_short_all_call_reply() {
        // A short (DF11) frame checksums clean too and must be sliced at 56 bits.
        let frame = encode_all_call_reply(0x3C6444, CA_LEVEL2).to_vec();
        assert_eq!(frame.len(), 7);
        let mag = synth_2400(&frame, 0);
        let out = decode(&mag);
        assert!(
            out.iter().any(|b| *b == frame),
            "DF11 short frame not recovered: {out:?}"
        );
    }

    #[test]
    fn emits_frame_exactly_once() {
        let frame = encode_identification(0x4840D6, "KLM1023").to_vec();
        let mag = synth_2400(&frame, 0);
        let hits = decode(&mag).into_iter().filter(|b| *b == frame).count();
        assert_eq!(
            hits, 1,
            "the frame should be emitted exactly once, not duplicated"
        );
    }

    #[test]
    fn rejects_crc_corrupted_frame() {
        // Flip a payload bit so the parity no longer checks — Stage A accepts only
        // CRC-clean frames, so it must not appear.
        let mut frame = encode_identification(0x4840D6, "KLM1023").to_vec();
        frame[5] ^= 0x20;
        let mag = synth_2400(&frame, 0);
        assert!(
            !decode(&mag).iter().any(|b| *b == frame),
            "a CRC-broken frame must be rejected in Stage A"
        );
    }

    #[test]
    fn no_frames_from_silence_or_noise() {
        // A flat/all-zero buffer (its all-zero slice checksums clean by
        // construction) and a slowly ramping buffer must both yield nothing.
        assert!(
            decode(&vec![0.0f32; 4000]).is_empty(),
            "silence must not decode"
        );
        let ramp: Vec<f32> = (0..4000).map(|i| (i % 7) as f32 * 0.01).collect();
        assert!(
            decode(&ramp).is_empty(),
            "structureless noise must not decode"
        );
    }

    #[test]
    fn recovers_frame_split_across_feeds() {
        // The streaming buffer must reassemble a frame straddling a feed boundary.
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
}
