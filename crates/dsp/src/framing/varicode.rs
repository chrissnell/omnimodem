//! PSK31 Varicode (Peter Martinez, G3PLX).
//!
//! Varicode is a prefix-free variable-length code where every codeword starts
//! and ends with a `1` and contains no two consecutive `0`s. Characters are
//! separated on the wire by `00` (two zero bits), which therefore cannot occur
//! inside any codeword. This gives self-synchronising, comma-free framing.
//!
//! Bit order: codewords are transmitted **most-significant-bit first** as
//! written in the canonical table (e.g. `'e'` = `11`). PSK31 sends the bits in
//! that left-to-right order; the inter-character `00` gap follows each char.
//!
//! The table is pluggable via [`VaricodeTable`] so MFSK/DominoEX-style nibble
//! varicodes can reuse the encoder/decoder. [`PSK31`] is the canonical table.

/// A prefix-free varicode mapping. Codewords are strings of `'0'`/`'1'`,
/// MSB-first, none containing `"00"`, separated on the wire by `"00"`.
pub struct VaricodeTable {
    /// `code[b]` is the codeword for byte value `b` (ASCII), MSB-first.
    code: [&'static str; 256],
}

impl VaricodeTable {
    /// Codeword for a byte, or `None` if unmapped.
    pub fn code_for(&self, b: u8) -> Option<&'static str> {
        let c = self.code[b as usize];
        if c.is_empty() {
            None
        } else {
            Some(c)
        }
    }
}

/// Encode text to a varicode bitstream (`Vec<u8>` of 0/1), inserting the `00`
/// inter-character separator after every character. Unmapped bytes are skipped.
pub fn encode(table: &VaricodeTable, text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in text.as_bytes() {
        if let Some(cw) = table.code_for(b) {
            for ch in cw.bytes() {
                out.push(ch - b'0');
            }
            out.push(0);
            out.push(0);
        }
    }
    out
}

/// Decode a `00`-delimited varicode bitstream back to text. Leading zeros and
/// the trailing separator are tolerated; unknown codewords are dropped.
pub fn decode(table: &VaricodeTable, bits: &[u8]) -> String {
    // Build a reverse lookup once.
    let mut out = String::new();
    let mut cur = String::new();
    let mut zeros = 0u8; // consecutive trailing zeros
    for &bit in bits {
        if bit == 0 {
            zeros += 1;
            if zeros >= 2 {
                // Character boundary: a codeword ends in `1`, so strip the two
                // separator zeros from the tail we accumulated.
                if !cur.is_empty() {
                    if let Some(c) = lookup(table, &cur) {
                        out.push(c);
                    }
                    cur.clear();
                }
                zeros = 0;
            }
        } else {
            // A single zero *inside* a codeword is part of it; a stray leading
            // zero (codeword buffer still empty, e.g. from an odd-length idle/
            // preamble run) is just separator residue and must be dropped, or it
            // corrupts the first codeword.
            if zeros == 1 && !cur.is_empty() {
                cur.push('0');
            }
            zeros = 0;
            cur.push('1');
        }
    }
    if !cur.is_empty() {
        if let Some(c) = lookup(table, &cur) {
            out.push(c);
        }
    }
    out
}

fn lookup(table: &VaricodeTable, code: &str) -> Option<char> {
    table
        .code
        .iter()
        .position(|&c| c == code)
        .map(|b| b as u8 as char)
}

/// The canonical PSK31 Varicode table (Peter Martinez, G3PLX).
pub static PSK31: VaricodeTable = VaricodeTable { code: build_psk31() };

const fn build_psk31() -> [&'static str; 256] {
    let mut t = [""; 256];
    // Control / whitespace.
    t[0x00] = "1010101011";
    t[0x01] = "1011011011";
    t[0x02] = "1011101101";
    t[0x03] = "1101110111";
    t[0x04] = "1011101011";
    t[0x05] = "1101011111";
    t[0x06] = "1011101111";
    t[0x07] = "1011111101";
    t[0x08] = "1011111111";
    t[0x09] = "11101111"; // HT
    t[0x0A] = "11101"; // LF
    t[0x0B] = "1101101111";
    t[0x0C] = "1011011101";
    t[0x0D] = "11111"; // CR
    t[0x0E] = "1101110101";
    t[0x0F] = "1110101011";
    t[0x10] = "1011110111";
    t[0x11] = "1011110101";
    t[0x12] = "1110101101";
    t[0x13] = "1110101111";
    t[0x14] = "1101011011";
    t[0x15] = "1101101011";
    t[0x16] = "1101101101";
    t[0x17] = "1101010111";
    t[0x18] = "1101111011";
    t[0x19] = "1101111101";
    t[0x1A] = "1110110111";
    t[0x1B] = "1101010101";
    t[0x1C] = "1101011101";
    t[0x1D] = "1110111011";
    t[0x1E] = "1011111011";
    t[0x1F] = "1101111111";
    t[0x20] = "1"; // space
    t[0x21] = "111111111"; // !
    t[0x22] = "101011111"; // "
    t[0x23] = "111110101"; // #
    t[0x24] = "111011011"; // $
    t[0x25] = "1011010101"; // %
    t[0x26] = "1010111011"; // &
    t[0x27] = "101111111"; // '
    t[0x28] = "11111011"; // (
    t[0x29] = "11110111"; // )
    t[0x2A] = "101101111"; // *
    t[0x2B] = "111011111"; // +
    t[0x2C] = "1110101"; // ,
    t[0x2D] = "110101"; // -
    t[0x2E] = "1010111"; // .
    t[0x2F] = "110101111"; // /
    t[0x30] = "10110111"; // 0
    t[0x31] = "10111101"; // 1
    t[0x32] = "11101101"; // 2
    t[0x33] = "11111111"; // 3
    t[0x34] = "101110111"; // 4
    t[0x35] = "101011011"; // 5
    t[0x36] = "101101011"; // 6
    t[0x37] = "110101101"; // 7
    t[0x38] = "110101011"; // 8
    t[0x39] = "110110111"; // 9
    t[0x3A] = "11110101"; // :
    t[0x3B] = "110111101"; // ;
    t[0x3C] = "111101101"; // <
    t[0x3D] = "1010101"; // =
    t[0x3E] = "111010111"; // >
    t[0x3F] = "1010101111"; // ?
    t[0x40] = "1010111101"; // @
    t[0x41] = "1111101"; // A
    t[0x42] = "11101011"; // B
    t[0x43] = "10101101"; // C
    t[0x44] = "10110101"; // D
    t[0x45] = "1110111"; // E
    t[0x46] = "11011011"; // F
    t[0x47] = "11111101"; // G
    t[0x48] = "101010101"; // H
    t[0x49] = "1111111"; // I
    t[0x4A] = "111111101"; // J
    t[0x4B] = "101111101"; // K
    t[0x4C] = "11010111"; // L
    t[0x4D] = "10111011"; // M
    t[0x4E] = "11011101"; // N
    t[0x4F] = "10101011"; // O
    t[0x50] = "11010101"; // P
    t[0x51] = "111011101"; // Q
    t[0x52] = "10101111"; // R
    t[0x53] = "1101111"; // S
    t[0x54] = "1101101"; // T
    t[0x55] = "101010111"; // U
    t[0x56] = "110110101"; // V
    t[0x57] = "101011101"; // W
    t[0x58] = "101110101"; // X
    t[0x59] = "101111011"; // Y
    t[0x5A] = "1010101101"; // Z
    t[0x5B] = "111110111"; // [
    t[0x5C] = "111101111"; // backslash
    t[0x5D] = "111111011"; // ]
    t[0x5E] = "1010111111"; // ^
    t[0x5F] = "101101101"; // _
    t[0x60] = "1011011111"; // `
    t[0x61] = "1011"; // a
    t[0x62] = "1011111"; // b
    t[0x63] = "101111"; // c
    t[0x64] = "101101"; // d
    t[0x65] = "11"; // e
    t[0x66] = "111101"; // f
    t[0x67] = "1011011"; // g
    t[0x68] = "101011"; // h
    t[0x69] = "1101"; // i
    t[0x6A] = "111101011"; // j
    t[0x6B] = "10111111"; // k
    t[0x6C] = "11011"; // l
    t[0x6D] = "111011"; // m
    t[0x6E] = "1111"; // n
    t[0x6F] = "111"; // o
    t[0x70] = "111111"; // p
    t[0x71] = "110111111"; // q
    t[0x72] = "10101"; // r
    t[0x73] = "10111"; // s
    t[0x74] = "101"; // t
    t[0x75] = "110111"; // u
    t[0x76] = "1111011"; // v
    t[0x77] = "1101011"; // w
    t[0x78] = "11011111"; // x
    t[0x79] = "1011101"; // y
    t[0x7A] = "111010101"; // z
    t[0x7B] = "1010110111"; // {
    t[0x7C] = "110111011"; // |
    t[0x7D] = "1010110101"; // }
    t[0x7E] = "1011010111"; // ~
    t[0x7F] = "1110110101"; // DEL
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e_is_canonical_codeword() {
        // PSK31 'e' is the shortest codeword, "11" (MSB-first).
        assert_eq!(PSK31.code_for(b'e'), Some("11"));
        let bits = encode(&PSK31, "e");
        // "11" + "00" separator, MSB-first.
        assert_eq!(bits, vec![1, 1, 0, 0]);
    }

    #[test]
    fn separator_never_inside_codeword() {
        // No codeword may contain "00".
        for b in 0u8..=127 {
            if let Some(cw) = PSK31.code_for(b) {
                assert!(!cw.contains("00"), "{b:#x} has 00");
                assert!(cw.starts_with('1') && cw.ends_with('1'), "{b:#x} edges");
            }
        }
    }

    #[test]
    fn roundtrip_printable_ascii() {
        let text: String = (0x20u8..=0x7E).map(|b| b as char).collect();
        let bits = encode(&PSK31, &text);
        assert_eq!(decode(&PSK31, &bits), text);
    }

    #[test]
    fn roundtrip_words() {
        for s in ["the quick brown fox", "CQ CQ de N0CALL", "hello, world! 12345"] {
            let bits = encode(&PSK31, s);
            assert_eq!(decode(&PSK31, &bits), s);
        }
    }

    #[test]
    fn odd_leading_zero_run_does_not_corrupt_first_char() {
        // A PSK31 idle is a run of zeros of arbitrary parity; a leftover single
        // zero before the first codeword must be dropped, not absorbed into it.
        let mut bits = vec![0u8; 5]; // odd-length leading idle
        bits.extend(encode(&PSK31, "CQ"));
        assert_eq!(decode(&PSK31, &bits), "CQ");
        let mut even = vec![0u8; 4];
        even.extend(encode(&PSK31, "CQ"));
        assert_eq!(decode(&PSK31, &even), "CQ");
    }
}
