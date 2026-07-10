//! FSQ (FSQCALL) Varicode + CRC8 — the self-framing 33-tone IFK symbol code and
//! the callsign checksum used by the directed protocol.
//!
//! Port of fldigi `src/fsq/fsq_varicode.cxx` and `include/crc8.h` (upstream
//! 4.2.x, checked out at ../fldigi). The symbol structure is identical to IFKP —
//! one symbol (`sym1`) or two (`sym1`, then `sym2` when `sym2 > 28`) per
//! character, self-framing on the leading symbol — but the tables differ from
//! IFKP's in a few rows (space `= {0,0}`, `<LF>` `= {28,0}`, slot `248`). ref:
//! fsq.cxx:1367-1382 (`send_char`), fsq.cxx:1017-1110 (`process_symbol`).
//!
//! Both the `fsq_varicode[256][2]` encode table and the `wsq_varidecode` decode
//! table are transcribed **verbatim** and asserted byte-for-byte against the
//! golden vector (`tests/vectors/fsq_varicode.json`).

/// Number of IFK tones. ref: fsq.cxx:1355 (`% 33`).
pub const NUMTONES: usize = 33;

/// IFK tone-advance offset: `tone = (prev + sym + OFFSET) % 33`. ref: fsq.cxx:1355.
pub const OFFSET: u32 = 1;

/// The FSQ Varicode encode table: row `ch` gives `[sym1, sym2]`; `sym2` is
/// emitted only when `> 28`. Transcribed verbatim from fsq_varicode.cxx:1-34.
#[rustfmt::skip]
pub static FSQ_VARICODE: [[u8; 2]; 256] = [
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [27,31], [0,0], [28,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [11,30], [12,30], [13,30], [14,30], [15,30], [16,30], [17,30],
    [18,30], [19,30], [20,30], [21,30], [27,29], [22,30], [27,0], [23,30],
    [10,30], [1,30], [2,30], [3,30], [4,30], [5,30], [6,30], [7,30],
    [8,30], [9,30], [24,30], [25,30], [26,30], [0,31], [27,30], [28,29],
    [0,29], [1,29], [2,29], [3,29], [4,29], [5,29], [6,29], [7,29],
    [8,29], [9,29], [10,29], [11,29], [12,29], [13,29], [14,29], [15,29],
    [16,29], [17,29], [18,29], [19,29], [20,29], [21,29], [22,29], [23,29],
    [24,29], [25,29], [26,29], [1,31], [2,31], [3,31], [4,31], [5,31],
    [9,31], [1,0], [2,0], [3,0], [4,0], [5,0], [6,0], [7,0],
    [8,0], [9,0], [10,0], [11,0], [12,0], [13,0], [14,0], [15,0],
    [16,0], [17,0], [18,0], [19,0], [20,0], [21,0], [22,0], [23,0],
    [24,0], [25,0], [26,0], [6,31], [7,31], [8,31], [0,30], [28,31],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [14,31], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [12,31], [10,31], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [13,31],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [11,31],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
];

/// The FSQ Varicode decode table, indexed `prev * 32 + curr`. Single-symbol
/// decode is `[prev]` (curr < 29); two-symbol decode is `[prev*32 + curr]` (28 <
/// curr < 32). `-1` is unreachable. Transcribed verbatim from
/// fsq_varicode.cxx:44-75 (`wsq_varidecode`).
#[rustfmt::skip]
pub static WSQ_VARIDECODE: [i16; 29 * 32] = [
     32,  97,  98,  99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122,  46,  10,  64, 126,  61,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  65,  49,  91,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  66,  50,  92,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  67,  51,  93,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  68,  52,  94,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  69,  53,  95,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  70,  54, 123,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  71,  55, 124,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  72,  56, 125,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  73,  57,  96,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  74,  48, 177,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  75,  33, 247,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  76,  34, 176,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  77,  35, 215,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  78,  36, 163,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  79,  37,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  80,  38,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  81,  39,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  82,  40,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  83,  41,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  84,  42,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  85,  43,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  86,  45,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  87,  47,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  88,  58,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  89,  59,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  90,  60,  -1,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  44,  62,   8,
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  63,   0, 127,
];

/// The symbols `send_char` emits for character `ch`: `sym1`, then `sym2` when
/// `sym2 > 28`. `NUL` is the two-symbol idle `28, 30`. ref: fsq.cxx:1359-1382.
pub fn encode_char(ch: u8) -> Vec<u8> {
    if ch == 0 {
        return vec![28, 30]; // send_idle
    }
    let row = FSQ_VARICODE[ch as usize];
    let mut out = Vec::with_capacity(2);
    out.push(row[0]);
    if row[1] > 28 {
        out.push(row[1]);
    }
    out
}

/// The symbol stream for a raw on-air string (concatenated per-character
/// encodings). BOT/EOT framing is applied by the caller (`modes::fsq`).
pub fn encode_str(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for b in s.bytes() {
        out.extend(encode_char(b));
    }
    out
}

/// CRC-8/CCITT (poly `0x07`, init `0x00`, no reflection) — the callsign
/// checksum FSQ appends to the directed header. ref: crc8.h.
pub fn crc8(s: &[u8]) -> u8 {
    let mut val: u8 = 0;
    for &b in s {
        let mut x = val ^ b;
        for _ in 0..8 {
            x = (x << 1) ^ if x & 0x80 != 0 { 0x07 } else { 0 };
        }
        val = x;
    }
    val
}

/// The 2-char lowercase hex checksum string, as `CRC8::sval` returns it. ref:
/// crc8.h.
pub fn crc8_hex(s: &str) -> String {
    format!("{:02x}", crc8(s.as_bytes()))
}

/// Streaming FSQ Varicode framer: identical state machine to IFKP's, over the
/// `wsq_varidecode` table. Seeded `prev = 0`. ref: fsq.cxx:1017-1110.
pub struct Framer {
    prev: u8,
}

impl Default for Framer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framer {
    pub fn new() -> Self {
        Framer { prev: 0 }
    }

    /// Push one received symbol. Returns `Some(ch)` when a character completes.
    /// ref: fsq.cxx:1032-1106.
    pub fn push(&mut self, curr: u8) -> Option<u8> {
        let mut ch: i16 = -1;
        if self.prev < 29 && curr < 29 {
            ch = WSQ_VARIDECODE[self.prev as usize];
        } else if self.prev < 29 && curr > 28 && curr < 32 {
            ch = WSQ_VARIDECODE[self.prev as usize * 32 + curr as usize];
        }
        self.prev = curr;
        if ch > 0 {
            Some(ch as u8)
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.prev = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_matches_reference() {
        // ref: tests/vectors/fsq_varicode.json crc8 rows (fldigi CRC8::sval).
        assert_eq!(crc8_hex("w1hkj"), "ef");
        assert_eq!(crc8_hex("k1abc"), "f3");
        assert_eq!(crc8_hex("n0call"), "69");
        assert_eq!(crc8_hex("allcall"), "6c");
        assert_eq!(crc8_hex("cqcqcq"), "33");
        assert_eq!(crc8_hex("ve3xyz/p"), "b6");
    }

    #[test]
    fn known_encodings_match_reference() {
        // ref: fsq_varicode.cxx rows. ' '=32→{0,0} single (sym 0); '<LF>'=10→
        // {28,0} single; 'a'=97→{1,0}; '@'=64→{0,29} two.
        assert_eq!(encode_char(b' '), vec![0]);
        assert_eq!(encode_char(b'\n'), vec![28]);
        assert_eq!(encode_char(b'a'), vec![1]);
        assert_eq!(encode_char(b'@'), vec![0, 29]);
        assert_eq!(encode_char(0), vec![28, 30]); // idle
    }

    /// Faithful decode: feed the symbols plus two flushing idle-`28` symbols,
    /// exactly as the golden extractor does. FSQ's `wsq_varidecode[0] == ' '`
    /// means the `prev = 0` seed always emits a **leading space**, and the two
    /// flush symbols always emit a **trailing `\n`** (`wsq_varidecode[28]`). Both
    /// are real fldigi behaviour, stripped by the BOT/EOT parse layer.
    fn faithful(text: &str) -> String {
        let syms = encode_str(text);
        let mut f = Framer::new();
        let mut out = String::new();
        for s in syms.into_iter().chain([28, 28]) {
            if let Some(c) = f.push(s) {
                out.push(c as char);
            }
        }
        out
    }

    /// The clean payload: faithful decode minus the leading space + trailing LF.
    fn clean(text: &str) -> String {
        let s = faithful(text);
        let s = s.strip_prefix(' ').unwrap_or(&s);
        let s = s.strip_suffix('\n').unwrap_or(s);
        s.to_string()
    }

    #[test]
    fn framer_matches_fldigi_decode() {
        // ref: tests/vectors/fsq_varicode.json "text" frame decode.
        assert_eq!(faithful("the quick brown fox de w1hkj"), " the quick brown fox de w1hkj\n");
    }

    #[test]
    fn framer_round_trips_payload() {
        for msg in ["the quick brown fox de w1hkj", "k1abc? snr", "test 123"] {
            assert_eq!(clean(msg), msg, "round-trip {msg:?}");
        }
    }

    #[test]
    fn every_printable_char_round_trips() {
        for c in 32u8..=126 {
            let got = clean(&format!("{}", c as char));
            assert_eq!(got, format!("{}", c as char), "char {c}");
        }
    }
}
