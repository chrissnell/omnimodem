//! Golay(23,12) and extended Golay(24,12). The (23,12) code is *perfect*: every
//! one of the 2^11 syndromes is the syndrome of exactly one error pattern of
//! weight ≤ 3, so a complete syndrome→coset-leader table gives an exact
//! 3-error-correcting decoder. The extended (24,12) code adds an overall parity
//! bit for 3-correct / 4-detect. Used by FreeDV 1600 and the M17 LICH. 12 data
//! bits live in the low bits of a `u16`.
//!
//! Reference for cross-checks: codec2 `golay23.c` / `libm17`.

use std::sync::OnceLock;

/// Golay(23,12) generator polynomial g(x)=x^11+x^10+x^6+x^5+x^4+x^2+1 (0xC75).
const GOLAY_POLY: u32 = 0xC75;

/// Encode 12 data bits → 23-bit codeword (data in high 12 bits, parity low 11).
pub fn encode23(data: u16) -> u32 {
    let msg = (data as u32 & 0xFFF) << 11;
    msg | parity11(msg)
}

/// Remainder of a 23-bit polynomial mod g(x): the low 11 bits (the syndrome of a
/// received word, or the parity of a message-shifted word).
fn parity11(mut rem: u32) -> u32 {
    for i in (11..23).rev() {
        if rem & (1 << i) != 0 {
            rem ^= GOLAY_POLY << (i - 11);
        }
    }
    rem & 0x7FF
}

fn syndrome(word: u32) -> u32 {
    parity11(word & 0x7F_FFFF)
}

/// Complete syndrome → minimal-weight error-pattern table (2048 entries). Built
/// once: enumerate every error pattern of weight ≤ 3 over 23 bits and index it
/// by its syndrome. Perfectness guarantees a 1:1 cover.
fn coset_leaders() -> &'static [u32; 2048] {
    static TABLE: OnceLock<[u32; 2048]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [u32::MAX; 2048];
        let set = |e: u32, t: &mut [u32; 2048]| {
            let s = syndrome(e) as usize;
            if t[s] == u32::MAX {
                t[s] = e;
            }
        };
        set(0, &mut t);
        for a in 0..23 {
            set(1 << a, &mut t);
            for b in (a + 1)..23 {
                set((1 << a) | (1 << b), &mut t);
                for c in (b + 1)..23 {
                    set((1 << a) | (1 << b) | (1 << c), &mut t);
                }
            }
        }
        debug_assert!(t.iter().all(|&e| e != u32::MAX), "perfect code must cover all syndromes");
        t
    })
}

/// Decode a (possibly corrupted) 23-bit word → (12 data bits, errors corrected).
/// Always succeeds for 23-bit input (the code is perfect); ≤3 errors are
/// corrected exactly, ≥4 errors decode to a wrong (but valid) codeword.
pub fn decode23(word: u32) -> Option<(u16, u32)> {
    let e = coset_leaders()[syndrome(word) as usize];
    let cw = word ^ e;
    Some((((cw >> 11) & 0xFFF) as u16, e.count_ones()))
}

/// Encode 12 data bits → 24-bit extended codeword: the 23-bit codeword in the
/// high bits plus an overall even-parity bit in bit 0.
pub fn encode24(data: u16) -> u32 {
    let cw23 = encode23(data);
    (cw23 << 1) | (cw23.count_ones() & 1)
}

/// Decode a 24-bit extended word. Corrects ≤3 errors; if the recomputed overall
/// parity disagrees after correction (a likely 4-error event), returns `None`.
pub fn decode24(word: u32) -> Option<(u16, u32)> {
    let cw23 = (word >> 1) & 0x7F_FFFF;
    let overall = word & 1;
    let (data, errs) = decode23(cw23)?;
    let corrected = encode23(data);
    // The transmitted overall parity covered the 23-bit codeword; verify it
    // against the corrected codeword to catch a fourth error.
    if (corrected.count_ones() & 1) == overall || errs < 3 {
        Some((data, errs))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_round_trip_23() {
        for d in [0u16, 1, 0xABC, 0xFFF, 0x555, 0x800, 0x123] {
            let cw = encode23(d);
            assert_eq!(syndrome(cw), 0, "valid codeword has zero syndrome");
            assert_eq!(decode23(cw).unwrap(), (d, 0));
        }
    }

    #[test]
    fn corrects_up_to_three_errors() {
        let d = 0xABC;
        let cw = encode23(d);
        for bits in [
            vec![5],
            vec![0, 22],
            vec![1, 11, 20],
            vec![3, 4, 5],
            vec![0, 1, 2],
        ] {
            let mut bad = cw;
            for &b in &bits {
                bad ^= 1 << b;
            }
            let (data, errs) = decode23(bad).unwrap();
            assert_eq!(data, d, "errors at {bits:?}");
            assert_eq!(errs as usize, bits.len());
        }
    }

    #[test]
    fn extended_round_trip_and_4_error_detect() {
        let d = 0x6C3;
        let cw = encode24(d);
        assert_eq!(decode24(cw).unwrap(), (d, 0));
        // 3 errors: corrected.
        let mut e3 = cw;
        for b in [2, 9, 17] {
            e3 ^= 1 << b;
        }
        assert_eq!(decode24(e3).unwrap().0, d);
    }
}
