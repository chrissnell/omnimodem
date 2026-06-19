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

/// Map an M-FSK symbol's per-tone powers to per-bit LLRs.
///
/// `tone_powers[k]` is the (non-negative) energy in tone `k`; there are
/// `M = tone_powers.len()` tones carrying `log2(M)` bits, **Gray-mapped** so
/// the index↔bits mapping matches `gray::gray_encode`. Bit ordering: bit 0 is
/// the MSB of the tone index, consistent with WSJT-X big-endian symbol bits.
///
/// Each bit's LLR is the max-log-MAP approximation
/// `L_b = (max_{idx: bit=0} P_idx − max_{idx: bit=1} P_idx) / noise_var`,
/// so the sign follows the dominant tone's bit pattern and `|L|` grows with
/// the power gap (i.e. with SNR).
pub fn demap_fsk(tone_powers: &[f32], noise_var: f32) -> Vec<Llr> {
    let m = tone_powers.len();
    assert!(m >= 2 && m.is_power_of_two(), "M-FSK requires power-of-two tones ≥ 2");
    let bits = m.trailing_zeros() as usize;
    let nv = noise_var.max(f32::MIN_POSITIVE);
    let mut out = Vec::with_capacity(bits);
    for b in 0..bits {
        // bit b is bit (bits-1-b) of the index (MSB first)
        let shift = bits - 1 - b;
        let mut max0 = f32::NEG_INFINITY;
        let mut max1 = f32::NEG_INFINITY;
        for (idx, &p) in tone_powers.iter().enumerate() {
            let sym = gray_index_to_bits(idx);
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

/// Tone index → the symbol-value bits it carries. Tones are laid out so that
/// symbol value `v` is sent on physical tone `gray_encode(v)`; inverting,
/// physical tone `idx` carries bits `gray_decode(idx)`.
fn gray_index_to_bits(idx: usize) -> usize {
    super::gray::gray_decode(idx as u32) as usize
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
}
