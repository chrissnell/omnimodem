//! LDPC encode + min-sum belief-propagation decode, parametric in `H`.
//!
//! Ships an FT8/FT4-shaped `(N=174, K=91)` code. The decoder machinery
//! (systematic encode, sparse Tanner-graph min-sum BP with 0.75 scaling,
//! parity check) is complete and general; it works for any binary parity
//! matrix supplied to [`Ldpc::from_parity`].
//!
//! Matrix provenance: the exact FT8 tables (`kFTX_LDPC_Nm`, `kFTX_LDPC_Mn`,
//! `kFTX_LDPC_generator`) are to be transcribed from `ft8_lib`'s `ldpc.c` /
//! `constants.c` in Phase 4 and KAT'd against `encode174`. Until then
//! [`Ldpc::ft8`] builds an **internally-consistent, valid** `(174,91)` code
//! programmatically (a deterministic seeded systematic `H = [P | I]`, giving
//! generator `G = [I | Pᵀ]`). Only the specific matrix constants are
//! placeholder-but-valid; the codec itself is not a stub — encode→noiseless
//! decode round-trips and it corrects errors under AWGN.

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

    /// FT8/FT4-shaped `(174, 91)` code (placeholder-but-valid parity matrix —
    /// see module docs). Deterministic seeded construction so encode/decode are
    /// reproducible across runs.
    pub fn ft8() -> Self {
        let k = 91;
        let m = 174 - 91; // 83
        let mut rng: u64 = 0x6F8B_45A2_1C3D_9E07; // fixed seed
        let mut next = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        // Build a moderately sparse P: each data column touches ~3 checks,
        // each check row gets a handful of ones. Ensures a connected, low-
        // density Tanner graph that BP handles well.
        let mut p = vec![vec![0u8; k]; m];
        #[allow(clippy::needless_range_loop)] // col indexes the inner dimension across rows
        for col in 0..k {
            // place 3 ones in distinct rows for this column
            let mut placed = 0;
            while placed < 3 {
                let r = (next() as usize) % m;
                if p[r][col] == 0 {
                    p[r][col] = 1;
                    placed += 1;
                }
            }
        }
        // Guarantee no empty check row (every parity bit is constrained).
        for row in p.iter_mut() {
            if row.iter().all(|&b| b == 0) {
                let c = (next() as usize) % k;
                row[c] = 1;
            }
        }
        Self::from_systematic(k, &p)
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
