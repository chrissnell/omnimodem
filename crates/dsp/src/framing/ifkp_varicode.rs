//! IFKP Varicode — the self-framing 33-tone IFK symbol code.
//!
//! Port of fldigi `src/ifkp/ifkp_varicode.cxx` (upstream 4.2.x, checked out at
//! ../fldigi). Each character maps to one symbol (`sym1`) or two (`sym1`, then
//! `sym2` when `sym2 > 28`). `sym1` is 0..28; the optional `sym2` is 29, 30 or
//! 31. The stream is self-framing: a single-symbol character is completed by the
//! *next* character's leading symbol (both < 29), while a two-symbol character
//! completes as soon as its `sym2` (in 29..31) arrives. ref: ifkp.cxx:717-728
//! (`send_char`), ifkp.cxx:423-480 (`process_symbol` streaming decode).
//!
//! Both the `ifkp_varicode[256][2]` encode table and the `ifkp_varidecode`
//! decode table are transcribed **verbatim** from the reference and asserted
//! byte-for-byte against the golden vector (`tests/vectors/ifkp_varicode.json`,
//! driver `scratch/refvectors/build_ifkp_varicode.sh`). They are *not* shared
//! with FSQ — the two tables differ in a handful of rows (`<LF>`, space, the
//! `248` slot).

/// Number of IFK tones. ref: ifkp.cxx:708 (`% 33`).
pub const NUMTONES: usize = 33;

/// IFK tone-advance offset: `tone = (prev + sym + OFFSET) % 33`. ref: ifkp.h:46.
pub const OFFSET: u32 = 1;

/// The IFKP Varicode encode table: row `ch` gives `[sym1, sym2]`. `sym2` is
/// emitted only when `> 28`. Transcribed verbatim from ifkp_varicode.cxx:1-34.
#[rustfmt::skip]
pub static IFKP_VARICODE: [[u8; 2]; 256] = [
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [27,31], [0,0], [28,30], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
    [28,0], [11,30], [12,30], [13,30], [14,30], [15,30], [16,30], [17,30],
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
    [12,31], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0], [0,0],
];

/// The IFKP Varicode decode table, indexed `prev * 32 + curr`: `prev` in 0..28,
/// `curr` in 0..31. A single-symbol character decodes as `[prev]` (curr < 29); a
/// two-symbol character decodes as `[prev*32 + curr]` (28 < curr < 32). `-1` is
/// an unreachable slot. Transcribed verbatim from ifkp_varicode.cxx:44-75.
#[rustfmt::skip]
pub static IFKP_VARIDECODE: [i16; 29 * 32] = [
      0,  97,  98,  99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122,  46,  32,  64, 126,  61,
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
     -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  -1,  63,  10, 127,
];

/// The symbols `send_char` emits for character `ch`: `sym1`, then `sym2` when
/// `sym2 > 28`. ref: ifkp.cxx:717-728.
pub fn encode_char(ch: u8) -> Vec<u8> {
    let row = IFKP_VARICODE[ch as usize];
    let mut out = Vec::with_capacity(2);
    out.push(row[0]);
    if row[1] > 28 {
        out.push(row[1]);
    }
    out
}

/// The symbol stream for `text` (concatenated per-character encodings).
pub fn encode_text(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for b in text.bytes() {
        out.extend(encode_char(b));
    }
    out
}

/// Streaming IFKP Varicode framer: fed one symbol at a time (0..32), it emits a
/// decoded character whenever `process_symbol`'s two-symbol state machine
/// completes one. Single-symbol characters emit one symbol late (on the next
/// symbol); two-symbol characters emit on their `sym2`. Seeded with `prev = 0`,
/// exactly as fldigi's `prev_nibble` starts. ref: ifkp.cxx:423-480.
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
    /// Mirrors `process_symbol`: single-symbol when `prev < 29 && curr < 29`,
    /// two-symbol when `prev < 29 && 28 < curr < 32`; `curr_ch > 0` gates output
    /// (so NUL/`-1` are dropped). ref: ifkp.cxx:437-476.
    pub fn push(&mut self, curr: u8) -> Option<u8> {
        let mut ch: i16 = -1;
        if self.prev < 29 && curr < 29 {
            ch = IFKP_VARIDECODE[self.prev as usize];
        } else if self.prev < 29 && curr > 28 && curr < 32 {
            ch = IFKP_VARIDECODE[self.prev as usize * 32 + curr as usize];
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

    // ---- bit-exact KATs mirrored from the fldigi golden vector -------------
    // These run in plain `cargo test` (CI does not enable `testutil`); the
    // reference-vector KAT in tests/kat.rs asserts the same tables against the
    // JSON. Provenance: tests/vectors/ifkp_varicode.json.

    #[test]
    fn known_encodings_match_reference() {
        // ref: ifkp_varicode.cxx rows. 'a'=97→{1,0} single; '@'=64→{0,29} two;
        // ' '=32→{28,0} single; '!'=33→{11,30} two.
        assert_eq!(encode_char(b'a'), vec![1]);
        assert_eq!(encode_char(b'@'), vec![0, 29]);
        assert_eq!(encode_char(b' '), vec![28]);
        assert_eq!(encode_char(b'!'), vec![11, 30]);
        assert_eq!(encode_char(b'A'), vec![1, 29]);
    }

    /// Feed the encoded symbol stream through the framer (with two flushing idle
    /// symbols) and confirm every printable character round-trips.
    fn roundtrip(text: &str) -> String {
        let syms = encode_text(text);
        let mut f = Framer::new();
        let mut out = String::new();
        for s in syms.into_iter().chain([0, 0]) {
            if let Some(c) = f.push(s) {
                out.push(c as char);
            }
        }
        out
    }

    #[test]
    fn framer_round_trips_text() {
        for msg in [
            "hello world",
            "CQ CQ CQ de K1ABC",
            "The quick brown fox 0123456789!",
            "abcXYZ .,?/()[]",
        ] {
            assert_eq!(roundtrip(msg), msg, "round-trip {msg:?}");
        }
    }

    #[test]
    fn every_printable_char_round_trips() {
        // Every printable ASCII character round-trips through encode→frame.
        for c in 32u8..=126 {
            let got = roundtrip(&format!("{}", c as char));
            assert_eq!(got, format!("{}", c as char), "char {c}");
        }
    }
}
