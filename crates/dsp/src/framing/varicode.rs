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


/// The IZ8BLY MFSK Varicode (as used by fldigi's PSK-R / +F "robust" modes).
///
/// Unlike PSK31 Varicode this is *self-framing* — codewords are concatenated
/// with **no** inter-character separator, and the receiver reframes with a shift
/// register: after each bit, when the low three bits are `0b001` the codeword is
/// `shreg >> 1` and the register resets to `1`. ref: fldigi
/// src/mfsk/mfskvaricode.cxx:35-292 (varicode[]), src/psk/psk.cxx:1117-1123
/// (rx_bit framing), 2467-2489 (tx_char: no separator for the `_pskr` path).
pub static MFSK: VaricodeTable = VaricodeTable { code: build_mfsk() };

const fn build_mfsk() -> [&'static str; 256] {
    [
        "11101011100", "11101100000", "11101101000", "11101101100", "11101110000", "11101110100", "11101111000", "11101111100",
        "10101000", "11110000000", "11110100000", "11110101000", "11110101100", "10101100", "11110110000", "11110110100",
        "11110111000", "11110111100", "11111000000", "11111010000", "11111010100", "11111011000", "11111011100", "11111100000",
        "11111101000", "11111101100", "11111110000", "11111110100", "11111111000", "11111111100", "100000000000", "101000000000",
        "100", "111000000", "111111100", "1011011000", "1010101000", "1010100000", "1000000000", "110111100",
        "111110100", "111110000", "1010110100", "111100000", "10100000", "111011000", "111010100", "111101000",
        "11100000", "11110000", "101000000", "101010100", "101110100", "101100000", "101101100", "110100000",
        "110000000", "110101100", "111101100", "111111000", "1011000000", "111011100", "1010111100", "111010000",
        "1010000000", "10111100", "100000000", "11010100", "11011100", "10111000", "11111000", "101010000",
        "101011000", "11000000", "110110100", "101111100", "11110100", "11101000", "11111100", "11010000",
        "11101100", "110110000", "11011000", "10110100", "10110000", "101011100", "110101000", "101101000",
        "101110000", "101111000", "110111000", "1011101000", "1011010000", "1011101100", "1011010100", "1010110000",
        "1010101100", "10100", "1100000", "111000", "110100", "1000", "1010000", "1011000",
        "110000", "11000", "10000000", "1110000", "101100", "1000000", "11100", "10000",
        "1010100", "1111000", "100000", "101000", "1100", "111100", "1101100", "1101000",
        "1110100", "1011100", "1111100", "1011011100", "1010111000", "1011100000", "1011110000", "101010000000",
        "101010100000", "101010101000", "101010101100", "101010110000", "101010110100", "101010111000", "101010111100", "101011000000",
        "101011010000", "101011010100", "101011011000", "101011011100", "101011100000", "101011101000", "101011101100", "101011110000",
        "101011110100", "101011111000", "101011111100", "101100000000", "101101000000", "101101010000", "101101010100", "101101011000",
        "101101011100", "101101100000", "101101101000", "101101101100", "101101110000", "101101110100", "101101111000", "101101111100",
        "1011110100", "1011111000", "1011111100", "1100000000", "1101000000", "1101010000", "1101010100", "1101011000",
        "1101011100", "1101100000", "1101101000", "1101101100", "1101110000", "1101110100", "1101111000", "1101111100",
        "1110000000", "1110100000", "1110101000", "1110101100", "1110110000", "1110110100", "1110111000", "1110111100",
        "1111000000", "1111010000", "1111010100", "1111011000", "1111011100", "1111100000", "1111101000", "1111101100",
        "1111110000", "1111110100", "1111111000", "1111111100", "10000000000", "10100000000", "10101000000", "10101010000",
        "10101010100", "10101011000", "10101011100", "10101100000", "10101101000", "10101101100", "10101110000", "10101110100",
        "10101111000", "10101111100", "10110000000", "10110100000", "10110101000", "10110101100", "10110110000", "10110110100",
        "10110111000", "10110111100", "10111000000", "10111010000", "10111010100", "10111011000", "10111011100", "10111100000",
        "10111101000", "10111101100", "10111110000", "10111110100", "10111111000", "10111111100", "11000000000", "11010000000",
        "11010100000", "11010101000", "11010101100", "11010110000", "11010110100", "11010111000", "11010111100", "11011000000",
        "11011010000", "11011010100", "11011011000", "11011011100", "11011100000", "11011101000", "11011101100", "11011110000",
        "11011110100", "11011111000", "11011111100", "11100000000", "11101000000", "11101010000", "11101010100", "11101011000",
    ]
}

/// The integer value of a `0`/`1` codeword string (MSB-first), for the MFSK
/// reframing reverse lookup. `varidecode[i] == mfsk_codeval(varicode[i])` in
/// fldigi, so one table suffices.
fn mfsk_codeval(code: &str) -> u32 {
    code.bytes().fold(0u32, |acc, b| (acc << 1) | (b - b'0') as u32)
}

/// Encode text to the self-framed MFSK Varicode bitstream — each character's
/// codeword bits concatenated, no separator (fldigi `tx_char` `_pskr` path).
/// Unmapped bytes are skipped.
pub fn mfsk_encode(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in text.as_bytes() {
        let cw = MFSK.code[b as usize];
        if !cw.is_empty() {
            out.extend(cw.bytes().map(|c| c - b'0'));
        }
    }
    out
}

/// Map one MFSK codeword value (the `shreg >> 1` at a framing boundary) to its
/// byte, or `None` if it is not a valid codeword. For streaming decoders that
/// run the `shreg` framing themselves (e.g. the PSK-R demodulator).
pub fn mfsk_symbol_to_byte(sym: u32) -> Option<u8> {
    (0..256)
        .find(|&i| {
            let cw = MFSK.code[i];
            !cw.is_empty() && mfsk_codeval(cw) == sym
        })
        .map(|i| i as u8)
}

/// Decode a self-framed MFSK Varicode bitstream back to text, using fldigi's
/// exact shift-register framing (`shreg=(shreg<<1)|bit; on (shreg&7)==1 decode
/// shreg>>1, reset to 1`). A final codeword with no following boundary bit is
/// not emitted — matching fldigi. ref: psk.cxx:1117-1123.
pub fn mfsk_decode(bits: &[u8]) -> String {
    let mut out = String::new();
    let mut shreg: u32 = 0;
    for &bit in bits {
        shreg = (shreg << 1) | (bit as u32 & 1);
        if shreg & 7 == 1 {
            let sym = shreg >> 1;
            if let Some(b) = mfsk_symbol_to_byte(sym) {
                if b != 0 {
                    out.push(b as char);
                }
            }
            shreg = 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-exact: the MFSK Varicode encode bitstream and the self-framed decode
    /// reproduce fldigi's `varienc`/`rx_bit` output byte-for-byte. Provenance:
    /// `tests/vectors/psk_mfsk.json` (fldigi 4.1.23 @ 61b97f413, driver
    /// `scratch/refvectors/build_mfsk_varicode.sh`).
    #[test]
    fn mfsk_matches_fldigi_vector() {
        let raw = include_str!("../../tests/vectors/psk_mfsk.json");
        let line = raw.lines().find(|l| l.contains("\"mfsk_bits\"")).unwrap();
        let field = |k: &str| {
            let i = line.find(k).unwrap() + k.len();
            line[i..line[i..].find('"').unwrap() + i].to_string()
        };
        let msg = field("\"msg\":\"");
        let want_bits: Vec<u8> = field("\"mfsk_bits\":\"").bytes().map(|c| c - b'0').collect();
        let want_decoded = field("\"decoded\":\"");
        assert_eq!(mfsk_encode(&msg), want_bits, "MFSK encode differs from fldigi");
        assert_eq!(mfsk_decode(&want_bits), want_decoded, "MFSK decode differs from fldigi");
    }

    #[test]
    fn mfsk_round_trips_except_final_char() {
        // The self-framing needs a following codeword's leading bit to close the
        // previous one, so the last character of a stream is never emitted; every
        // earlier character round-trips exactly.
        for s in ["CQ DE K1ABC", "the quick brown fox 0123"] {
            assert_eq!(mfsk_decode(&mfsk_encode(s)), s[..s.len() - 1]);
        }
    }

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
