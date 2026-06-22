//! Soft Walsh–Hadamard / Fast-Hadamard-Transform block codec. A `log2(n)`-bit
//! symbol selects one of `n` orthogonal Walsh sequences; the soft decoder runs
//! the FHT over the received soft sequence and picks the max-correlation index.
//! Parametric size: n=64 (Olivia), n=32 (Contestia). Walsh sequences are in
//! natural (Hadamard) order — confirm against fldigi `olivia.cxx` when wiring
//! the mode (it may apply a sequency permutation).

/// Encode a symbol in `0..n` to its length-`n` ±1 Walsh sequence (natural order).
pub fn walsh_encode(symbol: usize, n: usize) -> Vec<i8> {
    debug_assert!(n.is_power_of_two() && symbol < n);
    (0..n)
        .map(|i| if ((symbol & i).count_ones() & 1) == 0 { 1i8 } else { -1 })
        .collect()
}

/// In-place fast Hadamard transform on `f32` (natural order). Self-inverse up to
/// a factor of `n`: `fht(fht(x)) == n * x`.
pub fn fht(a: &mut [f32]) {
    let n = a.len();
    debug_assert!(n.is_power_of_two());
    let mut h = 1;
    while h < n {
        let mut i = 0;
        while i < n {
            for j in i..i + h {
                let x = a[j];
                let y = a[j + h];
                a[j] = x + y;
                a[j + h] = x - y;
            }
            i += 2 * h;
        }
        h *= 2;
    }
}

/// Soft-decode a received length-`n` soft sequence to the most likely symbol and
/// its correlation magnitude (a soft confidence). Positive soft value ⇒ Walsh
/// +1 chip. The FHT of the received ±1 sequence has its peak at the transmitted
/// symbol index; the peak's sign reflects an overall inversion, so we take the
/// max by absolute value.
pub fn walsh_soft_decode(soft: &[f32]) -> (usize, f32) {
    let mut work = soft.to_vec();
    fht(&mut work);
    let mut best = 0usize;
    let mut mag = 0.0f32;
    for (i, &v) in work.iter().enumerate() {
        if v.abs() > mag {
            mag = v.abs();
            best = i;
        }
    }
    (best, mag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fht_is_self_inverse_up_to_scale() {
        let orig = [1.0f32, -2.0, 3.0, 0.5, -1.0, 4.0, 0.0, 2.5];
        let mut a = orig;
        fht(&mut a);
        fht(&mut a);
        for (i, &v) in a.iter().enumerate() {
            assert!((v - 8.0 * orig[i]).abs() < 1e-4, "idx {i}: {v}");
        }
    }

    #[test]
    fn clean_symbol_round_trips_for_n64() {
        let n = 64;
        for sym in [0usize, 1, 17, 63] {
            let chips = walsh_encode(sym, n);
            let soft: Vec<f32> = chips.iter().map(|&c| c as f32).collect();
            let (got, mag) = walsh_soft_decode(&soft);
            assert_eq!(got, sym, "symbol {sym}");
            assert!(mag > 0.0);
        }
    }

    #[test]
    fn walsh_sequences_are_orthogonal() {
        let n = 32;
        for a in [0usize, 3, 9, 31] {
            for b in [0usize, 3, 9, 31] {
                let wa = walsh_encode(a, n);
                let wb = walsh_encode(b, n);
                let dot: i32 = wa.iter().zip(&wb).map(|(&x, &y)| x as i32 * y as i32).sum();
                if a == b {
                    assert_eq!(dot, n as i32);
                } else {
                    assert_eq!(dot, 0, "{a} vs {b}");
                }
            }
        }
    }

    #[test]
    fn noisy_symbol_still_decodes() {
        let n = 32;
        let chips = walsh_encode(5, n);
        let soft: Vec<f32> = chips
            .iter()
            .enumerate()
            .map(|(i, &c)| c as f32 * 0.6 + if i % 7 == 0 { -0.5 } else { 0.2 })
            .collect();
        assert_eq!(walsh_soft_decode(&soft).0, 5);
    }
}
