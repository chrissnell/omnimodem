//! JS8's LDPC(174,87) code + CRC-12, ported from js8call (upstream
//! `js8call/js8call` @ a7ff1be).
//!
//! JS8 reuses the **early FT8** channel code: a regular column-weight-3 PEG
//! LDPC(174,87) carrying a 75-bit message + 12-bit CRC (`KK=87`), *not* the
//! current FT8 (174,91)/CRC-14. The encoder (`encode174.f90`) forms
//! `[parity(87) | message(87)]` and then permutes the 174 columns by
//! `COLORDER`; the belief-propagation Tanner graph (`bpdecode174.f90`) operates
//! on that permuted codeword. We replicate the encoder bit-exact and build an
//! [`Ldpc`] whose generator rows are the permuted codewords of the unit
//! messages, so the existing BP + OSD decoders work unchanged.
//!
//! ref: js8call/lib/ft8/{encode174.f90, ldpc_174_87_params.f90, bpdecode174.f90}.
//! Tables live verbatim in [`super::js8_tables`].

use super::js8_tables::{JS8_174_87_COLORDER, JS8_174_87_GEN, JS8_174_87_MN, JS8_174_87_NM, JS8_174_87_NRW};
use super::ldpc::Ldpc;

// The bits→checks table `MN` is reference data our BP path does not consume
// directly (it works from `NM`/`check_vars`); it is retained verbatim for the
// `Mn`/`Nm` cross-consistency KAT and any future decoder. Anchor it at compile
// time so it stays live and shape-checked.
const _: () = assert!(JS8_174_87_MN.len() == N);

/// Codeword length.
pub const N: usize = 174;
/// Message length (75 message + 12 CRC bits).
pub const K: usize = 87;
/// Parity checks (`M = N - K`).
pub const M: usize = 87;

/// Unpack the generator hex table into the dense `M×K` parity matrix
/// `gen_matrix[i][j]` (parity bit `i` ← message bit `j`), exactly as
/// `encode174.f90` fills `gen(M,K)`: 11 bytes per row, 8 bits per byte
/// MSB-first, keeping the first 87 columns. ref: encode174.f90:22-33.
fn gen_matrix() -> [[u8; K]; M] {
    let mut g = [[0u8; K]; M];
    for (i, row) in JS8_174_87_GEN.iter().enumerate() {
        let bytes = row.as_bytes();
        for j in 0..11 {
            let hi = hex_val(bytes[j * 2]);
            let lo = hex_val(bytes[j * 2 + 1]);
            let istr = (hi << 4) | lo; // one byte, MSB-first
            for jj in 1..=8 {
                let icol = j * 8 + jj; // 1-origin column
                if icol <= 87 && (istr >> (8 - jj)) & 1 == 1 {
                    g[i][icol - 1] = 1;
                }
            }
        }
    }
    g
}

fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("bad hex nibble"),
    }
}

/// `encode174` — encode an 87-bit message into the 174-bit codeword, bit-exact
/// with the reference. `itmp = [parity(87) | message(87)]`, then
/// `codeword[COLORDER[i]] = itmp[i]`. ref: encode174.f90:35-46.
pub fn encode174(message: &[u8; K]) -> [u8; N] {
    let g = gen_matrix();
    let mut itmp = [0u8; N];
    for (i, gi) in g.iter().enumerate() {
        let mut nsum = 0u32;
        for j in 0..K {
            nsum += (message[j] & 1) as u32 * gi[j] as u32;
        }
        itmp[i] = (nsum % 2) as u8; // pchecks(i)
    }
    itmp[M..N].copy_from_slice(&message[..K]); // itmp(M+1:N) = message
    let mut codeword = [0u8; N];
    for i in 0..N {
        codeword[JS8_174_87_COLORDER[i]] = itmp[i];
    }
    codeword
}

/// 0-origin codeword positions holding the 87 message bits, i.e.
/// `COLORDER[M..N]`. `decoded[m] = codeword[MESSAGE_POS[m]]`.
pub fn message_positions() -> [usize; K] {
    let mut p = [0usize; K];
    p.copy_from_slice(&JS8_174_87_COLORDER[M..N]);
    p
}

/// Extract the 87 message bits from a (permuted) codeword.
pub fn extract_message(codeword: &[u8]) -> [u8; K] {
    let pos = message_positions();
    let mut m = [0u8; K];
    for (i, &p) in pos.iter().enumerate() {
        m[i] = codeword[p] & 1;
    }
    m
}

/// The JS8 LDPC(174,87) code as an [`Ldpc`]: generator rows are the permuted
/// codewords of the unit messages (so `Ldpc::encode == encode174`), and the
/// Tanner graph comes from `NM`/`NRW` (1-origin → 0-origin). Reuses the shared
/// BP min-sum + OSD decoders.
pub fn js8_174_87_code() -> Ldpc {
    // Generator row j = encode174(e_j).
    let mut gen = vec![vec![0u8; N]; K];
    for (j, row) in gen.iter_mut().enumerate() {
        let mut e = [0u8; K];
        e[j] = 1;
        let cw = encode174(&e);
        row.copy_from_slice(&cw);
    }
    // Tanner graph: check c covers the NM[c][0..NRW[c]] codeword vars (1-origin).
    let mut check_vars = vec![Vec::new(); M];
    for (c, vars) in check_vars.iter_mut().enumerate() {
        for &v in JS8_174_87_NM[c].iter().take(JS8_174_87_NRW[c] as usize) {
            vars.push(v as usize - 1);
        }
    }
    Ldpc::from_generator_and_checks(K, gen, check_vars)
}

/// CRC-12 used by JS8's message CRC, matching boost `augmented_crc<12, 0xc06>`:
/// MSB-first polynomial division of the byte buffer (which already carries the
/// 12-bit CRC slot as trailing zeros) by `x^12 + POLY`, initial remainder 0, no
/// reflection, no final XOR. `genjs8.f90` then XORs the result with 42.
///
/// ref: js8call/lib/crc12.cpp (`POLY 0xc06`), genjs8.f90:35 (`crc12` + `xor 42`).
/// The final on-air authority for this transcription is the Task-5 cross-decode
/// gate (boost is unavailable here to capture a native golden CRC).
pub const JS8_CRC12_POLY: u16 = 0xc06;

pub fn augmented_crc12(bytes: &[u8]) -> u16 {
    let mut rem: u16 = 0;
    for &b in bytes {
        for bit in (0..8).rev() {
            let msb = (rem >> 11) & 1;
            let inbit = ((b >> bit) & 1) as u16;
            rem = ((rem << 1) | inbit) & 0x0FFF;
            if msb == 1 {
                rem ^= JS8_CRC12_POLY;
            }
        }
    }
    rem & 0x0FFF
}

/// JS8 message CRC: `augmented_crc12(bytes) XOR 42`. ref: genjs8.f90:35.
pub fn js8_crc12(bytes: &[u8]) -> u16 {
    augmented_crc12(bytes) ^ 42
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_from_str(s: &str) -> Vec<u8> {
        s.bytes().map(|c| c - b'0').collect()
    }

    fn ldpc_vector() -> (Vec<u8>, Vec<u8>) {
        let raw = include_str!("../../tests/vectors/js8_ldpc.json");
        let field = |k: &str| -> String {
            let i = raw.find(k).unwrap() + k.len();
            raw[i..raw[i..].find('"').unwrap() + i].to_string()
        };
        (bits_from_str(&field("\"msgbits\": \"")), bits_from_str(&field("\"codeword\": \"")))
    }

    /// Bit-exact: `encode174` reproduces the reference encoder's 174-bit codeword.
    /// Provenance: `tests/vectors/js8_ldpc.json` (js8call @ a7ff1be, driver
    /// `scratch/refvectors/js8/build_ldpc.sh`, pure-Fortran `encode174.f90`).
    #[test]
    fn encode174_matches_reference() {
        let (msgbits, codeword) = ldpc_vector();
        let mut m = [0u8; K];
        m.copy_from_slice(&msgbits);
        let cw = encode174(&m);
        assert_eq!(cw.to_vec(), codeword, "encode174 differs from reference codeword");
    }

    /// The generator and the Tanner graph (`NM`) agree: every encoded codeword
    /// satisfies all parity checks. Confirms the two independent reference
    /// tables (generator vs `Nm`) describe the same code.
    #[test]
    fn encoder_and_parity_checks_agree() {
        let code = js8_174_87_code();
        assert_eq!((code.n(), code.k()), (174, 87));
        // Reference vector message.
        let (msgbits, codeword) = ldpc_vector();
        let mut m = [0u8; K];
        m.copy_from_slice(&msgbits);
        // Ldpc::encode == encode174, and both satisfy parity.
        assert_eq!(code.encode(&m), codeword);
        assert_eq!(code.parity_errors(&codeword), 0, "reference codeword fails parity");
        // A few more deterministic messages.
        for seed in 0u32..8 {
            let mut msg = [0u8; K];
            for (j, b) in msg.iter_mut().enumerate() {
                *b = (((j as u32).wrapping_mul(2654435761).wrapping_add(seed)) >> 13 & 1) as u8;
            }
            let cw = encode174(&msg);
            assert_eq!(code.encode(&msg), cw.to_vec());
            assert_eq!(code.parity_errors(&cw), 0);
            // Message extraction round-trips.
            assert_eq!(extract_message(&cw).to_vec(), msg.to_vec());
        }
    }

    /// The message bits live at `COLORDER[87..174]`; extraction inverts the
    /// encoder's column permutation.
    #[test]
    fn message_extraction_roundtrips() {
        let (msgbits, _cw) = ldpc_vector();
        let mut m = [0u8; K];
        m.copy_from_slice(&msgbits);
        assert_eq!(extract_message(&encode174(&m)).to_vec(), msgbits);
    }

    /// The shared BP min-sum + OSD decoders recover the message through the
    /// JS8 code, confirming it is decode-ready (used by the RX path). Clean
    /// codeword and a few flipped bits are both corrected.
    #[test]
    fn bp_and_osd_decode_recover_message() {
        use crate::fec::osd::osd_decode;
        use crate::types::Llr;
        let code = js8_174_87_code();
        let (msgbits, _cw) = ldpc_vector();
        let mut msg = [0u8; K];
        msg.copy_from_slice(&msgbits);
        let cw = encode174(&msg);
        // LLR convention: positive ⇒ bit 0. Flip 3 bits to exercise correction.
        let mut llrs: Vec<Llr> = cw.iter().map(|&b| if b == 0 { 4.0 } else { -4.0 }).collect();
        for &p in &[5usize, 50, 120] {
            llrs[p] = -llrs[p];
        }
        let (dec, perr) = code.decode_minsum(&llrs, 30);
        let dec = if perr == 0 { dec } else { osd_decode(&code, &llrs, 2).expect("osd basis") };
        assert_eq!(code.parity_errors(&dec), 0, "decoded codeword must satisfy parity");
        assert_eq!(extract_message(&dec).to_vec(), msgbits, "recovered message mismatch");
    }

    /// The two reference Tanner-graph representations agree: bit `v` appears in
    /// check `c` per `MN` iff check `c` lists bit `v` per `NM`. Cross-validates
    /// the transcription of both tables. ref: bpdecode174.f90 `Mn`/`Nm`.
    #[test]
    fn mn_and_nm_are_consistent() {
        // Build Nm as a set of (check, bit) pairs.
        let mut from_nm = std::collections::HashSet::new();
        for (c, row) in JS8_174_87_NM.iter().enumerate() {
            for &v in row.iter().take(JS8_174_87_NRW[c] as usize) {
                from_nm.insert((c, v as usize - 1));
            }
        }
        // Build the same set from Mn (each bit lists its 3 checks).
        let mut from_mn = std::collections::HashSet::new();
        for (v, checks) in JS8_174_87_MN.iter().enumerate() {
            for &c in checks {
                from_mn.insert((c as usize - 1, v));
            }
        }
        assert_eq!(from_nm, from_mn, "Mn and Nm Tanner graphs disagree");
        // Column weight 3 (regular code): each bit in exactly 3 checks.
        assert!(JS8_174_87_MN.iter().all(|c| c.len() == 3));
    }

    /// CRC-12 is 12 bits, deterministic, and the `xor 42` is applied.
    #[test]
    fn crc12_is_12_bits_and_xored() {
        let a = augmented_crc12(b"\x01\x02\x03\x00\x00");
        assert!(a < (1 << 12), "CRC-12 must fit in 12 bits");
        assert_eq!(js8_crc12(b"\x01\x02\x03\x00\x00"), a ^ 42);
        // All-zero augmented message → zero remainder.
        assert_eq!(augmented_crc12(&[0u8; 11]), 0);
    }

    /// Bit-exact vs the reference `crc12.cpp` (boost `augmented_crc<12,0xc06>`).
    /// Golden values from `scratch/refvectors/js8/crc/crc_dump.cpp` (js8call @
    /// a7ff1be). Confirms both the augmented-CRC transcription and the `xor 42`.
    #[test]
    fn crc12_matches_boost_reference() {
        let cases: &[([u8; 11], u16, u16)] = &[
            ([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 0, 42),
            ([1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0], 280, 306),
            ([0xAB, 0xCD, 0xEF, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0, 0], 412, 438),
            ([0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x11, 0x80, 0], 2636, 2662),
        ];
        for (buf, raw, xor42) in cases {
            assert_eq!(augmented_crc12(buf), *raw, "augmented_crc12 mismatch for {buf:02x?}");
            assert_eq!(js8_crc12(buf), *xor42, "js8_crc12 mismatch for {buf:02x?}");
        }
    }
}
