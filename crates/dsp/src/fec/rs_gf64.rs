//! Reed–Solomon over GF(2⁶) for JT65 RS(63,12). Hard algebraic decode
//! (syndromes → Berlekamp–Massey → Chien → Forney) is the baseline here; the
//! soft-decision Franke–Taylor layer (trial-erasing the least-reliable symbols)
//! wraps this in the JT65 mode file where per-symbol reliabilities exist.
//!
//! GF(2⁶) primitive polynomial x⁶+x+1 (0x43), matching WSJT-X JT65. The
//! algebra mirrors the GF(256) [`super::rs`] decoder; the period is 63 and the
//! field has 64 elements. Symbol/codeword layout is systematic: `data || parity`.
//! Reference for the soft layer + exact symbol mapping: WSJT-X `lib/` Karn RS +
//! `ftrsd`.

const FIELD: usize = 64;
const PERIOD: usize = 63;

/// GF(2⁶) antilog/log tables.
pub struct Gf64 {
    alpha_to: [u8; FIELD],
    index_of: [u8; FIELD],
}

impl Default for Gf64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Gf64 {
    pub fn new() -> Self {
        let mut alpha_to = [0u8; FIELD];
        let mut index_of = [0u8; FIELD];
        let mut x: u16 = 1;
        for (i, slot) in alpha_to.iter_mut().enumerate().take(PERIOD) {
            *slot = x as u8;
            index_of[x as usize] = i as u8;
            x <<= 1;
            if x & 0x40 != 0 {
                x ^= 0x40 | 0x03; // x⁶ = x + 1
            }
        }
        alpha_to[PERIOD] = 0; // α^63 sentinel
        index_of[0] = PERIOD as u8; // log(0) sentinel
        Gf64 { alpha_to, index_of }
    }

    pub fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            return 0;
        }
        let s = self.index_of[a as usize] as usize + self.index_of[b as usize] as usize;
        self.alpha_to[s % PERIOD]
    }

    fn inv(&self, a: u8) -> u8 {
        debug_assert!(a != 0);
        self.alpha_to[(PERIOD - self.index_of[a as usize] as usize) % PERIOD]
    }
}

/// Generic Reed–Solomon over GF(2⁶).
pub struct RsGf64 {
    field: Gf64,
    nroots: usize,
    fcr: usize,
    genpoly: Vec<u8>, // log form, length nroots+1
}

impl RsGf64 {
    /// RS(63,12) for JT65: 51 parity symbols, fcr = 1.
    pub fn jt65() -> Self {
        Self::new(51, 1)
    }

    pub fn new(nroots: usize, fcr: usize) -> Self {
        let field = Gf64::new();
        let mut rs = RsGf64 { field, nroots, fcr, genpoly: Vec::new() };
        rs.genpoly = rs.build_genpoly();
        rs
    }

    fn build_genpoly(&self) -> Vec<u8> {
        let mut g = vec![0u8; self.nroots + 1];
        g[0] = 1;
        let mut deg = 0usize;
        for i in 0..self.nroots {
            let root = self.field.alpha_to[(self.fcr + i) % PERIOD];
            deg += 1;
            for j in (1..=deg).rev() {
                g[j] = g[j - 1] ^ self.field.mul(g[j], root);
            }
            g[0] = self.field.mul(g[0], root);
        }
        g.iter().map(|&c| self.field.index_of[c as usize]).collect()
    }

    /// Compute the `nroots` systematic parity symbols for `data` (highest degree
    /// first, matching the `data || parity` codeword layout).
    pub fn encode_parity(&self, data: &[u8]) -> Vec<u8> {
        let nroots = self.nroots;
        let mut parity = vec![0u8; nroots];
        for &d in data {
            let feedback = d ^ parity[nroots - 1];
            for j in (1..nroots).rev() {
                parity[j] = parity[j - 1];
            }
            parity[0] = 0;
            if feedback != 0 {
                let fb_log = self.field.index_of[feedback as usize] as usize;
                for (pj, &gp) in parity.iter_mut().zip(self.genpoly.iter()) {
                    let gl = gp as usize;
                    if gl != PERIOD {
                        *pj ^= self.field.alpha_to[(gl + fb_log) % PERIOD];
                    }
                }
            }
        }
        parity.reverse();
        parity
    }

    /// Systematic JT65 encode: 12 data symbols → 63-symbol codeword.
    pub fn encode(&self, data: &[u8; 12]) -> Vec<u8> {
        let mut out = data.to_vec();
        out.extend(self.encode_parity(data));
        out
    }

    /// Hard-decision decode of a 63-symbol JT65 codeword → 12 data symbols, or
    /// `None` if uncorrectable.
    pub fn decode(&self, received: &[u8; 63]) -> Option<[u8; 12]> {
        let mut cw = received.to_vec();
        self.decode_in_place(&mut cw).ok()?;
        let mut data = [0u8; 12];
        data.copy_from_slice(&cw[..12]);
        Some(data)
    }

    /// Decode in place; returns the number of corrected symbols or `Err(())`.
    pub fn decode_in_place(&self, codeword: &mut [u8]) -> Result<usize, ()> {
        let n = codeword.len();
        let nroots = self.nroots;
        let f = &self.field;

        // 1. Syndromes.
        let mut synd = vec![0u8; nroots];
        let mut any = false;
        for (i, s) in synd.iter_mut().enumerate() {
            let mut acc = codeword[0];
            let root = (self.fcr + i) % PERIOD;
            for &c in &codeword[1..] {
                acc = if acc == 0 {
                    c
                } else {
                    c ^ f.alpha_to[(f.index_of[acc as usize] as usize + root) % PERIOD]
                };
            }
            *s = acc;
            if acc != 0 {
                any = true;
            }
        }
        if !any {
            return Ok(0);
        }

        // 2. Berlekamp–Massey → error locator Λ(x).
        let mut lambda = vec![0u8; nroots + 1];
        lambda[0] = 1;
        let mut b = vec![0u8; nroots + 1];
        b[0] = 1;
        let mut l = 0usize;
        let mut m = 1usize;
        let mut b_scalar = 1u8;
        for r in 0..nroots {
            let mut delta = synd[r];
            for i in 1..=l {
                if lambda[i] != 0 && synd[r - i] != 0 {
                    delta ^= f.mul(lambda[i], synd[r - i]);
                }
            }
            if delta == 0 {
                m += 1;
            } else if 2 * l <= r {
                let t = lambda.clone();
                let coef = f.mul(delta, f.inv(b_scalar));
                for i in 0..=nroots - m {
                    if b[i] != 0 {
                        lambda[i + m] ^= f.mul(coef, b[i]);
                    }
                }
                l = r + 1 - l;
                b = t;
                b_scalar = delta;
                m = 1;
            } else {
                let coef = f.mul(delta, f.inv(b_scalar));
                for i in 0..=nroots - m {
                    if b[i] != 0 {
                        lambda[i + m] ^= f.mul(coef, b[i]);
                    }
                }
                m += 1;
            }
        }

        let nerr = (0..=nroots).rev().find(|&i| lambda[i] != 0).unwrap_or(0);
        if nerr == 0 || nerr > nroots / 2 {
            return Err(());
        }

        // 3. Chien search.
        let mut err_pos = Vec::with_capacity(nerr);
        for p in 0..PERIOD {
            let xinv = (PERIOD - p % PERIOD) % PERIOD;
            let mut sum = lambda[0];
            for (i, &lam) in lambda.iter().enumerate().take(nroots + 1).skip(1) {
                if lam != 0 {
                    sum ^= f.alpha_to[(f.index_of[lam as usize] as usize + (i * xinv) % PERIOD) % PERIOD];
                }
            }
            if sum == 0 && p < n {
                err_pos.push(p);
            }
        }
        if err_pos.len() != nerr {
            return Err(());
        }

        // 4. Forney.
        let mut omega = vec![0u8; nroots];
        for (i, om) in omega.iter_mut().enumerate() {
            let mut acc = 0u8;
            for j in 0..=i {
                if lambda[j] != 0 && synd[i - j] != 0 {
                    acc ^= f.mul(lambda[j], synd[i - j]);
                }
            }
            *om = acc;
        }
        for &p in &err_pos {
            let xinv = (PERIOD - p % PERIOD) % PERIOD;
            let mut omega_val = 0u8;
            for (i, &w) in omega.iter().enumerate() {
                if w != 0 {
                    omega_val ^= f.alpha_to[(f.index_of[w as usize] as usize + (i * xinv) % PERIOD) % PERIOD];
                }
            }
            let mut lambda_prime = 0u8;
            let mut i = 1;
            while i <= nerr {
                if lambda[i] != 0 {
                    lambda_prime ^= f.alpha_to
                        [(f.index_of[lambda[i] as usize] as usize + ((i - 1) * xinv) % PERIOD) % PERIOD];
                }
                i += 2;
            }
            if lambda_prime == 0 {
                return Err(());
            }
            let mut mag = f.mul(omega_val, f.inv(lambda_prime));
            if mag != 0 {
                let exp = ((p as isize * (1 - self.fcr as isize)).rem_euclid(PERIOD as isize)) as usize;
                mag = f.alpha_to[(f.index_of[mag as usize] as usize + exp) % PERIOD];
            }
            codeword[n - 1 - p] ^= mag;
        }

        // 5. Verify.
        for i in 0..nroots {
            let mut acc = codeword[0];
            let root = (self.fcr + i) % PERIOD;
            for &c in &codeword[1..] {
                acc = if acc == 0 {
                    c
                } else {
                    c ^ f.alpha_to[(f.index_of[acc as usize] as usize + root) % PERIOD]
                };
            }
            if acc != 0 {
                return Err(());
            }
        }
        Ok(err_pos.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gf64_mul_has_identity_and_inverse_structure() {
        let f = Gf64::new();
        assert_eq!(f.mul(1, 37), 37);
        assert_eq!(f.mul(0, 37), 0);
        for a in 1u8..64 {
            let inv = f.inv(a);
            assert_eq!(f.mul(a, inv), 1, "inverse of {a}");
        }
    }

    #[test]
    fn rs_round_trips_clean() {
        let rs = RsGf64::jt65();
        let data = [3u8, 14, 1, 5, 9, 26, 53, 58, 0, 63, 12, 7];
        let cw: [u8; 63] = rs.encode(&data).try_into().unwrap();
        assert_eq!(rs.decode(&cw).unwrap(), data);
    }

    #[test]
    fn rs_corrects_up_to_t_errors() {
        let rs = RsGf64::jt65();
        let t = 51 / 2; // 25 correctable symbol errors
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let cw: [u8; 63] = rs.encode(&data).try_into().unwrap();
        let mut bad = cw;
        for k in 0..t {
            let pos = (k * 2 + 1) % 63;
            bad[pos] ^= ((k as u8).wrapping_mul(5) | 1) & 0x3F;
        }
        assert_eq!(rs.decode(&bad).unwrap(), data);
    }

    #[test]
    fn rs_reports_uncorrectable_beyond_capacity() {
        let rs = RsGf64::jt65();
        let data = [9u8, 8, 7, 6, 5, 4, 3, 2, 1, 0, 63, 62];
        let cw: [u8; 63] = rs.encode(&data).try_into().unwrap();
        let mut bad = cw;
        for pos in 0..30 {
            // 30 > t=25 errors
            bad[pos] ^= 0x15;
        }
        // Either flags uncorrectable, or (rarely) lands on a different valid
        // codeword — but it must not return the original data silently.
        match rs.decode(&bad) {
            None => {}
            Some(d) => assert_ne!(d, data),
        }
    }
}
