//! FST4 / FST4W mode assembly (WSJT-X LF/MF weak-signal), ported from
//! `wsjtx/lib/fst4/`. This module currently covers the tone-sequence assembly
//! (message bits → 160 4-GFSK tone indices); the windowed block demod + GFSK
//! waveform build on it. ref: wsjtx/lib/fst4/genfst4.f90, fst4_params.f90.

use crate::fec::ldpc_fst4::encode_240_101;

/// Total tone/symbol count: 40 sync + 120 data. ref: fst4_params.f90 (NN).
pub const FST4_NN: usize = 160;
/// Data symbols. ref: fst4_params.f90 (ND).
pub const FST4_ND: usize = 120;

/// The two 8-tone FST4 sync words. ref: genfst4.f90 (isyncword1/isyncword2).
const FST4_SYNC1: [u8; 8] = [0, 1, 3, 2, 1, 0, 2, 3];
const FST4_SYNC2: [u8; 8] = [2, 3, 1, 0, 3, 2, 0, 1];

/// Assemble the 160-tone FST4 frame (4-GFSK tone indices 0..3) from the 101
/// LDPC message bits (77-bit payload after rvec-scramble + 24-bit CRC). Mirrors
/// `genfst4`'s `get_fst4_tones_from_bits` entry: LDPC-encode to 240 bits,
/// Gray-map bit-pairs (00→0, 01→1, 11→2, 10→3), and interleave four 30-symbol
/// data blocks between five sync words: `s8 d30 s8 d30 s8 d30 s8 d30 s8`.
/// ref: genfst4.f90 (label 2 onward).
pub fn fst4_tones_from_msgbits(msgbits: &[u8; 101]) -> [u8; FST4_NN] {
    let cw = encode_240_101(msgbits);
    // 120 data symbols: is = cw[2i-1(MSB)] pair; then the Gray remap.
    let mut d = [0u8; FST4_ND];
    for (i, dt) in d.iter_mut().enumerate() {
        let is = cw[2 * i + 1] + 2 * cw[2 * i]; // ref: is=codeword(2*i)+2*codeword(2*i-1)
        *dt = match is {
            0 | 1 => is,
            2 => 3,
            3 => 2,
            _ => unreachable!("2-bit symbol"),
        };
    }
    let mut t = [0u8; FST4_NN];
    t[0..8].copy_from_slice(&FST4_SYNC1);
    t[8..38].copy_from_slice(&d[0..30]);
    t[38..46].copy_from_slice(&FST4_SYNC2);
    t[46..76].copy_from_slice(&d[30..60]);
    t[76..84].copy_from_slice(&FST4_SYNC1);
    t[84..114].copy_from_slice(&d[60..90]);
    t[114..122].copy_from_slice(&FST4_SYNC2);
    t[122..152].copy_from_slice(&d[90..120]);
    t[152..160].copy_from_slice(&FST4_SYNC1);
    t
}

/// Error function (Abramowitz & Stegun 7.1.26, |err| < 1.5e-7), f64 internally.
fn erf(x: f32) -> f32 {
    let x = x as f64;
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    (sign * y) as f32
}

/// FST4/WSJT-X GFSK frequency-deviation pulse (BT product `b`).
/// ref: wsjtx/lib/ft2/gfsk_pulse.f90.
fn gfsk_pulse(b: f32, t: f32) -> f32 {
    let c = std::f32::consts::PI * (2.0f32 / 2.0f32.ln()).sqrt();
    0.5 * (erf(c * b * (t + 0.5)) - erf(c * b * (t - 0.5)))
}

/// Generate the FST4 4-GFSK audio for a tone sequence. Ports gen_fst4wave.f90:
/// a BT=2.0 Gaussian frequency pulse 3 symbols wide (overlap-added), phase
/// integration through a 65536-entry sine table, base frequency `f0`, and a
/// quarter-symbol raised-cosine ramp up/down. FP-tolerance vs the reference.
/// `nsps` = samples/symbol (720..134400 by T/R period); `hmod` = mod index.
/// ref: wsjtx/lib/fst4/gen_fst4wave.f90.
pub fn fst4_gen_wave(itone: &[u8], nsps: usize, fsample: f32, hmod: u32, f0: f32) -> Vec<f32> {
    const NTAB: usize = 65536;
    let twopi = 2.0 * std::f32::consts::PI;
    let nsym = itone.len();
    let nwave = (nsym + 2) * nsps;
    let dt = 1.0 / fsample;
    let tsym = nsps as f32 / fsample;

    let mut pulse = vec![0.0f32; 3 * nsps];
    for (i, pp) in pulse.iter_mut().enumerate() {
        let tt = ((i + 1) as f32 - 1.5 * nsps as f32) / nsps as f32;
        *pp = gfsk_pulse(2.0, tt);
    }

    let mut dphi = vec![0.0f32; (nsym + 2) * nsps];
    let dphi_peak = twopi * hmod as f32 / nsps as f32;
    for (j, &tone) in itone.iter().enumerate() {
        let ib = j * nsps;
        for k in 0..3 * nsps {
            dphi[ib + k] += dphi_peak * pulse[k] * tone as f32;
        }
    }
    let shift = twopi * (f0 - 1.5 * hmod as f32 / tsym) * dt;
    for d in dphi.iter_mut() {
        *d += shift;
    }

    let mut wave = vec![0.0f32; nwave];
    let mut phi = 0.0f32;
    for k in 0..nsym * nsps {
        let idx = ((phi * NTAB as f32 / twopi) as i64 as usize) & (NTAB - 1);
        wave[k] = (idx as f32 * twopi / NTAB as f32).sin();
        phi += dphi[nsps + k];
        if phi > twopi {
            phi -= twopi;
        }
    }

    let q = nsps / 4;
    for (i, w) in wave.iter_mut().take(q).enumerate() {
        *w *= (1.0 - (twopi * i as f32 / (nsps as f32 / 2.0)).cos()) / 2.0;
    }
    let k1 = (nsym - 1) * nsps + 3 * nsps / 4;
    for i in 0..=q {
        wave[k1 + i] *= (1.0 + (twopi * i as f32 / (nsps as f32 / 2.0)).cos()) / 2.0;
    }
    wave
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden tone sequence from the UNMODIFIED genfst4 `get_fst4_tones_from_bits`
    /// (msgbits bit = 1 where i % 3 == 0). ref: scratch/refvectors/build_fst4_tones.sh.
    const REF_TONES: &str = "0132102331031031031031031031031031031023103201310310310310310310311222003011013210230001103112000110131122220032302310320122322213221230222022020232223201321023";

    #[test]
    fn fst4_tone_assembly_matches_wsjtx_reference() {
        let mut msgbits = [0u8; 101];
        for (i, b) in msgbits.iter_mut().enumerate() {
            *b = u8::from(i % 3 == 0);
        }
        let tones = fst4_tones_from_msgbits(&msgbits);
        let want: Vec<u8> = REF_TONES.bytes().map(|c| c - b'0').collect();
        assert_eq!(want.len(), FST4_NN, "reference must be 160 tones");
        assert_eq!(tones.to_vec(), want, "FST4 tone sequence differs from genfst4");
    }

    /// Golden GFSK wave from the UNMODIFIED gen_fst4wave (nsym=4, nsps=16,
    /// itone=0,1,2,3). ref: scratch/refvectors/build_fst4_wave.sh.
    const REF_WAVE: [f32; 96] = [
    0.00000000, 0.02857032, 0.19134173, 0.47420889, 0.70710677, 0.83146966, 0.92387950, 0.98076659,
    1.00000000, 0.98080397, 0.92391616, 0.83146954, 0.70710677, 0.55485255, 0.37105003, 0.11574722,
    -0.27317473, -0.71593177, -0.98095328, -0.92387944, -0.55565000, -0.00009567, 0.55549055, 0.92384285,
    0.98080397, 0.70717454, 0.19518432, -0.38259488, -0.83146977, -0.99999964, -0.82442641, -0.30730224,
    0.45559573, 0.98315811, 0.70649642, -0.19509049, -0.92384297, -0.83152270, -0.00009567, 0.83141637,
    0.92391616, 0.19518432, -0.70703912, -0.98080397, -0.38268343, 0.55628753, 0.99992114, 0.48704773,
    -0.62050855, -0.91900045, 0.19593655, 1.00000000, 0.19518432, -0.92384297, -0.55565000, 0.70703900,
    0.83152288, -0.38259488, -0.98080397, -0.00009567, 0.98074788, 0.32890743, -0.40494755, -0.12529999,
    0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000,
    0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000,
    0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000,
    0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000, 0.00000000,
    ];

    #[test]
    fn fst4_gfsk_wave_matches_wsjtx_reference() {
        let wave = fst4_gen_wave(&[0, 1, 2, 3], 16, 12000.0, 1, 1500.0);
        assert_eq!(wave.len(), 96);
        let maxerr = wave
            .iter()
            .zip(REF_WAVE.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(maxerr < 1e-3, "GFSK wave max abs error {maxerr} exceeds tolerance");
    }

    #[test]
    fn fst4_sync_words_land_in_the_frame() {
        let tones = fst4_tones_from_msgbits(&[0u8; 101]);
        assert_eq!(&tones[0..8], &FST4_SYNC1);
        assert_eq!(&tones[38..46], &FST4_SYNC2);
        assert_eq!(&tones[152..160], &FST4_SYNC1);
        assert!(tones.iter().all(|&t| t < 4), "tones must be 0..3");
    }
}
