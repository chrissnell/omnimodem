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

/// The four FST4 tone frequencies (Hz) for the given base `f0`, mod index and
/// baud. Tone `t` (0..3) sits at `f0 + hmod*baud*(t - 1.5)`. ref: gen_fst4wave
/// frequency layout (dphi_peak + the `-1.5*hmod/tsym` base shift).
fn fst4_tone_freqs(f0: f32, hmod: u32, baud: f32) -> [f32; 4] {
    let mut f = [0.0f32; 4];
    for (t, ft) in f.iter_mut().enumerate() {
        *ft = f0 + hmod as f32 * baud * (t as f32 - 1.5);
    }
    f
}

/// Non-coherent per-symbol tone magnitudes: for each of `FST4_NN` symbols,
/// correlate its `nsps`-sample window against the four tone frequencies and
/// return `|correlation|`. This is the front end for both hard-tone recovery
/// and soft-LLR demapping.
fn fst4_symbol_tone_mags(wave: &[f32], nsps: usize, fsample: f32, hmod: u32, f0: f32) -> Vec<[f32; 4]> {
    use crate::types::Cplx;
    let baud = fsample / nsps as f32;
    let freqs = fst4_tone_freqs(f0, hmod, baud);
    let twopi = 2.0 * std::f32::consts::PI;
    let mut out = Vec::with_capacity(FST4_NN);
    for s in 0..FST4_NN {
        let mut mags = [0.0f32; 4];
        for (t, &ft) in freqs.iter().enumerate() {
            let w = twopi * ft / fsample;
            let mut acc = Cplx::new(0.0, 0.0);
            for k in 0..nsps {
                let n = s * nsps + k;
                if n >= wave.len() {
                    break;
                }
                acc += Cplx::new(0.0, -(w * n as f32)).exp() * wave[n];
            }
            mags[t] = acc.norm();
        }
        out.push(mags);
    }
    out
}

/// Soft-decode the FST4 audio to 240 codeword-bit LLRs (`L = ln P(0)/P(1)`,
/// positive ⇒ bit 0) ready for [`crate::fec::ldpc_fst4::fst4_240_101_code`].
/// Skips the five sync words, un-Gray-maps each data tone into its (MSB, LSB)
/// bit-pair LLRs via a max-log metric over the four tone magnitudes.
/// ref: genfst4.f90 (Gray map) inverted; frame `s8 d30 s8 d30 s8 d30 s8 d30 s8`.
pub fn fst4_demod_soft(wave: &[f32], nsps: usize, fsample: f32, hmod: u32, f0: f32) -> Vec<f32> {
    let mags = fst4_symbol_tone_mags(wave, nsps, fsample, hmod, f0);
    // The 120 data-symbol positions, in codeword order (sync words removed).
    // Frame: s8 d30 s8 d30 s8 d30 s8 d30 s8.
    let data_ranges = [8usize..38, 46..76, 84..114, 122..152];
    let mut llrs = vec![0.0f32; 2 * FST4_ND];
    let mut di = 0usize;
    for r in data_ranges {
        for sym in r {
            let m = &mags[sym];
            // Gray: tone0=(msb0,lsb0) tone1=(msb0,lsb1) tone2=(msb1,lsb1) tone3=(msb1,lsb0).
            // MSB=0 → {0,1}, MSB=1 → {2,3};  LSB=0 → {0,3}, LSB=1 → {1,2}.
            let msb = m[0].max(m[1]) - m[2].max(m[3]);
            let lsb = m[0].max(m[3]) - m[1].max(m[2]);
            llrs[2 * di] = msb; // codeword bit 2i   (MSB)
            llrs[2 * di + 1] = lsb; // codeword bit 2i+1 (LSB)
            di += 1;
        }
    }
    llrs
}


/// FST4 24-bit CRC generator polynomial 0x100065b, as the 25-bit array the
/// reference divides by. ref: wsjtx/lib/fst4/get_crc24.f90 (`data p`).
const FST4_CRC24_P: [u8; 25] = [
    1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 1, 1,
];

/// The 77-bit `rvec` pseudo-random scramble XORed into the FST4 payload before
/// the CRC. ref: wsjtx/lib/fst4/genfst4.f90 (`data rvec`).
const FST4_RVEC: [u8; 77] = [
    0, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 1, 1, 1, 1, 0, 1, 0, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 0, 1, 0, 0,
    1, 0, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1,
    1, 0, 1, 1, 1, 1, 1, 0, 0, 0, 1, 0, 1,
];

/// FST4 24-bit CRC over the first `len` bits of `mc` (bit-serial division by the
/// 0x100065b polynomial). To generate, pass a length-`len` array whose last 24
/// bits are zero; to check, pass message+CRC and expect 0. ref: get_crc24.f90.
pub fn fst4_crc24(mc: &[u8], len: usize) -> u32 {
    let mut r = [0u8; 25];
    r.copy_from_slice(&mc[0..25]);
    for i in 0..=(len - 25) {
        r[24] = mc[i + 24]; // mc(i+25), 1-origin
        if r[0] == 1 {
            for (rk, pk) in r.iter_mut().zip(FST4_CRC24_P.iter()) {
                *rk ^= *pk;
            }
        }
        let first = r[0]; // cshift(r, 1): rotate left
        for k in 0..24 {
            r[k] = r[k + 1];
        }
        r[24] = first;
    }
    let mut crc = 0u32; // r(1:24), MSB first
    for &b in r.iter().take(24) {
        crc = (crc << 1) | b as u32;
    }
    crc
}

/// Assemble the 101 FST4 LDPC message bits from a 77-bit packed payload:
/// XOR the `rvec` scramble into the payload, then append its 24-bit CRC.
/// ref: genfst4.f90 (main path: `msgbits(1:77)=payload+rvec`, CRC-24 in 78:101).
pub fn fst4_msgbits_from_payload(payload: &[u8; 77]) -> [u8; 101] {
    let mut mc = [0u8; 101];
    for i in 0..77 {
        mc[i] = (payload[i] ^ FST4_RVEC[i]) & 1;
    }
    let crc = fst4_crc24(&mc, 101); // last 24 bits still zero here
    for i in 0..24 {
        mc[77 + i] = ((crc >> (23 - i)) & 1) as u8;
    }
    mc
}

/// Recover the 77-bit payload from decoded 101 message bits: strip the CRC and
/// undo the `rvec` scramble. Inverse of the payload step in
/// [`fst4_msgbits_from_payload`]. ref: genfst4.f90 (msgbits(1:77)=payload+rvec).
pub fn fst4_payload_from_msgbits(msgbits: &[u8; 101]) -> [u8; 77] {
    let mut payload = [0u8; 77];
    for (i, p) in payload.iter_mut().enumerate() {
        *p = (msgbits[i] ^ FST4_RVEC[i]) & 1;
    }
    payload
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
    fn fst4_crc24_matches_wsjtx_reference() {
        // Fixed pattern: first 77 bits = 1,1,0,0 repeating; last 24 zero.
        let mut mc = [0u8; 101];
        for (i, b) in mc.iter_mut().take(77).enumerate() {
            *b = u8::from(i % 4 < 2);
        }
        assert_eq!(fst4_crc24(&mc, 101), 7_450_690, "CRC-24 differs from get_crc24");
    }

    #[test]
    fn fst4_msgbits_crc_is_self_consistent() {
        // A payload -> msgbits(101) whose appended CRC checks to zero, and whose
        // scramble is involutive (rvec twice returns the payload).
        let mut payload = [0u8; 77];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = u8::from((i * 5 + 3) % 7 < 3);
        }
        let mc = fst4_msgbits_from_payload(&payload);
        assert_eq!(fst4_crc24(&mc, 101), 0, "assembled CRC must self-check to 0");
        for i in 0..77 {
            assert_eq!(mc[i] ^ FST4_RVEC[i], payload[i], "rvec must be recoverable");
        }
    }

    #[test]
    fn fst4_text_to_air_to_text_loopback() {
        use crate::fec::ldpc_fst4::fst4_240_101_code;
        use crate::framing::pack77::{pack77_standard, unpack77_standard};
        let nsps = 720;
        let fsample = 12000.0;
        let hmod = 1;
        let baud = fsample / nsps as f32;
        let f0 = 1500.0 + 1.5 * hmod as f32 * baud;
        let code = fst4_240_101_code();
        for msg in ["CQ K1ABC FN42", "K1ABC W9XYZ RR73", "W9XYZ K1ABC -11"] {
            // TX: message -> 77-bit payload -> msgbits -> tones -> wave.
            let payload = pack77_standard(msg).unwrap();
            let msgbits = fst4_msgbits_from_payload(&payload);
            let tones = fst4_tones_from_msgbits(&msgbits);
            let wave = fst4_gen_wave(&tones, nsps, fsample, hmod, f0);
            // RX: wave -> soft demap -> LDPC decode -> payload -> message.
            let llrs = fst4_demod_soft(&wave, nsps, fsample, hmod, f0);
            let (hard, errs) = code.decode_minsum(&llrs, 80);
            assert_eq!(errs, 0, "{msg}: LDPC parity unsatisfied");
            let mut rx_bits = [0u8; 101];
            rx_bits.copy_from_slice(&hard[..101]);
            let rx_payload = fst4_payload_from_msgbits(&rx_bits);
            assert_eq!(
                unpack77_standard(&rx_payload).as_deref(),
                Some(msg),
                "text round-trip failed for {msg}"
            );
        }
    }

    #[test]
    fn fst4_full_loopback_recovers_message() {
        use crate::fec::ldpc_fst4::fst4_240_101_code;
        // A representative 101-bit message (payload+CRC bit domain).
        let mut msgbits = [0u8; 101];
        for (i, b) in msgbits.iter_mut().enumerate() {
            *b = u8::from((i * 3 + 1) % 5 < 2);
        }
        let nsps = 720; // TR = 15 s
        let fsample = 12000.0;
        let hmod = 1;
        let baud = fsample / nsps as f32;
        let f0 = 1500.0 + 1.5 * hmod as f32 * baud; // center the 4-tone cluster near 1500 Hz

        let tones = fst4_tones_from_msgbits(&msgbits);
        let wave = fst4_gen_wave(&tones, nsps, fsample, hmod, f0);

        // Front end recovers every transmitted tone (hard) at high SNR.
        let mags = fst4_symbol_tone_mags(&wave, nsps, fsample, hmod, f0);
        for (s, m) in mags.iter().enumerate() {
            let hard = (0..4).max_by(|&a, &b| m[a].partial_cmp(&m[b]).unwrap()).unwrap() as u8;
            assert_eq!(hard, tones[s], "tone {s} misdetected");
        }

        // Soft demap + LDPC decode recovers the message bit-for-bit.
        let llrs = fst4_demod_soft(&wave, nsps, fsample, hmod, f0);
        let (hard, errs) = fst4_240_101_code().decode_minsum(&llrs, 50);
        assert_eq!(errs, 0, "LDPC left unsatisfied checks after loopback");
        assert_eq!(&hard[..101], &msgbits[..], "loopback did not recover the message");
    }

    #[test]
    fn fst4_decodes_under_awgn() {
        use crate::fec::ldpc_fst4::fst4_240_101_code;
        use crate::testutil::{add_awgn, Rng};
        let mut msgbits = [0u8; 101];
        for (i, b) in msgbits.iter_mut().enumerate() {
            *b = u8::from((i * 3 + 1) % 5 < 2);
        }
        let nsps = 720; // TR = 15 s
        let fsample = 12000.0;
        let hmod = 1;
        let baud = fsample / nsps as f32;
        let f0 = 1500.0 + 1.5 * hmod as f32 * baud;
        let tones = fst4_tones_from_msgbits(&msgbits);
        let wave = fst4_gen_wave(&tones, nsps, fsample, hmod, f0);
        let code = fst4_240_101_code();
        // The windowed mode integrates nsps=720 samples/symbol, so it tolerates
        // deep noise: sigma=4.0 is noise 4x the signal amplitude per sample.
        let trials = 30;
        let mut ok = 0;
        for trial in 0..trials as u64 {
            let mut w = wave.clone();
            let mut rng = Rng::new(0xF574_0000 + trial);
            add_awgn(&mut w, 4.0, &mut rng);
            let llrs = fst4_demod_soft(&w, nsps, fsample, hmod, f0);
            let (hard, errs) = code.decode_minsum(&llrs, 80);
            if errs == 0 && hard[..101] == msgbits[..] {
                ok += 1;
            }
        }
        let rate = ok as f32 / trials as f32;
        assert!(rate >= 0.9, "FST4 AWGN decode rate {rate} below 0.9");
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
