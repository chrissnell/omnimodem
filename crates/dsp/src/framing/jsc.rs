//! JSC — JS8Call's compressed-text codec.
//!
//! Port of `js8call/jsc.cpp` (upstream `js8call/js8call` @ a7ff1be). JSC is a
//! dictionary + variable-length prefix code: text is greedily split into the
//! **longest dictionary-prefix matches**, and each match's dictionary index is
//! emitted as a self-delimiting run of 4-bit groups (plus a 1-bit
//! word-separator flag on the terminal group). It is the source coder behind
//! JS8's `FrameData` / `FrameDataCompressed` frames.
//!
//! The dictionary is a 262,144-entry table. It is embedded as a compact binary
//! blob (`jsc_dict.bin`) produced by the extractor `scratch/refvectors/js8/`
//! from the **unmodified** reference tables (`jsc_map.cpp`, `jsc_list.cpp`):
//!   - `map` — words in index order (used by `decompress` and codeword indices),
//!   - `list` — the search-order permutation (used by `compress`'s `lookup`;
//!     its ordering is load-bearing — longest matches come first within a
//!     first-byte bucket),
//!   - `prefix` — the 103-entry first-byte jump table.
//! Blob format is documented at the extractor (`dumper.cpp`). The bit-domain
//! output is deterministic and asserted **bit-exact** against reference golden
//! vectors (`tests/vectors/js8_jsc.json`) — see the tests below.
//!
//! Codec parameters (`jsc.cpp:52-100`): `b=4` (bits per group), `s=7`, and
//! `c = 2^b - s = 9`. Bits are packed MSB-first per group
//! (`Varicode::intToBits`, `varicode.cpp:649`).

use std::sync::OnceLock;

/// A bit-domain codeword: MSB-first bits, one `bool` per bit. Mirrors the
/// reference `QVector<bool>`.
pub type Codeword = Vec<bool>;

const BLOB: &[u8] = include_bytes!("jsc_dict.bin");

/// JSC codec parameters (`jsc.cpp:55-57`).
const B: u32 = 4;
const S: u32 = 7;
const C: u32 = (1 << B) - S; // 9

/// The parsed dictionary. Built once from `BLOB` and cached.
struct Dict {
    /// Backing store for the word bytes (borrowed from the embedded blob).
    blob: &'static [u8],
    /// Per map-index: byte offset into `blob` of the word's latin1 bytes.
    word_off: Vec<u32>,
    /// Per map-index: byte length (`strlen`) of the word.
    word_len: Vec<u8>,
    /// Per map-index: the reference `map[i].size` field. Equal to `word_len`
    /// for every entry except the filler `ROSIDS` (@262143, size 1) — preserved
    /// verbatim because `compress` consumes exactly `map_size` input bytes.
    map_size: Vec<u8>,
    /// The `list` search-order permutation: `list_perm[i]` is the map index of
    /// the `i`-th entry in reference `list` order.
    list_perm: Vec<u32>,
    /// First-byte jump table: `(first_byte, bucket_count, list_start)`.
    prefix: Vec<(u8, u32, u32)>,
}

impl Dict {
    fn word(&self, idx: u32) -> &[u8] {
        let off = self.word_off[idx as usize] as usize;
        let len = self.word_len[idx as usize] as usize;
        &self.blob[off..off + len]
    }
}

fn read_u32(b: &[u8], p: &mut usize) -> u32 {
    let v = u32::from_le_bytes([b[*p], b[*p + 1], b[*p + 2], b[*p + 3]]);
    *p += 4;
    v
}

fn dict() -> &'static Dict {
    static DICT: OnceLock<Dict> = OnceLock::new();
    DICT.get_or_init(|| {
        let b = BLOB;
        let mut p = 0usize;
        let count = read_u32(b, &mut p) as usize;
        let mut word_off = Vec::with_capacity(count);
        let mut word_len = Vec::with_capacity(count);
        let mut map_size = Vec::with_capacity(count);
        for _ in 0..count {
            let ms = b[p];
            let slen = b[p + 1];
            p += 2;
            word_off.push(p as u32);
            word_len.push(slen);
            map_size.push(ms);
            p += slen as usize;
        }
        let count2 = read_u32(b, &mut p) as usize;
        assert_eq!(count, count2, "jsc_dict.bin: word/list count mismatch");
        let mut list_perm = Vec::with_capacity(count2);
        for _ in 0..count2 {
            list_perm.push(read_u32(b, &mut p));
        }
        let psize = read_u32(b, &mut p) as usize;
        let mut prefix = Vec::with_capacity(psize);
        for _ in 0..psize {
            let fb = b[p];
            p += 1;
            let cnt = read_u32(b, &mut p);
            let start = read_u32(b, &mut p);
            prefix.push((fb, cnt, start));
        }
        // Sanity: the blob is exactly consumed and the table anchors are right.
        assert_eq!(p, b.len(), "jsc_dict.bin: trailing bytes");
        let d = Dict { blob: b, word_off, word_len, map_size, list_perm, prefix };
        debug_assert_eq!(d.word(0), b"E");
        debug_assert_eq!(d.word(1), b"T");
        debug_assert_eq!(d.word(262143), b"ROSIDS");
        debug_assert_eq!(d.map_size[262143], 1);
        d
    })
}

/// `Varicode::intToBits(value, expected)` — MSB-first, left-padded to `expected`
/// bits. ref: varicode.cpp:649-663.
fn int_to_bits(mut value: u64, expected: usize) -> Codeword {
    let mut bits = Codeword::new();
    while value != 0 {
        bits.insert(0, (value & 1) != 0);
        value >>= 1;
    }
    while bits.len() < expected {
        bits.insert(0, false);
    }
    bits
}

/// `Varicode::bitsToInt` — MSB-first. ref: varicode.cpp:666-672.
fn bits_to_int(bits: &[bool]) -> u64 {
    let mut v = 0u64;
    for &b in bits {
        v = (v << 1) + b as u64;
    }
    v
}

/// `JSC::codeword(index, separate, b, s, c)` — emit the self-delimiting bit run
/// for a dictionary `index`. ref: jsc.cpp:31-50.
fn codeword(index: u32, separate: bool, bytesize: u32, s: u32, c: u32) -> Codeword {
    let mut groups: Vec<Codeword> = Vec::new();
    let v = ((index % s) << 1) + separate as u32;
    groups.insert(0, int_to_bits(v as u64, (bytesize + 1) as usize));
    let mut x = index / s;
    while x > 0 {
        x -= 1;
        groups.insert(0, int_to_bits(((x % c) + s) as u64, bytesize as usize));
        x /= c;
    }
    let mut word = Codeword::new();
    for g in groups {
        word.extend(g);
    }
    word
}

/// Longest-dictionary-prefix lookup for the input starting at `b`. Returns the
/// map index whose word is the reference's chosen (first-in-`list`-order) prefix
/// of `b`, or `None`. ref: jsc.cpp:196-238.
fn lookup(d: &Dict, b: &[u8]) -> Option<u32> {
    if b.is_empty() {
        return None;
    }
    let first = b[0];
    let mut index = 0u32;
    let mut count = 0u32;
    let mut found = false;
    for &(pb, pcount, pstart) in &d.prefix {
        if first != pb {
            continue;
        }
        // Single-entry bucket: the reference returns immediately (single-char /
        // punctuation words), without a full strncmp. ref: jsc.cpp:208-212.
        if pcount == 1 {
            return Some(d.list_perm[pstart as usize]);
        }
        index = pstart;
        count = pcount;
        found = true;
        break;
    }
    if !found {
        return None;
    }
    for i in index..index + count {
        let mi = d.list_perm[i as usize];
        let w = d.word(mi);
        if b.len() >= w.len() && &b[..w.len()] == w {
            return Some(mi);
        }
    }
    None
}

/// A compressed word chunk: `(bits, char_count)`, where `char_count` counts the
/// input characters consumed (including a following space when one was folded
/// into this chunk). Mirrors `CodewordPair`. ref: jsc.cpp:52-95.
pub type CodewordPair = (Codeword, u32);

/// `JSC::compress` — compress `text` to a list of `(bits, nchars)` chunks.
///
/// Operates on latin1/ASCII bytes (JS8 traffic is ASCII); the input `&str` is
/// treated as its UTF-8 bytes, which coincides with latin1 for ASCII. ref:
/// jsc.cpp:52-95.
pub fn compress(text: &str) -> Vec<CodewordPair> {
    let d = dict();
    let mut out: Vec<CodewordPair> = Vec::new();

    // text.split(" ", KeepEmptyParts)
    let words: Vec<&[u8]> = split_keep_empty(text.as_bytes(), b' ');
    let len = words.len();
    for (i, wslice) in words.iter().enumerate() {
        let is_last_word = i == len - 1;
        let mut is_space_character = false;
        // An empty part is a space, unless it is the last word.
        let mut w: &[u8] = if wslice.is_empty() && !is_last_word {
            is_space_character = true;
            b" "
        } else {
            wslice
        };

        while !w.is_empty() {
            let index = match lookup(d, w) {
                Some(ix) => ix,
                None => break,
            };
            let msize = d.map_size[index as usize] as usize;
            w = &w[msize.min(w.len())..];

            let is_last = w.is_empty();
            let should_append_space = is_last && !is_space_character && !is_last_word;
            let bits = codeword(index, should_append_space, B, S, C);
            out.push((bits, msize as u32 + should_append_space as u32));
        }
    }
    out
}

/// `text.split(sep, KeepEmptyParts)` on bytes — trailing/leading empties kept.
fn split_keep_empty(text: &[u8], sep: u8) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    for (i, &c) in text.iter().enumerate() {
        if c == sep {
            parts.push(&text[start..i]);
            start = i + 1;
        }
    }
    parts.push(&text[start..]);
    parts
}

/// `JSC::decompress` — inverse of [`compress`]. Interprets dictionary words as
/// latin1 (byte → char). ref: jsc.cpp:97-171.
pub fn decompress(bitvec: &[bool]) -> String {
    let d = dict();
    let s = S;
    let c = C;

    // base[k] offsets. ref: jsc.cpp:104-112.
    let mut base = [0u64; 8];
    base[0] = 0;
    base[1] = s as u64;
    for k in 2..8 {
        base[k] = base[k - 1] + (s as u64) * (c as u64).pow((k - 1) as u32);
    }

    let mut bytes: Vec<u64> = Vec::new();
    let mut separators: Vec<u32> = Vec::new();

    let count = bitvec.len();
    let mut i = 0usize;
    while i < count {
        if i + 4 > count {
            break;
        }
        let byte = bits_to_int(&bitvec[i..i + 4]);
        bytes.push(byte);
        i += 4;
        if byte < s as u64 {
            if count - i > 0 && bitvec[i] {
                separators.push(bytes.len() as u32 - 1);
            }
            i += 1;
        }
    }

    let mut out = String::new();
    let size = d.list_perm.len() as u64; // == JSC::size (262144)
    let mut start = 0usize;
    let mut sep_head = 0usize;
    while start < bytes.len() {
        let mut k = 0usize;
        let mut j = 0u64;
        while start + k < bytes.len() && bytes[start + k] >= s as u64 {
            j = j * c as u64 + (bytes[start + k] - s as u64);
            k += 1;
        }
        if j >= size {
            break;
        }
        if start + k >= bytes.len() {
            break;
        }
        j = j * s as u64 + bytes[start + k] + base[k];
        if j >= size {
            break;
        }
        // latin1 → char
        for &byte in d.word(j as u32) {
            out.push(byte as char);
        }
        if sep_head < separators.len() && separators[sep_head] as usize == start + k {
            out.push(' ');
            sep_head += 1;
        }
        start += k + 1;
    }
    out
}

/// Concatenate the `(bits, _)` chunks of [`compress`] into one bit stream.
pub fn compress_bits(text: &str) -> Codeword {
    let mut all = Codeword::new();
    for (bits, _) in compress(text) {
        all.extend(bits);
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_from_str(s: &str) -> Codeword {
        s.bytes().map(|c| c == b'1').collect()
    }
    fn bits_to_str(b: &[bool]) -> String {
        b.iter().map(|&x| if x { '1' } else { '0' }).collect()
    }

    /// Minimal JSON field extractor matching the style used across `crates/dsp`
    /// (no serde dependency).
    fn vectors() -> Vec<(String, String, String)> {
        let raw = include_str!("../../tests/vectors/js8_jsc.json");
        let mut out = Vec::new();
        for line in raw.lines().filter(|l| l.contains("\"compressed\"")) {
            let field = |k: &str| -> String {
                let i = line.find(k).unwrap() + k.len();
                line[i..line[i..].find('"').unwrap() + i].to_string()
            };
            out.push((field("\"text\": \""), field("\"compressed\": \""), field("\"decompressed\": \"")));
        }
        out
    }

    /// Bit-exact: `compress` reproduces the reference bit stream byte-for-byte.
    /// Provenance: `tests/vectors/js8_jsc.json` (js8call @ a7ff1be, driver
    /// `scratch/refvectors/js8/build_jsc.sh`).
    #[test]
    fn jsc_compress_matches_reference() {
        for (text, compressed, _) in vectors() {
            let got = compress_bits(&text);
            assert_eq!(
                bits_to_str(&got),
                compressed,
                "JSC compress differs from reference for {text:?}"
            );
        }
    }

    /// Bit-exact: `decompress` of the reference bit stream reproduces the text.
    #[test]
    fn jsc_decompress_matches_reference() {
        for (_text, compressed, decompressed) in vectors() {
            let bits = bits_from_str(&compressed);
            assert_eq!(
                decompress(&bits),
                decompressed,
                "JSC decompress differs from reference"
            );
        }
    }

    /// Round-trip: `decompress(compress(x))` reproduces the reference's
    /// canonical form (which equals `x` for these in-dictionary ASCII vectors).
    #[test]
    fn jsc_roundtrip() {
        for (text, _c, decompressed) in vectors() {
            assert_eq!(decompress(&compress_bits(&text)), decompressed);
        }
    }

    /// Single-character sanity against hand-computed values (E=index 0, A=index 2).
    #[test]
    fn jsc_single_char() {
        assert_eq!(bits_to_str(&compress_bits("E")), "00000");
        assert_eq!(bits_to_str(&compress_bits("A")), "00100");
    }
}
