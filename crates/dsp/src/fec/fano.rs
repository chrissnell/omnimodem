//! K=32, rate-1/2 convolutional code with a Fano sequential decoder. WSJT-X JT9
//! and WSPR use this; Viterbi is impractical at K=32 (2^31 states). Generator
//! polynomials 0xf2d05351 / 0xe4613c47 (WSJT-X `wsprd`/`jt9` — confirm against
//! the reference when wiring those modes for cross-decode). Bit order MSB-first
//! into the register (`reg = (reg << 1) | bit`), matching WSJT-X.
//!
//! The decoder uses the soft Fano (Gallager) branch metric
//! `m(L,c) = 1 − log2(1 + e^{∓L}) − R`, with rate bias `R = 1/2` per code bit:
//! the metric is positive when the hypothesised code bit agrees with the sign of
//! the LLR and reliability is high, and strongly negative on disagreement, so
//! the correct path drifts up while wrong paths drift down. Threshold tightening
//! plus a node-visit budget bound the search; a hopeless (pure-noise) input
//! exhausts the budget and returns `None` rather than emitting garbage.

use crate::types::Llr;

/// Number of zero tail bits the encoder flushes to terminate the register.
const TAIL: usize = 31;
/// Rate bias per code bit (rate-1/2 ⇒ 0.5), the Fano-metric drift term.
const RATE_BIAS: f32 = 0.5;
/// Default node-visit budget; a noisy input that can't converge hits this.
const DEFAULT_BUDGET: u32 = 200_000;

pub struct FanoCode {
    polys: [u32; 2],
}

impl Default for FanoCode {
    fn default() -> Self {
        FanoCode { polys: [0xf2d0_5351, 0xe461_3c47] }
    }
}

impl FanoCode {
    /// Encode `data` bits with a 31-bit zero tail; 2 output bits per input bit.
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let mut reg: u32 = 0;
        let mut out = Vec::with_capacity((data.len() + TAIL) * 2);
        for &bit in data.iter().chain(std::iter::repeat(&0).take(TAIL)) {
            reg = (reg << 1) | (bit as u32 & 1);
            for p in self.polys {
                out.push((reg & p).count_ones() as u8 & 1);
            }
        }
        out
    }

    /// Fano sequential decode. `llrs`: 2 per trellis step (positive ⇒ code bit
    /// 0). `data_len`: payload bits (the 31-bit tail is decoded then dropped).
    /// `delta`: Fano threshold step. Returns `Some(bits)` if a full-length path
    /// is found within the node budget, else `None`.
    pub fn fano_decode(&self, llrs: &[Llr], data_len: usize, delta: f32) -> Option<Vec<u8>> {
        self.fano_decode_budget(llrs, data_len, delta, DEFAULT_BUDGET)
    }

    fn fano_decode_budget(
        &self,
        llrs: &[Llr],
        data_len: usize,
        delta: f32,
        budget: u32,
    ) -> Option<Vec<u8>> {
        let total = data_len + TAIL;
        if llrs.len() < total * 2 || delta <= 0.0 {
            return None;
        }

        // Per-depth state. reg[d]/mu[d] describe the node at depth d; tried[d]
        // counts how many of its two children we have descended into; bit[d] is
        // the chosen bit on the edge from depth d to d+1.
        let mut reg = vec![0u32; total + 1];
        let mut mu = vec![0.0f32; total + 1];
        let mut tried = vec![0u8; total + 1];
        let mut bit = vec![0u8; total];

        let mut d = 0usize;
        let mut t = 0.0f32; // running threshold
        let mut nodes = 0u32;

        loop {
            nodes += 1;
            if nodes > budget {
                return None;
            }
            if d == total {
                return Some(bit[..data_len].to_vec());
            }

            // Cumulative metrics of the two children of node d.
            let cm = |b: u32| -> f32 {
                let nr = (reg[d] << 1) | b;
                let c0 = ((nr & self.polys[0]).count_ones() & 1) as u8;
                let c1 = ((nr & self.polys[1]).count_ones() & 1) as u8;
                mu[d] + fano_bit_metric(llrs[2 * d], c0) + fano_bit_metric(llrs[2 * d + 1], c1)
            };
            let cm0 = cm(0);
            let cm1 = cm(1);
            // Order children best-first.
            let (better_bit, better_cm, worse_bit, worse_cm) = if cm0 >= cm1 {
                (0u32, cm0, 1u32, cm1)
            } else {
                (1u32, cm1, 0u32, cm0)
            };

            // Select the next untried child (in best-first order).
            let next = match tried[d] {
                0 => Some((better_bit, better_cm)),
                1 => Some((worse_bit, worse_cm)),
                _ => None,
            };

            if let Some((b, child_cm)) = next {
                if child_cm >= t {
                    // Move forward to this child.
                    tried[d] += 1;
                    bit[d] = b as u8;
                    reg[d + 1] = (reg[d] << 1) | b;
                    mu[d + 1] = child_cm;
                    d += 1;
                    tried[d] = 0; // first visit to the new node
                    // Tighten the threshold up toward the new node's metric.
                    while mu[d] - t >= delta {
                        t += delta;
                    }
                    continue;
                }
                // else: best remaining child is below threshold → look back.
            }

            // Look back: move to the parent if it is itself above threshold;
            // otherwise lower the threshold and re-examine this node.
            if d > 0 && mu[d - 1] >= t {
                d -= 1; // parent's tried[] already advanced; try its next child
            } else {
                t -= delta;
                tried[d] = 0; // re-examine children under the lower threshold
            }
        }
    }
}

/// Soft Fano branch metric for one code bit. `l` is the LLR (positive ⇒ bit 0),
/// `c` the hypothesised code bit.
fn fano_bit_metric(l: Llr, c: u8) -> f32 {
    let l = l.clamp(-60.0, 60.0);
    let e = if c == 0 { (-l).exp() } else { l.exp() };
    1.0 - (1.0 + e).log2() - RATE_BIAS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn llr(bits: &[u8]) -> Vec<Llr> {
        bits.iter().map(|&b| if b == 0 { 3.0 } else { -3.0 }).collect()
    }

    #[test]
    fn round_trips_clean_input() {
        let code = FanoCode::default();
        let data: Vec<u8> = (0..50).map(|i| ((i * 7 + 3) % 2) as u8).collect();
        let enc = code.encode(&data);
        let dec = code.fano_decode(&llr(&enc), data.len(), 0.5).expect("decode");
        assert_eq!(dec, data);
    }

    #[test]
    fn recovers_from_soft_errors() {
        // Realistic soft errors: a few code bits arrive with the WRONG sign but
        // LOW confidence (|L| small). The Fano metric lets the globally
        // consistent correct path override them. (Fully confident wrong bits are
        // the adversarial hard-error case sequential decoding declares failure
        // on — that's by design, not a bug.)
        let code = FanoCode::default();
        let data: Vec<u8> = (0..40).map(|i| ((i * 5 + 1) % 2) as u8).collect();
        let enc = code.encode(&data);
        let mut soft = llr(&enc);
        for &pos in &[7usize, 58, 102] {
            soft[pos] = -soft[pos].signum() * 0.4; // weak, wrong-signed
        }
        let dec = code.fano_decode(&soft, data.len(), 0.5).expect("decode");
        assert_eq!(dec, data);
    }

    #[test]
    fn returns_none_on_pure_noise() {
        let code = FanoCode::default();
        let noise: Vec<Llr> = (0..400)
            .map(|i| if i % 3 == 0 { 0.1 } else { -0.1 })
            .collect();
        assert!(code.fano_decode(&noise, 50, 0.5).is_none());
    }

    #[test]
    fn short_payload_round_trips() {
        let code = FanoCode::default();
        let data = [1u8, 0, 0, 1, 1, 0, 1, 1, 0, 0];
        let enc = code.encode(&data);
        let dec = code.fano_decode(&llr(&enc), data.len(), 0.5).expect("decode");
        assert_eq!(dec, data.to_vec());
    }
}
