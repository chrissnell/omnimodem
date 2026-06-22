//! Parametric convolutional code: rate-1/n encoder + soft-decision Viterbi
//! decoder, with optional puncturing. Used by fldigi-family modes (K=7 MFSK16/
//! THOR/DominoEX). K=32 codes use [`super::fano`] instead — Viterbi is
//! impractical there (2^31 states).
//!
//! Bit order: data bits enter the shift register one at a time, newest bit in
//! the LSB (`reg = (reg << 1) | bit`), matching fldigi `viterbi.cxx`/`libm17`.
//! The generator polynomials are stored as raw bit masks (octal in the
//! reference). Decoders consume the locked [`Llr`] convention: `L =
//! ln(P(0)/P(1))`, positive ⇒ code bit 0.

use crate::types::Llr;

/// A rate-1/n convolutional code definition. `polys` are the n generator
/// polynomials, `k` is the constraint length (shift-register length = k).
#[derive(Debug, Clone)]
pub struct ConvCode {
    pub k: usize,
    pub polys: Vec<u32>,
}

impl ConvCode {
    /// Standard K=7, rate-1/2 code (NASA/CCSDS; fldigi MFSK/THOR), generator
    /// polynomials 0o171 / 0o133 (`viterbi.cxx`).
    pub fn k7_r12() -> Self {
        ConvCode { k: 7, polys: vec![0o171, 0o133] }
    }

    /// Encode data bits (each 0/1) → n output bits per input bit, with a
    /// zero-tail flush of `k-1` bits so the decoder terminates in the zero
    /// state. Output bit order: for each input bit, the n poly outputs in
    /// `polys` order.
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let n = self.polys.len();
        let mut reg: u32 = 0;
        let mut out = Vec::with_capacity((data.len() + self.k - 1) * n);
        let flushed = data.iter().copied().chain(std::iter::repeat_n(0, self.k - 1));
        for bit in flushed {
            reg = (reg << 1) | (bit as u32 & 1);
            for &p in &self.polys {
                out.push((reg & p).count_ones() as u8 & 1);
            }
        }
        out
    }

    /// Soft Viterbi decode: `llrs` carries n LLRs per trellis step (positive ⇒
    /// code bit 0). Returns the `data_len` decoded data bits (tail stripped).
    ///
    /// Survivor paths store both the input bit and the predecessor state, so
    /// traceback is exact regardless of constraint length.
    pub fn viterbi_decode(&self, llrs: &[Llr], data_len: usize) -> Vec<u8> {
        let n = self.polys.len();
        let states = 1usize << (self.k - 1);
        let mask = states - 1;
        let steps = data_len + self.k - 1;
        debug_assert_eq!(llrs.len(), steps * n, "expected {} LLRs", steps * n);

        let mut metric = vec![f32::NEG_INFINITY; states];
        metric[0] = 0.0;
        let mut bit_at = vec![0u8; steps * states]; // input bit entering (t, ns)
        let mut prev_at = vec![0usize; steps * states]; // predecessor of (t, ns)

        for t in 0..steps {
            let mut next = vec![f32::NEG_INFINITY; states];
            for (s, &ms) in metric.iter().enumerate() {
                if ms == f32::NEG_INFINITY {
                    continue;
                }
                for bit in 0..2u32 {
                    let reg = ((s as u32) << 1) | bit;
                    let ns = (reg as usize) & mask;
                    let mut branch = 0.0f32;
                    for (j, &p) in self.polys.iter().enumerate() {
                        let code_bit = (reg & p).count_ones() & 1;
                        let l = llrs[t * n + j];
                        // Correlation metric: +L if the code bit is 0, -L if 1.
                        branch += if code_bit == 0 { l } else { -l };
                    }
                    let cand = ms + branch;
                    if cand > next[ns] {
                        next[ns] = cand;
                        bit_at[t * states + ns] = bit as u8;
                        prev_at[t * states + ns] = s;
                    }
                }
            }
            metric = next;
        }

        // The zero tail forces termination in state 0; trace back from there.
        let mut out = vec![0u8; steps];
        let mut s = 0usize;
        for t in (0..steps).rev() {
            out[t] = bit_at[t * states + s];
            s = prev_at[t * states + s];
        }
        out.truncate(data_len);
        out
    }
}

/// Puncture an encoded stream by a boolean pattern (true = keep).
pub fn puncture(bits: &[u8], pattern: &[bool]) -> Vec<u8> {
    bits.iter()
        .zip(pattern.iter().cycle())
        .filter(|(_, &k)| k)
        .map(|(&b, _)| b)
        .collect()
}

/// Depuncture: re-insert erasure LLRs (`0.0`) where bits were dropped, restoring
/// the `full_len` trellis-step stream the Viterbi decoder expects.
pub fn depuncture(llrs: &[Llr], pattern: &[bool], full_len: usize) -> Vec<Llr> {
    let mut out = Vec::with_capacity(full_len);
    let mut it = llrs.iter();
    for i in 0..full_len {
        if pattern[i % pattern.len()] {
            out.push(it.next().copied().unwrap_or(0.0));
        } else {
            out.push(0.0); // erasure
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_to_llr(bits: &[u8]) -> Vec<Llr> {
        // Hard bits → strong LLRs (+4 for code bit 0, -4 for 1).
        bits.iter().map(|&b| if b == 0 { 4.0 } else { -4.0 }).collect()
    }

    #[test]
    fn encode_lengths_and_tail() {
        let code = ConvCode::k7_r12();
        let enc = code.encode(&[1, 0, 1, 1]);
        assert_eq!(enc.len(), (4 + 6) * 2);
    }

    #[test]
    fn encode_is_deterministic_known_vector() {
        // Regression KAT pinning the K=7 (0o171,0o133) encoder for a fixed
        // input. The polynomials are the standard NASA/CCSDS pair fldigi uses;
        // this locks our output so any accidental change to bit/poly order is
        // caught. (Bidirectional cross-decode vs fldigi is the #[ignore] nightly
        // gate in tests/kat.rs.)
        let code = ConvCode::k7_r12();
        let enc = code.encode(&[1, 0, 1]);
        // 3 data + 6 tail = 9 steps × 2 = 18 output bits.
        let expected: [u8; 18] = [1, 1, 0, 1, 1, 1, 1, 0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1];
        assert_eq!(enc, expected);
    }

    #[test]
    fn viterbi_round_trips_clean() {
        let code = ConvCode::k7_r12();
        let data = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0];
        let enc = code.encode(&data);
        let dec = code.viterbi_decode(&bits_to_llr(&enc), data.len());
        assert_eq!(dec, data);
    }

    #[test]
    fn viterbi_corrects_a_few_errors() {
        let code = ConvCode::k7_r12();
        let data = [1u8, 1, 0, 1, 0, 1, 1, 0, 0, 1];
        let mut enc = code.encode(&data);
        enc[3] ^= 1;
        enc[10] ^= 1; // two spread bit flips a K=7 r=1/2 code recovers
        let dec = code.viterbi_decode(&bits_to_llr(&enc), data.len());
        assert_eq!(dec, data);
    }

    #[test]
    fn puncture_depuncture_inserts_erasures() {
        let p = [true, true, false]; // rate-2/3 puncture
        let bits = [1u8, 0, 1, 1, 0, 1];
        let punc = puncture(&bits, &p);
        assert_eq!(punc.len(), 4);
        let de = depuncture(&bits_to_llr(&punc), &p, 6);
        assert_eq!(de[2], 0.0); // erased position
        assert_eq!(de[5], 0.0);
    }

    #[test]
    fn punctured_round_trip_recovers_data() {
        // rate-2/3: keep 2 of every 3 code bits, decode through erasures.
        let code = ConvCode::k7_r12();
        let data = [1u8, 0, 0, 1, 1, 0, 1, 0];
        let enc = code.encode(&data);
        let pat = [true, true, false];
        let punc = puncture(&enc, &pat);
        let de = depuncture(&bits_to_llr(&punc), &pat, enc.len());
        assert_eq!(code.viterbi_decode(&de, data.len()), data);
    }
}
