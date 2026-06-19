//! LDPC encode + min-sum belief-propagation decode, parametric in `H`.
//!
//! Ships the FT8/FT4 `(N=174, K=91)` code. The decoder machinery (systematic
//! encode, sparse Tanner-graph min-sum BP with 0.75 scaling, parity check) is
//! complete and general; it works for any binary parity matrix supplied to
//! [`Ldpc::from_systematic`].
//!
//! Matrix provenance: [`Ldpc::ft8`] uses the **real WSJT-X tables** transcribed
//! byte-for-byte from `ft8_lib` (`kgoba/ft8_lib`, `ft8/constants.c`): the packed
//! `kFTX_LDPC_generator` for systematic encode and `kFTX_LDPC_Nm` /
//! `kFTX_LDPC_Num_rows` for the parity-check Tanner graph (see
//! [`super::ft8_tables`]). The codeword layout matches `ft8_lib`'s `encode174`:
//! 91 payload bits (77-bit message + 14-bit CRC) followed by 83 parity bits,
//! all MSB-first. This is bit-exact interoperable with WSJT-X — a codeword built
//! by [`Ldpc::encode`] satisfies every `Nm` parity check, and the `kat.rs`
//! `ft8_ldpc_matches_reference` test confirms every systematic generator row is
//! itself a codeword of the `Nm` parity-check matrix (`G·Hᵀ = 0`), so the two
//! independently-transcribed tables are mutually consistent.

use crate::types::Llr;

pub struct Ldpc {
    n: usize,
    k: usize,
    /// Systematic generator rows: `gen[i]` is a length-`n` bit row; codeword
    /// bit `j = Σ_i msg_i · gen[i][j]`. Rows 0..k form `[I | Pᵀ]`.
    gen: Vec<Vec<u8>>,
    /// Parity-check rows: `check_vars[c]` lists variable indices in check `c`.
    check_vars: Vec<Vec<usize>>,
}

impl Ldpc {
    pub fn n(&self) -> usize {
        self.n
    }
    pub fn k(&self) -> usize {
        self.k
    }

    /// Build from a dense parity matrix in systematic form `H = [P | I_m]`,
    /// where `p` has `m` rows of `k` bits each (`m = n - k`). The parity part
    /// is the trailing `m×m` identity, so the generator is `G = [I_k | Pᵀ]`.
    pub fn from_systematic(k: usize, p: &[Vec<u8>]) -> Self {
        let m = p.len();
        let n = k + m;
        // Generator: row i (0..k) = e_i followed by column i of P (= Pᵀ row i).
        let mut gen = vec![vec![0u8; n]; k];
        for (i, row) in gen.iter_mut().enumerate() {
            row[i] = 1;
            for (c, prow) in p.iter().enumerate() {
                row[k + c] = prow[i] & 1;
            }
        }
        // Parity checks: check c covers data vars where P[c][i]==1 plus its own
        // identity parity var (k + c).
        let mut check_vars = vec![Vec::new(); m];
        for (c, prow) in p.iter().enumerate() {
            for (i, &b) in prow.iter().enumerate() {
                if b & 1 == 1 {
                    check_vars[c].push(i);
                }
            }
            check_vars[c].push(k + c);
        }
        Ldpc { n, k, gen, check_vars }
    }

    /// The FT8/FT4 `(174, 91)` LDPC code, using the real WSJT-X / `ft8_lib`
    /// tables (see module docs and [`super::ft8_tables`]).
    ///
    /// The systematic generator rows are unpacked from the bit-packed
    /// `kFTX_LDPC_generator` (parity bit `c` is the XOR of the message bits its
    /// row selects), and the Tanner graph comes straight from `kFTX_LDPC_Nm`.
    /// Codeword layout is `[payload(91) | parity(83)]`, MSB-first, identical to
    /// `ft8_lib`'s `encode174`.
    pub fn ft8() -> Self {
        use super::ft8_tables::{FT8_LDPC_GENERATOR, FT8_LDPC_NM, FT8_LDPC_NUM_ROWS};
        const N: usize = 174;
        const K: usize = 91;
        const M: usize = 83;

        // Systematic generator: row j is e_j (bit j of the payload) followed by
        // the parity contributions of message bit j. ft8_lib packs each parity
        // row MSB-first, so message bit j lives in byte j/8 at bit 7-(j%8).
        let mut gen = vec![vec![0u8; N]; K];
        #[allow(clippy::needless_range_loop)] // c indexes the parity dimension across the packed rows
        for (j, row) in gen.iter_mut().enumerate() {
            row[j] = 1;
            for c in 0..M {
                let byte = FT8_LDPC_GENERATOR[c][j / 8];
                row[K + c] = (byte >> (7 - (j % 8))) & 1;
            }
        }

        // Parity checks: Nm lists the 1-origin codeword indices in each check,
        // zero-padded to 7; Num_rows gives the valid count.
        let mut check_vars = vec![Vec::new(); M];
        for (c, vars) in check_vars.iter_mut().enumerate() {
            for &v in FT8_LDPC_NM[c].iter().take(FT8_LDPC_NUM_ROWS[c] as usize) {
                vars.push(v as usize - 1);
            }
        }

        Ldpc { n: N, k: K, gen, check_vars }
    }

    /// Systematic encode: `k` message bits → `n` codeword bits.
    pub fn encode(&self, message_bits: &[u8]) -> Vec<u8> {
        assert_eq!(message_bits.len(), self.k, "expected {} message bits", self.k);
        let mut cw = vec![0u8; self.n];
        for (i, &mi) in message_bits.iter().enumerate() {
            if mi & 1 == 1 {
                for (j, c) in cw.iter_mut().enumerate() {
                    *c ^= self.gen[i][j];
                }
            }
        }
        cw
    }

    /// Count unsatisfied parity checks for a hard codeword.
    pub fn parity_errors(&self, hard: &[u8]) -> usize {
        self.check_vars
            .iter()
            .filter(|vars| vars.iter().map(|&v| hard[v] as usize).sum::<usize>() & 1 != 0)
            .count()
    }

    /// Min-sum belief-propagation decode.
    ///
    /// Input LLRs follow the locked convention `L = ln(P(0)/P(1))` (positive ⇒
    /// bit 0). Returns the `n`-bit hard decision and the number of unsatisfied
    /// parity checks (caller takes the first `k` bits as the message and
    /// requires `parity_errors == 0`). Min-sum messages are scaled by 0.75.
    pub fn decode_minsum(&self, llrs: &[Llr], max_iters: usize) -> (Vec<u8>, usize) {
        assert_eq!(llrs.len(), self.n);
        const SCALE: f32 = 0.75;
        let m = self.check_vars.len();

        // Variable→check messages, indexed [check][position-in-check].
        let mut v2c: Vec<Vec<f32>> = self
            .check_vars
            .iter()
            .map(|vars| vars.iter().map(|&v| llrs[v]).collect())
            .collect();
        let mut c2v: Vec<Vec<f32>> = self.check_vars.iter().map(|vars| vec![0.0; vars.len()]).collect();

        let mut hard = self.hard_from_llr(llrs);
        let mut best_hard = hard.clone();
        let mut best_errs = self.parity_errors(&hard);
        if best_errs == 0 {
            return (best_hard, 0);
        }

        for _ in 0..max_iters {
            // Check node update (min-sum): for each edge, product of signs ×
            // (scaled) min |msg| excluding this edge.
            for c in 0..m {
                let msgs = &v2c[c];
                let deg = msgs.len();
                // sign product and two smallest magnitudes
                let mut sign = 1.0f32;
                let mut min1 = f32::INFINITY;
                let mut min2 = f32::INFINITY;
                let mut min_idx = 0usize;
                for (i, &x) in msgs.iter().enumerate() {
                    if x < 0.0 {
                        sign = -sign;
                    }
                    let a = x.abs();
                    if a < min1 {
                        min2 = min1;
                        min1 = a;
                        min_idx = i;
                    } else if a < min2 {
                        min2 = a;
                    }
                }
                for i in 0..deg {
                    let self_sign = if msgs[i] < 0.0 { -1.0 } else { 1.0 };
                    let mag = if i == min_idx { min2 } else { min1 };
                    c2v[c][i] = SCALE * (sign * self_sign) * mag;
                }
            }

            // Variable node update: total = channel + Σ incoming; outgoing on
            // an edge excludes that edge's incoming message.
            // Recompute per-variable totals.
            let mut total = llrs.to_vec();
            for (vars, c2v_c) in self.check_vars.iter().zip(c2v.iter()) {
                for (&v, &msg) in vars.iter().zip(c2v_c.iter()) {
                    total[v] += msg;
                }
            }
            for ((vars, c2v_c), v2c_c) in
                self.check_vars.iter().zip(c2v.iter()).zip(v2c.iter_mut())
            {
                for ((&v, &msg), out) in vars.iter().zip(c2v_c.iter()).zip(v2c_c.iter_mut()) {
                    *out = total[v] - msg;
                }
            }

            // Hard decision from totals.
            for (v, t) in total.iter().enumerate() {
                hard[v] = u8::from(*t < 0.0);
            }
            let errs = self.parity_errors(&hard);
            if errs < best_errs {
                best_errs = errs;
                best_hard = hard.clone();
            }
            if errs == 0 {
                return (hard, 0);
            }
        }
        (best_hard, best_errs)
    }

    fn hard_from_llr(&self, llrs: &[Llr]) -> Vec<u8> {
        llrs.iter().map(|&l| u8::from(l < 0.0)).collect()
    }

    // --- OSD support (used by `osd.rs`) ---

    /// Generator rows `[I_k | Pᵀ]` (length `k`, each `n` bits wide).
    pub fn generator_rows(&self) -> &[Vec<u8>] {
        &self.gen
    }

    /// Re-encode a `k`-bit message into the full `n`-bit codeword.
    pub fn reencode(&self, message: &[u8]) -> Vec<u8> {
        self.encode(message)
    }

    /// Variable (codeword-bit) indices participating in parity check `c`.
    pub fn check_vars(&self, c: usize) -> &[usize] {
        &self.check_vars[c]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Rng;

    fn bits_to_llr(bits: &[u8], conf: f32) -> Vec<Llr> {
        // bit 0 -> +conf, bit 1 -> -conf
        bits.iter().map(|&b| if b == 0 { conf } else { -conf }).collect()
    }

    #[test]
    fn noiseless_roundtrip() {
        let code = Ldpc::ft8();
        let mut rng = Rng::new(99);
        for _ in 0..20 {
            let msg: Vec<u8> = (0..code.k()).map(|_| (rng.next_u64() & 1) as u8).collect();
            let cw = code.encode(&msg);
            assert_eq!(code.parity_errors(&cw), 0, "encoder produced invalid codeword");
            let llr = bits_to_llr(&cw, 4.0);
            let (dec, errs) = code.decode_minsum(&llr, 50);
            assert_eq!(errs, 0);
            assert_eq!(&dec[..code.k()], &msg[..]);
        }
    }

    #[test]
    fn corrects_single_flips() {
        let code = Ldpc::ft8();
        let mut rng = Rng::new(7);
        let msg: Vec<u8> = (0..code.k()).map(|_| (rng.next_u64() & 1) as u8).collect();
        let cw = code.encode(&msg);
        for flip in [0usize, 50, 173] {
            let mut llr = bits_to_llr(&cw, 5.0);
            llr[flip] = -llr[flip]; // confidently wrong on one bit
            let (dec, errs) = code.decode_minsum(&llr, 50);
            assert_eq!(errs, 0, "failed to correct flip at {flip}");
            assert_eq!(&dec[..code.k()], &msg[..]);
        }
    }

    #[test]
    fn ber_smoke_over_awgn() {
        let code = Ldpc::ft8();
        let mut rng = Rng::new(2026);
        let trials = 100;
        let mut ok = 0;
        // BPSK over AWGN: tx bit 0 -> +1, bit 1 -> -1; rx = tx + noise.
        // sigma chosen for a moderate, comfortably-decodable SNR.
        let sigma = 0.45f32;
        let noise_var = sigma * sigma;
        for _ in 0..trials {
            let msg: Vec<u8> = (0..code.k()).map(|_| (rng.next_u64() & 1) as u8).collect();
            let cw = code.encode(&msg);
            let llr: Vec<Llr> = cw
                .iter()
                .map(|&b| {
                    let tx = if b == 0 { 1.0 } else { -1.0 };
                    let rx = tx + sigma * rng.next_normal();
                    2.0 * rx / noise_var // demap_bpsk
                })
                .collect();
            let (dec, errs) = code.decode_minsum(&llr, 50);
            if errs == 0 && dec[..code.k()] == msg[..] {
                ok += 1;
            }
        }
        assert!(ok > 90, "decode rate {ok}/{trials} below floor");
    }
}
