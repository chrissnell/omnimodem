//! THOR varicode (`fldigi/src/thor/thorvaricode.cxx`, upstream 4.1.23 @
//! 61b97f413).
//!
//! THOR carries a **primary** character set that is exactly the IZ8BLY MFSK
//! varicode ([`crate::framing::varicode::MFSK`], `varienc`/`varidec`), and a
//! **secondary** set (values `0x100..=0x1FF`) that reuses the otherwise-unused
//! 12-bit codes for a second live-text stream. Only the secondary table is new
//! here; it is transcribed verbatim from `thorvaricode.cxx` with the encode and
//! decode halves kept as one source of truth.
//!
//! Wire-determining data (both varicode tables, code values, bit orderings) is
//! asserted byte-for-byte against `tests/vectors/thor_varicode.json`
//! (`scratch/refvectors/build_thor.sh`).

use super::varicode::{mfsk_symbol_to_byte, MFSK};

/// The THOR secondary character set, one 12-bit codeword (MSB-first `'0'/'1'`)
/// per ASCII value `0x20..=0x7A` (`' '..'z'`). ref: thorvaricode.cxx:39-131
/// (`thor_varicode[]`).
pub static THOR_SECONDARY: [&str; 91] = [
    "101110000000", // 032 <SPC>
    "101110100000", // 033 !
    "101110101000", // 034 "
    "101110101100", // 035 #
    "101110110000", // 036 $
    "101110110100", // 037 %
    "101110111000", // 038 &
    "101110111100", // 039 '
    "101111000000", // 040 (
    "101111010000", // 041 )
    "101111010100", // 042 *
    "101111011000", // 043 +
    "101111011100", // 044 ,
    "101111100000", // 045 -
    "101111101000", // 046 .
    "101111101100", // 047 /
    "101111110000", // 048 0
    "101111110100", // 049 1
    "101111111000", // 050 2
    "101111111100", // 051 3
    "110000000000", // 052 4
    "110100000000", // 053 5
    "110101000000", // 054 6
    "110101010100", // 055 7
    "110101011000", // 056 8
    "110101011100", // 057 9
    "110101100000", // 058 :
    "110101101000", // 059 ;
    "110101101100", // 060 <
    "110101110000", // 061 =
    "110101110100", // 062 >
    "110101111000", // 063 ?
    "110101111100", // 064 @
    "110110000000", // 065 A
    "110110100000", // 066 B
    "110110101000", // 067 C
    "110110101100", // 068 D
    "110110110000", // 069 E
    "110110110100", // 070 F
    "110110111000", // 071 G
    "110110111100", // 072 H
    "110111000000", // 073 I
    "110111010000", // 074 J
    "110111010100", // 075 K
    "110111011000", // 076 L
    "110111011100", // 077 M
    "110111100000", // 078 N
    "110111101000", // 079 O
    "110111101100", // 080 P
    "110111110000", // 081 Q
    "110111110100", // 082 R
    "110111111000", // 083 S
    "110111111100", // 084 T
    "111000000000", // 085 U
    "111010000000", // 086 V
    "111010100000", // 087 W
    "111010101100", // 088 X
    "111010110000", // 089 Y
    "111010110100", // 090 Z
    "111010111000", // 091 [
    "111010111100", // 092 backslash
    "111011000000", // 093 ]
    "111011010000", // 094 ^
    "111011010100", // 095 _
    "111011011000", // 096 `
    "111011011100", // 097 a
    "111011100000", // 098 b
    "111011101000", // 099 c
    "111011101100", // 100 d
    "111011110000", // 101 e
    "111011110100", // 102 f
    "111011111000", // 103 g
    "111011111100", // 104 h
    "111100000000", // 105 i
    "111101000000", // 106 j
    "111101010000", // 107 k
    "111101010100", // 108 l
    "111101011000", // 109 m
    "111101011100", // 110 n
    "111101100000", // 111 o
    "111101101000", // 112 p
    "111101101100", // 113 q
    "111101110000", // 114 r
    "111101110100", // 115 s
    "111101111000", // 116 t
    "111101111100", // 117 u
    "111110000000", // 118 v
    "111110100000", // 119 w
    "111110101000", // 120 x
    "111110101100", // 121 y
    "111110110000", // 122 z
];

/// The value below which an accumulated codeword is a *primary* (MFSK) code;
/// at or above it is a THOR secondary code. ref: thorvaricode.cxx:185.
const SECONDARY_FLOOR: u32 = 0xB80;

/// The integer value of a `'0'/'1'` codeword string (MSB-first).
fn codeval(code: &str) -> u32 {
    code.bytes().fold(0u32, |acc, b| (acc << 1) | (b - b'0') as u32)
}

/// `thorvarienc(c, secondary)` as a `'0'/'1'` bit vector. Primary uses the MFSK
/// varicode; secondary uses the THOR table for `' '..'z'`, falling back to the
/// primary NUL codeword otherwise. ref: thorvaricode.cxx:170-179.
pub fn encode(c: u8, secondary: bool) -> Vec<u8> {
    let code = if !secondary {
        MFSK.code_for(c).unwrap_or("")
    } else if (b' '..=b'z').contains(&c) {
        THOR_SECONDARY[(c - b' ') as usize]
    } else {
        MFSK.code_for(0).unwrap_or("")
    };
    code.bytes().map(|b| b - b'0').collect()
}

/// `thorvaridec(sym)`: map an accumulated codeword value to its decoded value —
/// `0x00..=0xFF` for a primary character, `0x100..=0x1FF` for a secondary one —
/// or `None` if it matches no codeword. ref: thorvaricode.cxx:181-193.
pub fn decode(sym: u32) -> Option<u16> {
    if sym < SECONDARY_FLOOR {
        return mfsk_symbol_to_byte(sym).map(|b| b as u16);
    }
    THOR_SECONDARY
        .iter()
        .position(|&code| codeval(code) == sym)
        .map(|i| (b' ' as u16) + i as u16 + 0x100)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-exact: the THOR secondary varicode encode string, its integer value,
    /// and the round-trip decode reproduce fldigi's `thorvarienc`/`thorvaridec`
    /// byte-for-byte. Provenance: `tests/vectors/thor_varicode.json` (fldigi
    /// 4.1.23 @ 61b97f413, driver `scratch/refvectors/build_thor.sh`).
    #[test]
    fn secondary_matches_fldigi_vector() {
        let raw = include_str!("../../tests/vectors/thor_varicode.json");
        let line = raw.lines().find(|l| l.contains("\"secondary\"")).unwrap();
        // Each entry: {"c":32,"code":"101110000000","val":2944,"dec":288}
        let mut n = 0;
        for entry in line.split('{').skip(2) {
            let field = |k: &str| -> &str {
                let i = entry.find(k).unwrap() + k.len();
                &entry[i..i + entry[i..].find(['"', ',', '}']).unwrap()]
            };
            let c: u8 = field("\"c\":").parse().unwrap();
            let code = field("\"code\":\"");
            let val: u32 = field("\"val\":").parse().unwrap();
            let dec: u16 = field("\"dec\":").parse().unwrap();

            let bits = encode(c, true);
            let got_code: String = bits.iter().map(|b| (b + b'0') as char).collect();
            assert_eq!(got_code, code, "secondary encode for c={c}");
            assert_eq!(codeval(code), val, "codeval for c={c}");
            assert_eq!(decode(val), Some(dec), "secondary decode for val={val}");
            n += 1;
        }
        assert_eq!(n, 91, "expected the full ' '..'z' secondary table");
    }

    #[test]
    fn primary_uses_mfsk_and_decodes() {
        // Primary encode is the MFSK varicode; a value below the secondary floor
        // decodes back through the MFSK table.
        for c in [b'C', b'Q', b' ', b'e', 0u8] {
            let bits = encode(c, false);
            let sym = bits.iter().fold(0u32, |a, &b| (a << 1) | b as u32);
            assert!(sym < SECONDARY_FLOOR, "primary code for {c} must be < floor");
            assert_eq!(decode(sym), Some(c as u16));
        }
    }
}
