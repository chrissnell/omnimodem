//! Shared M-FSK demod helpers for the windowed WSJT-X breadth modes (JT65/JT9/
//! WSPR) and the streaming MFSK modes. The transmit side is `frontend::modulate::
//! MFsk`; on receive each `sps`-sample symbol window is scored against every tone
//! with a Goertzel detector and the per-tone powers drive either a hard symbol
//! pick or a soft-LLR demap.
//!
//! Tone orthogonality: these modes choose `spacing_hz` so an integer number of
//! cycles separates adjacent tones over one symbol, so the Goertzel powers of
//! neighbouring tones are (near-)orthogonal and a clean signal decodes by argmax.

use crate::types::{Llr, Sample};
use std::f32::consts::TAU;

/// Goertzel power of `sig` at `freq` Hz (sample rate `rate`).
pub fn goertzel_power(sig: &[Sample], freq: f32, rate: f32) -> f32 {
    let w = TAU * freq / rate;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &x in sig {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// Per-tone Goertzel powers for one `sps`-sample symbol block at `window[off..]`.
/// Tone `k` sits at `base_hz + k * spacing_hz`.
pub fn tone_powers(
    block: &[Sample],
    rate: f32,
    base_hz: f32,
    spacing_hz: f32,
    tones: u32,
) -> Vec<f32> {
    (0..tones)
        .map(|k| goertzel_power(block, base_hz + k as f32 * spacing_hz, rate))
        .collect()
}

/// Detect the hard tone index (argmax power) for each of `n_symbols` consecutive
/// `sps`-sample blocks starting at sample `offset`.
#[allow(clippy::too_many_arguments)]
pub fn detect_symbols(
    window: &[Sample],
    offset: usize,
    sps: usize,
    n_symbols: usize,
    rate: f32,
    base_hz: f32,
    spacing_hz: f32,
    tones: u32,
) -> Vec<u32> {
    (0..n_symbols)
        .map(|i| {
            let start = offset + i * sps;
            let block = &window[start.min(window.len())..(start + sps).min(window.len())];
            let powers = tone_powers(block, rate, base_hz, spacing_hz, tones);
            argmax(&powers) as u32
        })
        .collect()
}

/// Index of the maximum value (0 if empty).
pub fn argmax(v: &[f32]) -> usize {
    let mut best = 0usize;
    let mut max = f32::NEG_INFINITY;
    for (i, &x) in v.iter().enumerate() {
        if x > max {
            max = x;
            best = i;
        }
    }
    best
}

/// Convert a hard symbol value (`0..2^bits`, MSB first) to `bits` strong LLRs
/// (positive ⇒ bit 0), suitable for feeding a soft FEC decoder from a clean
/// hard tone decision.
pub fn symbol_to_llrs(symbol: u32, bits: usize, strength: f32) -> Vec<Llr> {
    (0..bits)
        .map(|b| {
            let bit = (symbol >> (bits - 1 - b)) & 1;
            if bit == 0 {
                strength
            } else {
                -strength
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::modulate::MFsk;

    #[test]
    fn detects_clean_mfsk_symbols() {
        let (rate, baud, tones) = (8000.0f32, 31.25f32, 8u32);
        let sps = (rate / baud).round() as usize;
        let base = 500.0;
        let m = MFsk::new(rate, sps, base, baud, tones);
        let syms = [0u32, 7, 3, 1, 6, 2, 5, 4];
        let sig = m.modulate(&syms);
        let got = detect_symbols(&sig, 0, sps, syms.len(), rate, base, baud, tones);
        assert_eq!(got, syms);
    }

    #[test]
    fn symbol_to_llrs_sign_and_order() {
        // symbol 0b101 over 3 bits => bit0=1(neg), bit1=0(pos), bit2=1(neg)
        let l = symbol_to_llrs(0b101, 3, 4.0);
        assert_eq!(l, vec![-4.0, 4.0, -4.0]);
    }
}
