//! DominoEX Varicode — the self-framing nibble code shared by the whole
//! DominoEX/THOR IFK+ family.
//!
//! Port of fldigi `src/dominoex/dominovar.cxx` (upstream 4.1.23 @ 61b97f413).
//! Each character maps to 1–3 four-bit symbols (nibbles). The first nibble of a
//! character always has its MSB (`0x8`) **clear**; continuation nibbles always
//! have it **set**. That makes the stream self-delimiting: a nibble with the MSB
//! clear both starts a new character and completes the previous one. ref:
//! dominovar.cxx:27-95 (`varicode[512][3]`), dominoex.cxx:664-681 (`sendchar`
//! emits `code[0]`, then `code[1..2]` while `0x8` is set), dominoex.cxx:372-395
//! (`decodeDomino` accumulates the nibbles, newest in bits 3:0, into the
//! `varidec` index).
//!
//! The wire-determining `varicode` encode table is transcribed **verbatim** from
//! the reference. The redundant `varidecode` lookup (a 4096-entry table fldigi
//! ships alongside) is **derived here by inverting** that verbatim encode table —
//! computing each row's `decodeDomino` index and mapping it back to the row's
//! character. This reproduces `dominoex_varidec` for every reachable index and is
//! asserted byte-for-byte against the fldigi golden vector (all 256 primary rows'
//! `idx`/`dec`, plus a full 512-row round-trip). See
//! `tests/vectors/dominoex_varicode.json`.

/// Number of IFK+ tones (also the varicode symbol alphabet is a 4-bit nibble).
/// ref: dominoex.h:46.
pub const NUMTONES: usize = 18;

/// Maximum nibbles per character. ref: dominovar.h:27 (`MAX_VARICODE_LEN`).
pub const MAX_VARICODE_LEN: usize = 3;

/// The DominoEX Varicode encode table: row `c` (primary) / `256 + c` (secondary)
/// gives the up-to-three nibbles `{code[0], code[1], code[2]}`. A `code[i]` with
/// `0x8` clear (for `i >= 1`) terminates the character. Transcribed verbatim from
/// dominovar.cxx:27-95.
#[rustfmt::skip]
pub static VARICODE: [[u8; 3]; 512] = [
    [1,15,9], [1,15,10], [1,15,11], [1,15,12], [1,15,13], [1,15,14], [1,15,15], [2,8,8],
    [2,12,0], [2,8,9], [2,8,10], [2,8,11], [2,8,12], [2,13,0], [2,8,13], [2,8,14],
    [2,8,15], [2,9,8], [2,9,9], [2,9,10], [2,9,11], [2,9,12], [2,9,13], [2,9,14],
    [2,9,15], [2,10,8], [2,10,9], [2,10,10], [2,10,11], [2,10,12], [2,10,13], [2,10,14],
    [0,0,0], [7,11,0], [0,8,14], [0,10,11], [0,9,10], [0,9,9], [0,8,15], [7,10,0],
    [0,8,12], [0,8,11], [0,9,13], [0,8,8], [2,11,0], [7,14,0], [7,13,0], [0,8,9],
    [3,15,0], [4,10,0], [4,15,0], [5,9,0], [6,8,0], [5,12,0], [5,14,0], [6,12,0],
    [6,11,0], [6,14,0], [0,8,10], [0,8,13], [0,10,8], [7,15,0], [0,9,15], [7,12,0],
    [0,9,8], [3,9,0], [4,14,0], [3,12,0], [3,14,0], [3,8,0], [4,12,0], [5,8,0],
    [5,10,0], [3,10,0], [7,8,0], [6,10,0], [4,11,0], [4,8,0], [4,13,0], [3,11,0],
    [4,9,0], [6,15,0], [3,13,0], [2,15,0], [2,14,0], [5,11,0], [6,13,0], [5,13,0],
    [5,15,0], [6,9,0], [7,9,0], [0,10,14], [0,10,9], [0,10,15], [0,10,10], [0,9,12],
    [0,9,11], [4,0,0], [1,11,0], [0,12,0], [0,11,0], [1,0,0], [0,15,0], [1,9,0],
    [0,10,0], [5,0,0], [2,10,0], [1,14,0], [0,9,0], [0,14,0], [6,0,0], [3,0,0],
    [1,8,0], [2,8,0], [7,0,0], [0,8,0], [2,0,0], [0,13,0], [1,13,0], [1,12,0],
    [1,15,0], [1,10,0], [2,9,0], [0,10,12], [0,9,14], [0,10,13], [0,11,8], [2,10,15],
    [2,11,8], [2,11,9], [2,11,10], [2,11,11], [2,11,12], [2,11,13], [2,11,14], [2,11,15],
    [2,12,8], [2,12,9], [2,12,10], [2,12,11], [2,12,12], [2,12,13], [2,12,14], [2,12,15],
    [2,13,8], [2,13,9], [2,13,10], [2,13,11], [2,13,12], [2,13,13], [2,13,14], [2,13,15],
    [2,14,8], [2,14,9], [2,14,10], [2,14,11], [2,14,12], [2,14,13], [2,14,14], [2,14,15],
    [0,11,9], [0,11,10], [0,11,11], [0,11,12], [0,11,13], [0,11,14], [0,11,15], [0,12,8],
    [0,12,9], [0,12,10], [0,12,11], [0,12,12], [0,12,13], [0,12,14], [0,12,15], [0,13,8],
    [0,13,9], [0,13,10], [0,13,11], [0,13,12], [0,13,13], [0,13,14], [0,13,15], [0,14,8],
    [0,14,9], [0,14,10], [0,14,11], [0,14,12], [0,14,13], [0,14,14], [0,14,15], [0,15,8],
    [0,15,9], [0,15,10], [0,15,11], [0,15,12], [0,15,13], [0,15,14], [0,15,15], [1,8,8],
    [1,8,9], [1,8,10], [1,8,11], [1,8,12], [1,8,13], [1,8,14], [1,8,15], [1,9,8],
    [1,9,9], [1,9,10], [1,9,11], [1,9,12], [1,9,13], [1,9,14], [1,9,15], [1,10,8],
    [1,10,9], [1,10,10], [1,10,11], [1,10,12], [1,10,13], [1,10,14], [1,10,15], [1,11,8],
    [1,11,9], [1,11,10], [1,11,11], [1,11,12], [1,11,13], [1,11,14], [1,11,15], [1,12,8],
    [1,12,9], [1,12,10], [1,12,11], [1,12,12], [1,12,13], [1,12,14], [1,12,15], [1,13,8],
    [1,13,9], [1,13,10], [1,13,11], [1,13,12], [1,13,13], [1,13,14], [1,13,15], [1,14,8],
    [1,14,9], [1,14,10], [1,14,11], [1,14,12], [1,14,13], [1,14,14], [1,14,15], [1,15,8],
    [6,15,9], [6,15,10], [6,15,11], [6,15,12], [6,15,13], [6,15,14], [6,15,15], [7,8,8],
    [4,10,12], [7,8,9], [7,8,10], [7,8,11], [7,8,12], [4,10,13], [7,8,13], [7,8,14],
    [7,8,15], [7,9,8], [7,9,9], [7,9,10], [7,9,11], [7,9,12], [7,9,13], [7,9,14],
    [7,9,15], [7,10,8], [7,10,9], [7,10,10], [7,10,11], [7,10,12], [7,10,13], [7,10,14],
    [3,8,8], [4,15,11], [5,8,14], [5,10,11], [5,9,10], [5,9,9], [5,8,15], [4,15,10],
    [5,8,12], [5,8,11], [5,9,13], [5,8,8], [4,10,11], [4,15,14], [4,15,13], [5,8,9],
    [4,11,15], [4,12,10], [4,12,15], [4,13,9], [4,14,8], [4,13,12], [4,13,14], [4,14,12],
    [4,14,11], [4,14,14], [5,8,10], [5,8,13], [5,10,8], [4,15,15], [5,9,15], [4,15,12],
    [5,9,8], [4,11,9], [4,12,14], [4,11,12], [4,11,14], [4,11,8], [4,12,12], [4,13,8],
    [4,13,10], [4,11,10], [4,15,8], [4,14,10], [4,12,11], [4,12,8], [4,12,13], [4,11,11],
    [4,12,9], [4,14,15], [4,11,13], [4,10,15], [4,10,14], [4,13,11], [4,14,13], [4,13,13],
    [4,13,15], [4,14,9], [4,15,9], [5,10,14], [5,10,9], [5,10,15], [5,10,10], [5,9,12],
    [5,9,11], [3,8,12], [4,9,11], [4,8,12], [4,8,11], [3,8,9], [4,8,15], [4,9,9],
    [4,8,10], [3,8,13], [4,10,10], [4,9,14], [4,8,9], [4,8,14], [3,8,14], [3,8,11],
    [4,9,8], [4,10,8], [3,8,15], [4,8,8], [3,8,10], [4,8,13], [4,9,13], [4,9,12],
    [4,9,15], [4,9,10], [4,10,9], [5,10,12], [5,9,14], [5,10,12], [5,11,8], [7,10,15],
    [7,11,8], [7,11,9], [7,11,10], [7,11,11], [7,11,12], [7,11,13], [7,11,14], [7,11,15],
    [7,12,8], [7,12,9], [7,12,10], [7,12,11], [7,12,12], [7,12,13], [7,12,14], [7,12,15],
    [7,13,8], [7,13,9], [7,13,10], [7,13,11], [7,13,12], [7,13,13], [7,13,14], [7,13,15],
    [7,14,8], [7,14,9], [7,14,10], [7,14,11], [7,14,12], [7,14,13], [7,14,14], [7,14,15],
    [5,11,9], [5,11,10], [5,11,11], [5,11,12], [5,11,13], [5,11,14], [5,11,15], [5,12,8],
    [5,12,9], [5,12,10], [5,12,11], [5,12,12], [5,12,13], [5,12,14], [5,12,15], [5,13,8],
    [5,13,9], [5,13,10], [5,13,11], [5,13,12], [5,13,13], [5,13,14], [5,13,15], [5,14,8],
    [5,14,9], [5,14,10], [5,14,11], [5,14,12], [5,14,13], [5,14,14], [5,14,15], [5,15,8],
    [5,15,9], [5,15,10], [5,15,11], [5,15,12], [5,15,13], [5,15,14], [5,15,15], [6,8,8],
    [6,8,9], [6,8,10], [6,8,11], [6,8,12], [6,8,13], [6,8,14], [6,8,15], [6,9,8],
    [6,9,9], [6,9,10], [6,9,11], [6,9,12], [6,9,13], [6,9,14], [6,9,15], [6,10,8],
    [6,10,9], [6,10,10], [6,10,11], [6,10,12], [6,10,13], [6,10,14], [6,10,15], [6,11,8],
    [6,11,9], [6,11,10], [6,11,11], [6,11,12], [6,11,13], [6,11,14], [6,11,15], [6,12,8],
    [6,12,9], [6,12,10], [6,12,11], [6,12,12], [6,12,13], [6,12,14], [6,12,15], [6,13,8],
    [6,13,9], [6,13,10], [6,13,11], [6,13,12], [6,13,13], [6,13,14], [6,13,15], [6,14,8],
    [6,14,9], [6,14,10], [6,14,11], [6,14,12], [6,14,13], [6,14,14], [6,14,15], [6,15,8],
];

/// The nibbles `sendchar` actually emits for character `c`: `code[0]`, then each
/// of `code[1]`, `code[2]` while its MSB (`0x8`) is set. `secondary` selects the
/// secondary alphabet (row `256 + c`). ref: dominoex.cxx:664-681.
pub fn encode_char(c: u8, secondary: bool) -> Vec<u8> {
    let row = &VARICODE[c as usize + if secondary { 256 } else { 0 }];
    let mut out = Vec::with_capacity(MAX_VARICODE_LEN);
    out.push(row[0]);
    for &n in &row[1..] {
        if n & 0x8 != 0 {
            out.push(n);
        } else {
            break;
        }
    }
    out
}

/// The `decodeDomino` accumulation index for a character whose nibbles (in send
/// order) are `nib`: the newest (last) nibble occupies bits 3:0. ref:
/// dominoex.cxx:372-395 (`symbolbuf[0]` is newest, `sym |= symbolbuf[i] << 4*i`).
pub fn decode_index(nib: &[u8]) -> u16 {
    let mut sym = 0u16;
    for (i, &n) in nib.iter().rev().enumerate() {
        sym |= (n as u16) << (4 * i);
    }
    sym & 0xFFF
}

/// The DominoEX Varicode decoder: index (see [`decode_index`]) → character, or
/// `None` for an unreachable index. Derived by inverting [`VARICODE`]; primary
/// rows map to `0..=255`, secondary rows to `256..=511` (matching fldigi's
/// `varidecode` values). Built once; look-ups are O(1). ref: dominovar.cxx:105-362.
pub struct Varidecoder {
    table: Vec<i16>,
}

impl Default for Varidecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Varidecoder {
    pub fn new() -> Self {
        let mut table = vec![-1i16; 4096];
        for row in 0..512usize {
            let nib = encode_char((row & 0xFF) as u8, row >= 256);
            let idx = decode_index(&nib) as usize;
            table[idx] = row as i16;
        }
        Varidecoder { table }
    }

    /// Decode an accumulated index to a character value (`0..=511`), or `None` if
    /// the index is unreachable. Values `>= 256` are secondary-alphabet codes.
    pub fn decode(&self, idx: u16) -> Option<u16> {
        match self.table[(idx & 0xFFF) as usize] {
            -1 => None,
            v => Some(v as u16),
        }
    }
}

/// Streaming DominoEX Varicode framer: fed one nibble at a time, it emits a
/// character each time a new character *starts* (a nibble with the MSB clear
/// completes the character accumulated so far). ref: dominoex.cxx:368-395.
pub struct Framer {
    dec: Varidecoder,
    // The newest ≤ `MAX_VARICODE_LEN` nibbles of the in-progress character, in
    // send order (fldigi's `symbolbuf` shift register). `count` is the total
    // nibbles seen since the last start (clamped), so an over-long character is
    // *dropped* rather than truncated — see `push`.
    buf: Vec<u8>,
    count: usize,
    started: bool,
}

impl Default for Framer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framer {
    pub fn new() -> Self {
        Framer {
            dec: Varidecoder::new(),
            buf: Vec::with_capacity(MAX_VARICODE_LEN),
            count: 0,
            started: false,
        }
    }

    /// Push one varicode nibble. Returns `Some(char_value)` when a preceding
    /// character completes (i.e. `nib`'s MSB is clear and a character was already
    /// in progress). Mirrors `decodeDomino` (dominoex.cxx:372-395): a start nibble
    /// completes the accumulated character, but only if it was `<= MAX_VARICODE_LEN`
    /// nibbles long — an over-long run (only reachable on noisy/malformed input) is
    /// dropped, exactly as fldigi's `symcounter <= MAX_VARICODE_LEN` guard does.
    pub fn push(&mut self, nib: u8) -> Option<u16> {
        let mut done = None;
        if nib & 0x8 == 0 {
            if self.started && (1..=MAX_VARICODE_LEN).contains(&self.count) {
                let idx = decode_index(&self.buf);
                done = self.dec.decode(idx);
            }
            self.buf.clear();
            self.count = 0;
            self.started = true;
        }
        // Keep the newest `MAX_VARICODE_LEN` nibbles (shift register); the drop of
        // an over-long character is governed by `count`, not by this cap.
        self.buf.push(nib);
        if self.buf.len() > MAX_VARICODE_LEN {
            self.buf.remove(0);
        }
        self.count = (self.count + 1).min(MAX_VARICODE_LEN + 1);
        done
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.count = 0;
        self.started = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_primary_char_round_trips() {
        // encode → accumulate index → decode == the original character, for the
        // whole primary alphabet. Proves the verbatim encode table and the derived
        // inverse decoder agree with fldigi's dominoex_varidec (asserted against
        // the golden `idx`/`dec` columns in tests/kat.rs). ref: dominovar.cxx.
        let dec = Varidecoder::new();
        for c in 0u16..256 {
            let nib = encode_char(c as u8, false);
            assert!(nib.len() <= MAX_VARICODE_LEN);
            // first nibble always starts a character (MSB clear).
            assert_eq!(nib[0] & 0x8, 0, "char {c}: first nibble must have MSB clear");
            // continuation nibbles always have the MSB set.
            for &n in &nib[1..] {
                assert_ne!(n & 0x8, 0, "char {c}: continuation nibble must have MSB set");
            }
            let idx = decode_index(&nib);
            assert_eq!(dec.decode(idx), Some(c), "char {c} round-trip");
        }
    }

    #[test]
    fn secondary_alphabet_round_trips() {
        // The secondary alphabet round-trips for every character **except** the
        // one collision fldigi ships in its own table: rows 123 and 125 both use
        // the triple {5,10,12} (dominovar.cxx:78 lists {5,10,12} twice), so the
        // inverse — like fldigi's own varidecode — can only resolve one of them.
        // The secondary/MultiPsk path is deferred to Phase 9b; this documents the
        // reference quirk rather than "correcting" it.
        let dec = Varidecoder::new();
        for c in 0u16..256 {
            if c == 123 {
                continue; // collides with 125 on the duplicated {5,10,12}; see above.
            }
            let nib = encode_char(c as u8, true);
            let idx = decode_index(&nib);
            assert_eq!(dec.decode(idx), Some(256 + c), "secondary char {c} round-trip");
        }
    }

    #[test]
    fn known_codes_match_reference() {
        // ref: dominovar.cxx — spot checks. Space (0x20) is the single-symbol
        // tone 0; '!' (0x21) is two symbols {7,11}.
        assert_eq!(encode_char(b' ', false), vec![0]);
        assert_eq!(encode_char(b'!', false), vec![7, 11]);
        assert_eq!(encode_char(0, false), vec![1, 15, 9]); // NUL is 3 symbols
    }

    #[test]
    fn framer_delimits_a_multi_char_stream() {
        // Feed the concatenated nibbles of "AB" and confirm the framer emits A on
        // B's start nibble; the final char needs an explicit flush push.
        let mut f = Framer::new();
        let mut out = Vec::new();
        for &n in encode_char(b'A', false).iter().chain(encode_char(b'B', false).iter()) {
            if let Some(c) = f.push(n) {
                out.push(c);
            }
        }
        assert_eq!(out, vec![b'A' as u16]);
        // flushing (a fresh start nibble) completes B.
        assert_eq!(f.push(0), Some(b'B' as u16));
    }

    #[test]
    fn framer_drops_over_long_characters() {
        // fldigi's decodeDomino only decodes when symcounter <= MAX_VARICODE_LEN;
        // a run of >3 nibbles before a start nibble (only reachable on noisy input)
        // is dropped, not truncated. ref: dominoex.cxx:372-395.
        let mut f = Framer::new();
        // start nibble, then four continuation nibbles = 5 nibbles total (> 3).
        assert_eq!(f.push(1), None); // start
        for n in [0x8, 0x9, 0xA, 0xB] {
            assert_eq!(f.push(n), None);
        }
        // The start nibble that ends the over-long run drops it (no mis-decode)...
        assert_eq!(f.push(0), None, "over-long run must be dropped, not truncated");
        // ...and the framer resumes correctly: the single-nibble character just
        // started (tone 0 = space) frames cleanly on the next start.
        assert_eq!(f.push(0), Some(b' ' as u16));
    }
}
