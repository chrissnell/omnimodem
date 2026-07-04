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

    /// A continuous (streaming) soft Viterbi decoder for this code, for modes
    /// whose payload length is not known in advance and whose trellis is never
    /// tail-terminated (fldigi's differential PSK/QPSK feed the decoder one
    /// symbol at a time forever). See [`StreamingViterbi`].
    pub fn streaming_decoder(&self, traceback: usize) -> StreamingViterbi {
        StreamingViterbi::new(self.clone(), traceback)
    }
}

/// Continuous soft-decision Viterbi decoder with a fixed traceback depth.
///
/// Unlike [`ConvCode::viterbi_decode`] (block, zero-tail-terminated), this keeps
/// a rolling survivor history of `traceback` steps and, once primed, emits one
/// decoded bit per pushed trellis step — the bit on the best survivor path
/// `traceback` steps in the past. This matches fldigi's `viterbi` class, which
/// decodes the never-terminated convolutional stream of the QPSK/PSK-R modes.
pub struct StreamingViterbi {
    code: ConvCode,
    states: usize,
    mask: usize,
    metric: Vec<f32>,
    // Ring buffers of `depth` steps: the entering bit and predecessor per state.
    depth: usize,
    bit_at: Vec<u8>,   // depth * states
    prev_at: Vec<usize>, // depth * states
    pos: usize,        // ring write index (mod depth)
    filled: usize,     // steps pushed, capped at depth
}

impl StreamingViterbi {
    fn new(code: ConvCode, traceback: usize) -> Self {
        let states = 1usize << (code.k - 1);
        let depth = traceback.max(1);
        let mut metric = vec![f32::NEG_INFINITY; states];
        metric[0] = 0.0; // encoder starts in the zero state
        StreamingViterbi {
            code,
            states,
            mask: states - 1,
            metric,
            depth,
            bit_at: vec![0u8; depth * states],
            prev_at: vec![0usize; depth * states],
            pos: 0,
            filled: 0,
        }
    }

    /// Push one trellis step's `n` LLRs (positive ⇒ code bit 0, `polys` order).
    /// Returns the decoded data bit `traceback` steps ago once the history has
    /// filled, else `None`.
    pub fn push(&mut self, llrs: &[Llr]) -> Option<u8> {
        let n = self.code.polys.len();
        debug_assert_eq!(llrs.len(), n);
        let mut next = vec![f32::NEG_INFINITY; self.states];
        let base = self.pos * self.states;
        for (s, &ms) in self.metric.iter().enumerate() {
            if ms == f32::NEG_INFINITY {
                continue;
            }
            for bit in 0..2u32 {
                let reg = ((s as u32) << 1) | bit;
                let ns = (reg as usize) & self.mask;
                let mut branch = 0.0f32;
                for (j, &p) in self.code.polys.iter().enumerate() {
                    let code_bit = (reg & p).count_ones() & 1;
                    branch += if code_bit == 0 { llrs[j] } else { -llrs[j] };
                }
                let cand = ms + branch;
                if cand > next[ns] {
                    next[ns] = cand;
                    self.bit_at[base + ns] = bit as u8;
                    self.prev_at[base + ns] = s;
                }
            }
        }
        // Renormalise so metrics stay bounded on an endless stream.
        let best = next.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        if best.is_finite() {
            for m in next.iter_mut() {
                if m.is_finite() {
                    *m -= best;
                }
            }
        }
        self.metric = next;
        self.pos = (self.pos + 1) % self.depth;
        // After writing `depth` steps (steps 0..depth-1) the history spans the
        // full traceback window, so the first emit lands on data bit 0.
        self.filled = (self.filled + 1).min(self.depth);
        if self.filled < self.depth {
            return None;
        }
        // Trace back `depth` steps from the current best state and emit the
        // oldest bit (the one about to be overwritten at `self.pos`).
        let mut s = self
            .metric
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);
        let mut ring = (self.pos + self.depth - 1) % self.depth; // most-recent write
        let mut oldest_bit = 0u8;
        for _ in 0..self.depth {
            let idx = ring * self.states + s;
            oldest_bit = self.bit_at[idx];
            s = self.prev_at[idx];
            ring = (ring + self.depth - 1) % self.depth;
        }
        Some(oldest_bit)
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

    /// Bit-exact: the PSK-R robust K=7 code (POLY 0x6d/0x4f) reproduces fldigi's
    /// `encoder(7,0x6d,0x4f)` code-symbol sequence over an MFSK-varicode
    /// bitstream. Provenance: `tests/vectors/psk_robust.json` (fldigi 4.1.23 @
    /// 61b97f413, driver `scratch/refvectors/build_psk_robust.sh`). This is the
    /// FEC prerequisite for the PSK-R / +F robust modes.
    #[test]
    fn k7_pskr_matches_fldigi_vector() {
        let raw = include_str!("../../tests/vectors/psk_robust.json");
        let line = raw.lines().find(|l| l.contains("\"pskr_symbols\"")).unwrap();
        let field = |k: &str| {
            let i = line.find(k).unwrap() + k.len();
            line[i..line[i..].find('"').unwrap() + i].to_string()
        };
        let vbits: Vec<u8> = field("\"mfsk_bits\":\"").bytes().map(|c| c - b'0').collect();
        let want: Vec<u8> =
            field("\"pskr_symbols\":\"").split(' ').map(|s| s.parse().unwrap()).collect();
        let code = ConvCode { k: 7, polys: vec![0x6d, 0x4f] };
        let out = code.encode(&vbits);
        let got: Vec<u8> = (0..want.len()).map(|i| out[2 * i] | (out[2 * i + 1] << 1)).collect();
        assert_eq!(got, want, "K=7 PSK-R code symbols differ from fldigi");
    }

    #[test]
    fn streaming_viterbi_recovers_stream_after_traceback_delay() {
        // A K=5 code (as QPSK uses): encode a bit stream, feed the code bits as
        // strong LLRs to the streaming decoder, and confirm the emitted bits are
        // the input delayed by the traceback depth, with no tail termination.
        let code = ConvCode { k: 5, polys: vec![0x17, 0x19] };
        let data: Vec<u8> = (0..200u32).map(|i| ((i * 37 + 11) >> 2 & 1) as u8).collect();
        // Encode WITHOUT the block tail (continuous stream): raw per-bit outputs.
        let mut reg = 0u32;
        let mut coded = Vec::new();
        for &b in &data {
            reg = (reg << 1) | b as u32;
            for &p in &code.polys {
                coded.push((reg & p).count_ones() as u8 & 1);
            }
        }
        let depth = 30;
        let mut dec = code.streaming_decoder(depth);
        let mut out = Vec::new();
        for pair in coded.chunks(2) {
            if let Some(b) = dec.push(&bits_to_llr(pair)) {
                out.push(b);
            }
        }
        // `out[i]` is `data[i]` (the decoder emits one bit per step once primed,
        // lagging by `depth`). Compare the overlapping, settled region.
        assert!(out.len() >= data.len() - depth);
        for i in 0..(data.len() - depth) {
            assert_eq!(out[i], data[i], "streaming viterbi bit {i}");
        }
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

