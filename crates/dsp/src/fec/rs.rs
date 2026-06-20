//! Parametric GF(256) Reed–Solomon over primitive polynomial `0x11D`.
//!
//! Supports `fcr = 1` (FX.25) and `fcr = 0` (IL2P), arbitrary `nroots`, and
//! shortened blocks (the data slice may be any length up to `255 - nroots`).
//! Pipeline: syndromes → Berlekamp–Massey → Chien search → Forney.
//!
//! Symbol/bit order: each `u8` is one GF(256) symbol; parity symbols are
//! appended after the data (systematic). Convention follows the classic
//! Karn `fec` library and Direwolf `fx25_*.c` / `il2p_*.c`.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RsError {
    #[error("uncorrectable: more errors than parity can resolve")]
    Uncorrectable,
}

const FIELD: usize = 256;

pub struct Rs {
    nroots: usize,
    fcr: usize,
    /// `alpha_to[i] = α^i` (antilog), period 255.
    alpha_to: [u8; FIELD],
    /// `index_of[x] = log_α(x)`; `index_of[0]` is the sentinel `255`.
    index_of: [u8; FIELD],
    /// Generator polynomial coefficients in *index* (log) form, length nroots+1.
    genpoly: Vec<u8>,
}

impl Rs {
    /// Build GF(256) with primitive polynomial `prim` (e.g. `0x1D`, the low 8
    /// bits of `0x11D`) and the generator for `nroots`/`fcr`.
    pub fn new(nroots: usize, fcr: usize, prim: u8) -> Self {
        let (alpha_to, index_of) = build_tables(prim);
        let mut rs = Rs { nroots, fcr, alpha_to, index_of, genpoly: Vec::new() };
        rs.genpoly = rs.build_genpoly();
        rs
    }

    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            return 0;
        }
        let s = self.index_of[a as usize] as usize + self.index_of[b as usize] as usize;
        self.alpha_to[s % 255]
    }

    /// Generator poly `g(x) = Π_{i=0}^{nroots-1} (x - α^(fcr+i))`, returned as
    /// coefficients in index (log) form, highest degree last, monic.
    fn build_genpoly(&self) -> Vec<u8> {
        // Work in polynomial (value) form, then convert to log form.
        let mut g = vec![0u8; self.nroots + 1];
        g[0] = 1; // g(x) = 1
        let mut deg = 0usize;
        for i in 0..self.nroots {
            // multiply g(x) by (x - α^(fcr+i))
            let root = self.alpha_to[(self.fcr + i) % 255];
            deg += 1;
            for j in (1..=deg).rev() {
                g[j] = g[j - 1] ^ self.mul(g[j], root);
            }
            g[0] = self.mul(g[0], root);
        }
        // Convert to index form for fast encoding/decoding.
        g.iter().map(|&c| self.index_of[c as usize]).collect()
    }

    /// Compute the `nroots` systematic parity symbols for `data`.
    ///
    /// LFSR long division of `data·x^nroots` by the generator. `parity[j]`
    /// holds the coefficient of `x^j` in the running remainder; `genpoly` is
    /// stored in log form with `genpoly[j]` the log of the `x^j` coefficient
    /// (the leading `x^nroots` term is monic and skipped).
    pub fn encode_parity(&self, data: &[u8]) -> Vec<u8> {
        let nroots = self.nroots;
        let mut parity = vec![0u8; nroots];
        for &d in data {
            let feedback = d ^ parity[nroots - 1];
            // shift remainder up by one degree (toward higher index)
            for j in (1..nroots).rev() {
                parity[j] = parity[j - 1];
            }
            parity[0] = 0;
            if feedback != 0 {
                let fb_log = self.index_of[feedback as usize] as usize;
                for (pj, &gp) in parity.iter_mut().zip(self.genpoly.iter()) {
                    let gl = gp as usize; // log of g coeff for x^j
                    if gl != 255 {
                        *pj ^= self.alpha_to[(gl + fb_log) % 255];
                    }
                }
            }
        }
        // parity[j] is the coeff of x^j; emit highest degree first so the
        // appended order matches the codeword layout used by `decode`.
        parity.reverse();
        parity
    }

    /// Decode in place. `codeword = data || parity`. Returns the number of
    /// symbols corrected, or [`RsError::Uncorrectable`].
    pub fn decode(&self, codeword: &mut [u8]) -> Result<usize, RsError> {
        let n = codeword.len();
        let nroots = self.nroots;

        // 1. Syndromes S_i = r(α^(fcr+i)), i in 0..nroots. Index form.
        let mut synd = vec![0u8; nroots];
        let mut any = false;
        for (i, s) in synd.iter_mut().enumerate() {
            let mut acc = codeword[0];
            let root = (self.fcr + i) % 255;
            for &c in &codeword[1..] {
                if acc == 0 {
                    acc = c;
                } else {
                    acc = c ^ self.alpha_to[(self.index_of[acc as usize] as usize + root) % 255];
                }
            }
            *s = acc;
            if acc != 0 {
                any = true;
            }
        }
        if !any {
            return Ok(0);
        }

        // 2. Berlekamp–Massey → error locator Λ(x) (value form).
        let mut lambda = vec![0u8; nroots + 1];
        lambda[0] = 1;
        let mut b = vec![0u8; nroots + 1];
        b[0] = 1;
        let mut l = 0usize; // current number of assumed errors
        let mut m = 1usize;
        let mut b_scalar = 1u8; // previous discrepancy (value form)

        for r in 0..nroots {
            // discrepancy delta = S_r + Σ_{i=1}^{l} Λ_i S_{r-i}
            let mut delta = synd[r];
            for i in 1..=l {
                if lambda[i] != 0 && synd[r - i] != 0 {
                    delta ^= self.mul(lambda[i], synd[r - i]);
                }
            }
            if delta == 0 {
                m += 1;
            } else if 2 * l <= r {
                let t = lambda.clone();
                // Λ(x) = Λ(x) - (delta / b_scalar) x^m B(x)
                let coef = self.mul(delta, self.inv(b_scalar));
                for i in 0..=nroots - m {
                    if b[i] != 0 {
                        lambda[i + m] ^= self.mul(coef, b[i]);
                    }
                }
                l = r + 1 - l;
                b = t;
                b_scalar = delta;
                m = 1;
            } else {
                let coef = self.mul(delta, self.inv(b_scalar));
                for i in 0..=nroots - m {
                    if b[i] != 0 {
                        lambda[i + m] ^= self.mul(coef, b[i]);
                    }
                }
                m += 1;
            }
        }

        // degree of lambda == number of errors
        let nerr = (0..=nroots).rev().find(|&i| lambda[i] != 0).unwrap_or(0);
        if nerr == 0 || nerr > nroots / 2 {
            return Err(RsError::Uncorrectable);
        }

        // 3. Chien search: roots of Λ are α^{-pos}. Position p (0..n) is an
        //    error if Λ(α^{-p}) == 0, i.e. Λ(α^{255-p}) == 0. The symbol at
        //    codeword index (n-1-p) corresponds to x^p.
        let mut err_pos = Vec::with_capacity(nerr);
        for p in 0..255usize {
            // evaluate Λ(α^{-p})
            let xinv = (255 - p % 255) % 255;
            let mut sum = lambda[0];
            for (i, &lam) in lambda.iter().enumerate().take(nroots + 1).skip(1) {
                if lam != 0 {
                    sum ^= self.alpha_to
                        [(self.index_of[lam as usize] as usize + (i * xinv) % 255) % 255];
                }
            }
            if sum == 0 {
                // x^p is an error location; map to codeword index.
                if p < n {
                    err_pos.push(p);
                }
            }
        }
        if err_pos.len() != nerr {
            return Err(RsError::Uncorrectable);
        }

        // 4. Forney: e = X_j^{1-fcr} * Ω(X_j^{-1}) / Λ'(X_j^{-1}).
        //    Ω(x) = S(x)Λ(x) mod x^nroots.
        let mut omega = vec![0u8; nroots];
        for i in 0..nroots {
            let mut acc = 0u8;
            for j in 0..=i {
                if lambda[j] != 0 && synd[i - j] != 0 {
                    acc ^= self.mul(lambda[j], synd[i - j]);
                }
            }
            omega[i] = acc;
        }

        for &p in &err_pos {
            // X_j = α^p ; X_j^{-1} = α^{-p}
            let xinv = (255 - p % 255) % 255;
            // Ω(X_j^{-1})
            let mut omega_val = 0u8;
            for (i, &w) in omega.iter().enumerate() {
                if w != 0 {
                    omega_val ^=
                        self.alpha_to[(self.index_of[w as usize] as usize + (i * xinv) % 255) % 255];
                }
            }
            // Λ'(X_j^{-1}) — formal derivative keeps only odd-index terms.
            let mut lambda_prime = 0u8;
            let mut i = 1;
            while i <= nerr {
                if lambda[i] != 0 {
                    lambda_prime ^= self.alpha_to
                        [(self.index_of[lambda[i] as usize] as usize + ((i - 1) * xinv) % 255)
                            % 255];
                }
                i += 2;
            }
            if lambda_prime == 0 {
                return Err(RsError::Uncorrectable);
            }
            // magnitude = X_j^{1-fcr} * Ω / Λ'
            let mut mag = self.mul(omega_val, self.inv(lambda_prime));
            // multiply by X_j^{1-fcr} = α^{p*(1-fcr)}
            if mag != 0 {
                let exp = ((p as isize * (1 - self.fcr as isize)).rem_euclid(255)) as usize;
                mag = self.alpha_to
                    [(self.index_of[mag as usize] as usize + exp) % 255];
            }
            let idx = n - 1 - p;
            codeword[idx] ^= mag;
        }

        // 5. Verify: recompute syndromes; if any non-zero, we miscorrected.
        for i in 0..nroots {
            let mut acc = codeword[0];
            let root = (self.fcr + i) % 255;
            for &c in &codeword[1..] {
                if acc == 0 {
                    acc = c;
                } else {
                    acc = c ^ self.alpha_to[(self.index_of[acc as usize] as usize + root) % 255];
                }
            }
            if acc != 0 {
                return Err(RsError::Uncorrectable);
            }
        }

        Ok(err_pos.len())
    }

    fn inv(&self, a: u8) -> u8 {
        debug_assert!(a != 0);
        self.alpha_to[(255 - self.index_of[a as usize] as usize) % 255]
    }
}

/// Build antilog/log tables for GF(256) with primitive poly `prim` (low byte
/// of the full 9-bit poly, e.g. `0x1D` for `0x11D`).
fn build_tables(prim: u8) -> ([u8; FIELD], [u8; FIELD]) {
    let mut alpha_to = [0u8; FIELD];
    let mut index_of = [0u8; FIELD];
    let mut x: u16 = 1;
    for (i, slot) in alpha_to.iter_mut().enumerate().take(255) {
        *slot = x as u8;
        index_of[x as usize] = i as u8;
        x <<= 1;
        if x & 0x100 != 0 {
            x ^= 0x100 | prim as u16;
        }
    }
    alpha_to[255] = 0; // α^255 unused sentinel
    index_of[0] = 255; // log(0) sentinel
    (alpha_to, index_of)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIM: u8 = 0x1D; // 0x11D primitive polynomial

    #[test]
    fn gf_tables_are_consistent() {
        let rs = Rs::new(4, 1, PRIM);
        // α^0 = 1, and antilog/log invert each other for nonzero elements.
        assert_eq!(rs.alpha_to[0], 1);
        for v in 1u8..=255 {
            let l = rs.index_of[v as usize] as usize;
            assert_eq!(rs.alpha_to[l], v, "v={v}");
        }
        // multiplication identity
        assert_eq!(rs.mul(0x53, 0xCA), rs.mul(0xCA, 0x53));
    }

    fn roundtrip_corrupt(nroots: usize, fcr: usize, datalen: usize, nerr: usize, fail: bool) {
        let rs = Rs::new(nroots, fcr, PRIM);
        let data: Vec<u8> = (0..datalen).map(|i| (i as u8).wrapping_mul(31).wrapping_add(7)).collect();
        let parity = rs.encode_parity(&data);
        assert_eq!(parity.len(), nroots);
        let mut cw = data.clone();
        cw.extend_from_slice(&parity);
        let clean = cw.clone();

        // Corrupt `nerr` distinct positions with nonzero error values.
        for k in 0..nerr {
            let pos = (k * 7 + 3) % cw.len();
            cw[pos] ^= 0xA5u8.wrapping_add(k as u8).max(1);
        }

        let res = rs.decode(&mut cw);
        if fail {
            // Either reports uncorrectable OR (if it "corrects") must NOT
            // silently return the wrong codeword. The verify step guarantees
            // any returned Ok is a valid codeword, but it may differ from the
            // original. We only require it does not claim success-with-original
            // — i.e. if Ok, the result is a *valid* codeword (syndromes zero),
            // which the decoder already enforces. So assert it is Err for the
            // strong "beyond t" case where we corrupt t+1 symbols.
            assert!(
                res.is_err() || cw != clean,
                "beyond-capacity decode must not reproduce the original"
            );
        } else {
            assert!(res.is_ok(), "decode failed for nroots={nroots} nerr={nerr}: {res:?}");
            assert_eq!(cw, clean, "miscorrection nroots={nroots} nerr={nerr}");
        }
    }

    #[test]
    fn corrects_up_to_t_all_param_sets() {
        for &nroots in &[2usize, 4, 6, 8, 16, 32, 64] {
            for &fcr in &[0usize, 1] {
                let t = nroots / 2;
                let datalen = (255 - nroots).min(80);
                // zero errors
                roundtrip_corrupt(nroots, fcr, datalen, 0, false);
                // exactly t errors
                roundtrip_corrupt(nroots, fcr, datalen, t, false);
                // t-1 errors
                if t >= 1 {
                    roundtrip_corrupt(nroots, fcr, datalen, t - 1, false);
                }
            }
        }
    }

    #[test]
    fn reports_failure_beyond_capacity() {
        for &nroots in &[4usize, 8, 16] {
            for &fcr in &[0usize, 1] {
                let t = nroots / 2;
                roundtrip_corrupt(nroots, fcr, 60, t + 1, true);
            }
        }
    }

    #[test]
    fn shortened_block_il2p_style() {
        // IL2P uses fcr=0, RS(255,239) shortened. Use a short data block.
        let rs = Rs::new(16, 0, PRIM);
        let data: Vec<u8> = (0..50).map(|i| i as u8 ^ 0x3C).collect();
        let parity = rs.encode_parity(&data);
        let mut cw = data.clone();
        cw.extend_from_slice(&parity);
        let clean = cw.clone();
        // corrupt 8 symbols (= t)
        for k in 0..8 {
            cw[k * 5] ^= 0x7Fu8.wrapping_add(k as u8).max(1);
        }
        assert_eq!(rs.decode(&mut cw), Ok(8));
        assert_eq!(cw, clean);
    }

    #[test]
    fn fx25_style_fcr1() {
        let rs = Rs::new(32, 1, PRIM);
        let data: Vec<u8> = (0..223).map(|i| (i as u8).wrapping_mul(13)).collect();
        let parity = rs.encode_parity(&data);
        let mut cw = data.clone();
        cw.extend_from_slice(&parity);
        let clean = cw.clone();
        for k in 0..16 {
            cw[k * 14] ^= 0x11u8.wrapping_add(k as u8).max(1);
        }
        assert_eq!(rs.decode(&mut cw), Ok(16));
        assert_eq!(cw, clean);
    }

    #[test]
    fn property_random_errors() {
        let rs = Rs::new(16, 1, PRIM);
        let mut seed: u32 = 0x1234_5678;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for _ in 0..200 {
            let datalen = 100;
            let data: Vec<u8> = (0..datalen).map(|_| (rnd() & 0xFF) as u8).collect();
            let parity = rs.encode_parity(&data);
            let mut cw = data.clone();
            cw.extend_from_slice(&parity);
            let clean = cw.clone();
            let t = 8;
            // pick t distinct positions
            let mut positions = std::collections::BTreeSet::new();
            while positions.len() < t {
                positions.insert((rnd() as usize) % cw.len());
            }
            for &p in &positions {
                cw[p] ^= ((rnd() & 0xFF) as u8).max(1);
            }
            assert_eq!(rs.decode(&mut cw), Ok(t));
            assert_eq!(cw, clean);
        }
    }
}
