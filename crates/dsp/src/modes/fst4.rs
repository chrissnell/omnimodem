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

    #[test]
    fn fst4_sync_words_land_in_the_frame() {
        let tones = fst4_tones_from_msgbits(&[0u8; 101]);
        assert_eq!(&tones[0..8], &FST4_SYNC1);
        assert_eq!(&tones[38..46], &FST4_SYNC2);
        assert_eq!(&tones[152..160], &FST4_SYNC1);
        assert!(tones.iter().all(|&t| t < 4), "tones must be 0..3");
    }
}
