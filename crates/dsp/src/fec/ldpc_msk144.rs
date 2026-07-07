//! MSK144 LDPC(128,90) code + CRC-13, ported from WSJT-X `wsjtx/lib/`.
//!
//! MSK144 protects a 77-bit message with a 13-bit CRC (→ 90 information bits)
//! and a (128,90) LDPC code (38 parity bits). The systematic generator below is
//! transcribed verbatim from `ldpc_128_90_generator.f90`; [`encode_128_90`]
//! replicates `encode_128_90.f90` bit-for-bit (KAT vs the unmodified reference
//! encoder). [`get_crc13`] reproduces the boost `augmented_crc<13,0x15D7>` that
//! `crc13.cpp` uses, and [`msk144_code`] exposes the code as an [`Ldpc`] for the
//! belief-propagation decoder (sparse Tanner graph from
//! `ldpc_128_90_reordered_parity.f90`).
//!
//! ref: wsjtx/lib/{encode_128_90.f90, ldpc_128_90_generator.f90,
//! ldpc_128_90_reordered_parity.f90, crc13.cpp, genmsk_128_90.f90}
//! (WSJTX/wsjtx @ ccdfaf3c1c109010d15399674ce278167cfde848).

use super::ldpc::Ldpc;

/// (128,90) code dimensions.
pub const MSK144_N: usize = 128;
pub const MSK144_K: usize = 90;
const MSK144_M: usize = MSK144_N - MSK144_K; // 38 parity checks

/// CRC-13 truncated polynomial (implicit x^13). ref: crc13.cpp `#define POLY 0x15D7`.
const CRC13_POLY: u16 = 0x15D7;

/// Compute the MSK144 CRC-13 over a 77-bit message, matching boost
/// `augmented_crc<13,0x15D7>` as called by `encode_128_90.f90`: the 77 message
/// bits are packed MSB-first into a 12-byte (96-bit) buffer, the trailing 19
/// bits zero, and divided bit-serially by the polynomial with no reflection and
/// no final XOR. Returns the 13-bit remainder in the low bits.
/// ref: encode_128_90.f90:37-43 + crc13.cpp.
pub fn get_crc13(msgbits: &[u8; 77]) -> u16 {
    let mut rem: u16 = 0;
    // 96-bit augmented message: 77 payload bits followed by 19 zero bits.
    let mut process = |bit: u8| {
        let top = (rem >> 12) & 1;
        rem = ((rem << 1) | (bit as u16 & 1)) & 0x1FFF;
        if top == 1 {
            rem ^= CRC13_POLY;
        }
    };
    for &b in msgbits.iter() {
        process(b);
    }
    for _ in 0..19 {
        process(0);
    }
    rem & 0x1FFF
}

/// Append the CRC-13 to a 77-bit message, yielding the 90 information bits the
/// LDPC encoder consumes. ref: encode_128_90.f90:37-44.
pub fn msgbits_128_90(msg77: &[u8; 77]) -> [u8; MSK144_K] {
    let mut m = [0u8; MSK144_K];
    m[..77].copy_from_slice(msg77);
    let crc = get_crc13(msg77);
    for i in 0..13 {
        m[77 + i] = ((crc >> (12 - i)) & 1) as u8;
    }
    m
}

/// The (128,90) systematic generator, 38 rows of 23 hex chars. Each hex char
/// contributes 4 bits MSB-first, except the 23rd, which contributes 2 bits
/// (90 columns total). ref: wsjtx/lib/ldpc_128_90_generator.f90 (`g(38)`).
const G_128_90: [&str; MSK144_M] = [
    "a08ea80879050a5e94da994",
    "59f3b48040ca089c81ee880",
    "e4070262802e31b7b17d3dc",
    "95cbcbaf032dc3d960bacc8",
    "c4d79b5dcc21161a254ffbc",
    "93fde9cdbf2622a70868424",
    "e73b888bb1b01167379ba28",
    "45a0d0a0f39a7ad2439949c",
    "759acef19444bcad79c4964",
    "71eb4dddf4f5ed9e2ea17e0",
    "80f0ad76fb247d6b4ca8d38",
    "184fff3aa1b82dc66640104",
    "ca4e320bb382ed14cbb1094",
    "52514447b90e25b9e459e28",
    "dd10c1666e071956bd0df38",
    "99c332a0b792a2da8ef1ba8",
    "7bd9f688e7ed402e231aaac",
    "00fcad76eb647d6a0ca8c38",
    "6ac8d0499c43b02eed78d70",
    "2c2c764baf795b4788db010",
    "0e907bf9e280d2624823dd0",
    "b857a6e315afd8c1c925e64",
    "8deb58e22d73a141cae3778",
    "22d3cb80d92d6ac132dfe08",
    "754763877b28c187746855c",
    "1d1bb7cf6953732e04ebca4",
    "2c65e0ea4466ab9f5e1deec",
    "6dc530ca37fc916d1f84870",
    "49bccbbee152355be7ac984",
    "e8387f3f4367cf45a150448",
    "8ce25e03d67d51091c81884",
    "b798012ffa40a93852752c8",
    "2e43307933adfca37adc3c8",
    "ca06e0a42ca1ec782d6c06c",
    "c02b762927556a7039e638c",
    "4a3e9b7d08b6807f8619fac",
    "45e8030f68997bb68544424",
    "7e79362c16773efc6482e30",
];

/// Unpack one generator row's hex string into its `MSK144_K` parity-contribution
/// bits. ref: encode_128_90.f90:22-33 — nibble j, bit (4-jj), MSB-first, with the
/// 23rd (last) nibble contributing only 2 bits.
fn unpack_gen_row(hex: &str) -> [u8; MSK144_K] {
    let mut bits = [0u8; MSK144_K];
    let mut col = 0usize;
    for (j, ch) in hex.chars().enumerate() {
        let nib = ch.to_digit(16).expect("generator hex digit") as u8;
        let ibmax = if j == 22 { 2 } else { 4 };
        for jj in 1..=ibmax {
            bits[col] = (nib >> (4 - jj)) & 1; // btest(istr, 4-jj)
            col += 1;
        }
    }
    debug_assert_eq!(col, MSK144_K);
    bits
}

/// The dense (38×90) parity matrix `p` such that `parity = p · message`.
/// Row `c` is generator row `c`. ref: encode_128_90.f90:46-52.
fn parity_matrix() -> Vec<Vec<u8>> {
    G_128_90.iter().map(|h| unpack_gen_row(h).to_vec()).collect()
}

/// Systematic encode of the 90 information bits into the 128-bit MSK144
/// codeword: `codeword = [message(90) | parity(38)]`, `parity(c) = Σ_j
/// message(j)·gen(c,j) mod 2`. Bit-exact vs `encode_128_90.f90`.
pub fn encode_128_90(message: &[u8; MSK144_K]) -> [u8; MSK144_N] {
    let mut cw = [0u8; MSK144_N];
    cw[..MSK144_K].copy_from_slice(message);
    for (c, hex) in G_128_90.iter().enumerate() {
        let row = unpack_gen_row(hex);
        let mut sum = 0u8;
        for j in 0..MSK144_K {
            sum ^= message[j] & row[j];
        }
        cw[MSK144_K + c] = sum & 1;
    }
    cw
}

/// Full TX encode: 77 message bits → 128-bit codeword (CRC-13 appended, then
/// LDPC). ref: genmsk_128_90.f90:87-88 (`encode_128_90(msgbits,codeword)`).
pub fn encode_msk144(msg77: &[u8; 77]) -> [u8; MSK144_N] {
    encode_128_90(&msgbits_128_90(msg77))
}

/// Sparse Tanner graph: for each of the 38 checks, the 1-based codeword-variable
/// indices (0 = pad), and the valid count per check. Data fills column `c` of
/// `Nm(11,38)`. ref: wsjtx/lib/ldpc_128_90_reordered_parity.f90 (`Nm`, `nrw`).
const NM_128_90: [[u16; 11]; MSK144_M] = [
    [2, 15, 27, 40, 53, 65, 77, 91, 94, 115, 0],
    [3, 6, 28, 41, 54, 66, 78, 92, 94, 120, 0],
    [4, 16, 29, 42, 55, 67, 77, 90, 93, 106, 118],
    [5, 17, 30, 43, 52, 64, 79, 92, 102, 119, 0],
    [6, 18, 31, 44, 56, 68, 80, 89, 95, 108, 125],
    [7, 14, 32, 45, 57, 68, 79, 90, 96, 116, 121],
    [4, 19, 33, 43, 58, 69, 81, 97, 107, 121, 0],
    [2, 20, 30, 39, 54, 70, 80, 98, 107, 128, 0],
    [3, 21, 34, 46, 59, 67, 79, 99, 107, 123, 0],
    [8, 15, 29, 47, 56, 71, 82, 100, 111, 128, 0],
    [9, 22, 34, 44, 52, 72, 83, 101, 103, 126, 0],
    [10, 17, 26, 48, 60, 73, 84, 91, 110, 121, 0],
    [7, 23, 35, 38, 55, 73, 82, 101, 109, 120, 0],
    [11, 19, 36, 49, 53, 70, 85, 102, 104, 126, 0],
    [10, 20, 37, 46, 58, 71, 85, 105, 109, 122, 0],
    [5, 23, 37, 47, 57, 74, 86, 93, 110, 125, 0],
    [12, 13, 27, 41, 61, 68, 87, 97, 109, 113, 0],
    [11, 16, 38, 45, 58, 72, 78, 99, 108, 115, 0],
    [4, 22, 31, 41, 60, 74, 82, 105, 112, 115, 0],
    [12, 24, 32, 39, 62, 63, 88, 99, 102, 118, 0],
    [1, 19, 25, 45, 62, 75, 77, 100, 112, 119, 0],
    [6, 25, 33, 50, 59, 71, 83, 98, 117, 118, 0],
    [10, 16, 39, 51, 53, 66, 83, 95, 111, 124, 0],
    [9, 24, 35, 42, 59, 76, 89, 94, 114, 122, 0],
    [7, 17, 37, 50, 54, 75, 88, 111, 114, 123, 0],
    [11, 25, 35, 48, 61, 65, 88, 105, 116, 125, 0],
    [9, 21, 32, 49, 55, 69, 84, 86, 95, 113, 119],
    [2, 18, 28, 47, 63, 73, 87, 96, 106, 126, 0],
    [12, 26, 31, 49, 64, 72, 81, 100, 106, 120, 0],
    [13, 15, 28, 48, 51, 76, 85, 93, 103, 123, 0],
    [8, 20, 36, 44, 57, 75, 78, 91, 113, 117, 0],
    [5, 21, 29, 40, 51, 70, 81, 96, 117, 122, 0],
    [8, 26, 34, 40, 62, 74, 80, 92, 116, 127, 0],
    [1, 13, 14, 36, 42, 64, 66, 84, 101, 108, 0],
    [14, 22, 27, 50, 63, 69, 89, 104, 110, 128, 0],
    [1, 18, 30, 46, 60, 65, 90, 97, 114, 127, 0],
    [3, 23, 33, 52, 56, 76, 87, 104, 112, 124, 0],
    [24, 38, 43, 61, 67, 86, 98, 103, 124, 127, 0],
];

/// Valid variable count per check. ref: ldpc_128_90_reordered_parity.f90 (`nrw`).
const NRW_128_90: [u8; MSK144_M] = [
    10, 10, 11, 10, 11, 11, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
    10, 10, 11, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
];

/// The MSK144 (128,90) LDPC code: systematic generator (TX encode) + the sparse
/// reordered parity Tanner graph (RX belief-propagation decode).
pub fn msk144_code() -> Ldpc {
    let p = parity_matrix();
    let mut check_vars = vec![Vec::new(); MSK144_M];
    for (c, vars) in check_vars.iter_mut().enumerate() {
        for &v in NM_128_90[c].iter().take(NRW_128_90[c] as usize) {
            vars.push(v as usize - 1); // 1-based → 0-based
        }
    }
    Ldpc::from_systematic_sparse(MSK144_K, &p, check_vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(s: &str) -> Vec<u8> {
        s.bytes().map(|b| b - b'0').collect()
    }

    // Golden vectors from the UNMODIFIED wsjtx encoder + real boost crc13,
    // captured by scratch/refvectors/msk144/build_msk144.sh (see
    // tests/vectors/msk144_reference.json). Pattern: msg bit i = 1 where i%3==0.
    const REF_MSG77: &str =
        "10010010010010010010010010010010010010010010010010010010010010010010010010010";
    const REF_CRC13: &str = "0001111101000";
    const REF_CW128: &str = "10010010010010010010010010010010010010010010010010010010010010010010010010010000111110100001010000101010111101111001110100100000";

    #[test]
    fn crc13_matches_boost_reference() {
        let mut m = [0u8; 77];
        for (i, b) in m.iter_mut().enumerate() {
            *b = u8::from(i % 3 == 0);
        }
        let crc = get_crc13(&m);
        let want = u16::from_str_radix(REF_CRC13, 2).unwrap();
        assert_eq!(crc, want, "CRC-13 differs from boost augmented_crc<13,0x15D7>");
    }

    #[test]
    fn encode_128_90_matches_wsjtx_reference() {
        let msg77: [u8; 77] = bits(REF_MSG77).try_into().unwrap();
        let cw = encode_msk144(&msg77);
        let want = bits(REF_CW128);
        assert_eq!(cw.to_vec(), want, "codeword differs from encode_128_90.f90");
    }

    #[test]
    fn encode_128_90_matches_second_reference_pattern() {
        // Second golden pattern (pattern_pn in msk144_reference.json).
        let msg = "10110001110100101100011101001011000111010010110001110100101100011101001011001";
        let cw_ref = "10110001110100101100011101001011000111010010110001110100101100011101001011001100011101001100111011110100001110110011011101100011";
        let msg77: [u8; 77] = bits(msg).try_into().unwrap();
        let cw = encode_msk144(&msg77);
        assert_eq!(cw.to_vec(), bits(cw_ref));
    }

    #[test]
    fn codeword_carries_message_then_crc() {
        let msg77: [u8; 77] = bits(REF_MSG77).try_into().unwrap();
        let cw = encode_msk144(&msg77);
        // codeword(1:77) = message, codeword(78:90) = CRC-13.
        assert_eq!(&cw[..77], &msg77[..]);
        assert_eq!(&cw[77..90], &bits(REF_CRC13)[..]);
    }

    #[test]
    fn generator_and_parity_are_consistent() {
        // Every systematic codeword satisfies the sparse parity checks (G·Hᵀ=0).
        let code = msk144_code();
        for seed in 0u64..16 {
            let mut msg = [0u8; MSK144_K];
            let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1);
            for b in msg.iter_mut() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = (x & 1) as u8;
            }
            let cw = encode_128_90(&msg);
            assert_eq!(code.parity_errors(&cw), 0, "encoder codeword fails parity");
        }
    }

    #[test]
    fn ldpc_corrects_errors_at_high_snr() {
        use crate::types::Llr;
        let code = msk144_code();
        let mut msg = [0u8; MSK144_K];
        for (i, b) in msg.iter_mut().enumerate() {
            *b = u8::from((i * 7 + 3) % 5 < 2);
        }
        let cw = encode_128_90(&msg);
        // High-confidence LLRs with a few confident flips.
        let mut llr: Vec<Llr> = cw.iter().map(|&b| if b == 0 { 6.0 } else { -6.0 }).collect();
        for &flip in &[3usize, 40, 100] {
            llr[flip] = -llr[flip];
        }
        let (hard, errs) = code.decode_minsum(&llr, 50);
        assert_eq!(errs, 0, "BP left unsatisfied checks");
        assert_eq!(&hard[..MSK144_K], &msg[..], "BP failed to recover message");
    }

    #[test]
    fn crc13_self_checks_to_zero() {
        // Augmented CRC property: recomputing over message+CRC padded gives the
        // CRC back; here we assert the appended CRC matches a fresh computation.
        let mut m = [0u8; 77];
        for (i, b) in m.iter_mut().enumerate() {
            *b = u8::from((i * 5 + 1) % 7 < 3);
        }
        let full = msgbits_128_90(&m);
        let mut crc = 0u16;
        for &b in &full[77..90] {
            crc = (crc << 1) | b as u16;
        }
        assert_eq!(crc, get_crc13(&m));
    }
}
