//! Soft-LLR demapper — the contract spine between detector and FEC.
//!
//! Locked convention (`types::Llr`): `L = ln(P(bit=0)/P(bit=1))`; positive ⇒
//! bit 0, hard slice `bit = (L < 0)`. Magnitudes scale with SNR so the
//! downstream BP/min-sum decoders weight reliable bits more.

use crate::types::Llr;

/// BPSK soft-symbol → LLR. `soft` is the matched-filter output projected onto
/// the BPSK axis where a *positive* value carries bit 0. With Gaussian noise
/// of variance `noise_var`, the exact bit LLR is `2·soft/noise_var`.
pub fn demap_bpsk(soft: f32, noise_var: f32) -> Llr {
    2.0 * soft / noise_var.max(f32::MIN_POSITIVE)
}

/// WSJT-X FT8/FT4 8-FSK Gray map (`kFT8_Gray_map` in `ft8_lib`): symbol value
/// `v` is transmitted on physical tone `FT8_GRAY_MAP[v]`. This is **not** the
/// binary-reflected Gray code — tones 4–7 differ — so FT8 must use this table,
/// not [`super::gray::gray_encode`], for bit-exact interoperability.
pub const FT8_GRAY_MAP: [u8; 8] = [0, 1, 3, 2, 5, 6, 4, 7];

/// Map an M-FSK symbol's per-tone powers to per-bit LLRs, using the standard
/// binary-reflected Gray layout (symbol `v` on tone `gray_encode(v)`).
///
/// `tone_powers[k]` is the (non-negative) energy in tone `k`; there are
/// `M = tone_powers.len()` tones carrying `log2(M)` bits. Bit ordering: bit 0
/// is the MSB of the symbol value, consistent with WSJT-X big-endian symbol
/// bits.
///
/// Each bit's LLR is the max-log-MAP approximation
/// `L_b = (max_{idx: bit=0} P_idx − max_{idx: bit=1} P_idx) / noise_var`,
/// so the sign follows the dominant tone's bit pattern and `|L|` grows with
/// the power gap (i.e. with SNR). For FT8/FT4 use [`demap_fsk_ft8`], whose tone
/// layout matches WSJT-X exactly.
pub fn demap_fsk(tone_powers: &[f32], noise_var: f32) -> Vec<Llr> {
    demap_fsk_with(tone_powers, noise_var, |idx| super::gray::gray_decode(idx as u32) as usize)
}

/// FT8/FT4 8-FSK soft demapper using the exact WSJT-X [`FT8_GRAY_MAP`]. Requires
/// exactly 8 tones (3 bits/symbol, MSB first).
pub fn demap_fsk_ft8(tone_powers: &[f32], noise_var: f32) -> Vec<Llr> {
    assert_eq!(tone_powers.len(), 8, "FT8/FT4 is 8-FSK");
    // Invert the symbol→tone map to recover the symbol value a tone carries.
    let mut tone_to_sym = [0usize; 8];
    for (sym, &tone) in FT8_GRAY_MAP.iter().enumerate() {
        tone_to_sym[tone as usize] = sym;
    }
    demap_fsk_with(tone_powers, noise_var, |idx| tone_to_sym[idx])
}

/// Max-log-MAP M-FSK demapper core, parametric in the tone→symbol-value map.
fn demap_fsk_with(tone_powers: &[f32], noise_var: f32, sym_of_tone: impl Fn(usize) -> usize) -> Vec<Llr> {
    let m = tone_powers.len();
    assert!(m >= 2 && m.is_power_of_two(), "M-FSK requires power-of-two tones ≥ 2");
    let bits = m.trailing_zeros() as usize;
    let nv = noise_var.max(f32::MIN_POSITIVE);
    let mut out = Vec::with_capacity(bits);
    for b in 0..bits {
        // bit b is bit (bits-1-b) of the symbol value (MSB first)
        let shift = bits - 1 - b;
        let mut max0 = f32::NEG_INFINITY;
        let mut max1 = f32::NEG_INFINITY;
        for (idx, &p) in tone_powers.iter().enumerate() {
            let sym = sym_of_tone(idx);
            if (sym >> shift) & 1 == 0 {
                max0 = max0.max(p);
            } else {
                max1 = max1.max(p);
            }
        }
        out.push((max0 - max1) / nv);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SoftBits;

    #[test]
    fn bpsk_hand_values() {
        // positive soft => positive LLR => bit 0
        assert_eq!(demap_bpsk(1.0, 1.0), 2.0);
        assert_eq!(demap_bpsk(0.5, 0.25), 4.0);
        assert!(demap_bpsk(-1.0, 1.0) < 0.0);
    }

    #[test]
    fn bpsk_magnitude_scales_with_snr() {
        // Lower noise variance => larger |LLR|.
        let lo = demap_bpsk(1.0, 1.0).abs();
        let hi = demap_bpsk(1.0, 0.1).abs();
        assert!(hi > lo);
    }

    #[test]
    fn fsk_sign_follows_dominant_tone() {
        // 4-FSK, 2 bits. Physical tone for symbol v is gray_encode(v).
        // Make symbol value 0b10 = 2 dominant: tone = gray_encode(2) = 3.
        let mut powers = [0.1f32; 4];
        powers[3] = 5.0; // tone 3 dominant => bits = gray_decode(3) = 2 = 0b10
        let llrs = demap_fsk(&powers, 1.0);
        // bit0 (MSB) should be 1 => negative; bit1 (LSB) should be 0 => positive
        assert!(llrs[0] < 0.0, "MSB llr {:?}", llrs[0]);
        assert!(llrs[1] > 0.0, "LSB llr {:?}", llrs[1]);
        assert_eq!(SoftBits(llrs).hard(), vec![1, 0]);
    }

    #[test]
    fn fsk_magnitude_scales_with_power_gap() {
        let weak = [1.0f32, 0.9, 0.1, 0.1];
        let strong = [5.0f32, 0.1, 0.1, 0.1];
        let lw = demap_fsk(&weak, 1.0);
        let ls = demap_fsk(&strong, 1.0);
        assert!(ls[0].abs() > lw[0].abs());
    }

    #[test]
    fn fsk_high_snr_hard_equals_tx_bits() {
        // For each symbol, place all power on its tone => hard slice recovers it.
        for sym in 0..4usize {
            let tone = super::super::gray::gray_encode(sym as u32) as usize;
            let mut powers = [0.0f32; 4];
            powers[tone] = 100.0;
            let llrs = demap_fsk(&powers, 0.01);
            let hard = SoftBits(llrs).hard();
            let got = ((hard[0] as usize) << 1) | hard[1] as usize;
            assert_eq!(got, sym, "sym {sym}");
        }
    }

    #[test]
    fn ft8_map_differs_from_binary_reflected_gray() {
        // The whole point of demap_fsk_ft8: tones 4–7 are NOT binary-reflected.
        let brg: [u8; 8] = std::array::from_fn(|v| super::super::gray::gray_encode(v as u32) as u8);
        assert_eq!(FT8_GRAY_MAP, [0, 1, 3, 2, 5, 6, 4, 7]);
        assert_ne!(FT8_GRAY_MAP, brg, "FT8 map must differ from binary-reflected Gray");
    }

    #[test]
    fn ft8_demap_recovers_symbol_via_wsjtx_map() {
        // For every 3-bit FT8 symbol, energy on its WSJT-X tone must decode back
        // to the symbol's bits (MSB first). Catches any tone↔symbol mismatch.
        for (sym, &tone_u8) in FT8_GRAY_MAP.iter().enumerate() {
            let tone = tone_u8 as usize;
            let mut powers = [0.0f32; 8];
            powers[tone] = 50.0;
            let hard = SoftBits(demap_fsk_ft8(&powers, 0.01)).hard();
            let got = ((hard[0] as usize) << 2) | ((hard[1] as usize) << 1) | hard[2] as usize;
            assert_eq!(got, sym, "FT8 sym {sym} on tone {tone}");
        }
    }
}
