//! Throb / ThrobX family: dual-tone MFSK, parametric by submode.
//!
//! Port of fldigi's Throb modem (`fldigi/src/throb/throb.cxx`,
//! `fldigi/src/include/throb.h`, upstream 4.1.23 @ 61b97f413). Throb sounds each
//! printable character as a **pair of tones** together, shaped by a per-symbol
//! pulse envelope, at an 8 kHz sample rate. There is no varicode, FEC, or
//! interleave: a character maps (via a linear search of a fixed character set) to
//! a symbol index, and the symbol index maps to a tone pair. Regular Throb (9
//! tones, 45 symbols) has a SHIFT symbol that prefixes `? @ = \n`; ThrobX (11
//! tones, 55 symbols) has no shift and instead swaps its idle/space symbol indices
//! 0↔1 each time an idle or space is sent (`flip_syms`). Every transmission opens
//! with a 4-symbol idle preamble. ref: throb.cxx:582-721 (TX framing), :297-417
//! (RX), :729-944 (tables/freqs), throb.h:34-46 (constants).
//!
//! Wire-determining data is bit-exact vs fldigi: the tone-pair tables, character
//! sets, and the char→symbol→tone-pair sequence `tx_process`/`send` emit are
//! asserted byte-for-byte against vectors extracted from the unmodified fldigi
//! tables (`tests/vectors/throb.json`, `scratch/refvectors/build_throb.sh`).
//! Modulated audio is gated on a loopback decode only, never bit-exact (Doctrine
//! §3): fldigi's audio path is entangled with its FLTK modem runtime. Like the
//! DominoEX port, the streaming demod assumes symbol alignment to the fed buffer
//! (the sync/AFC front end and the fldigi cross-decode are the `#[ignore]` gate).
//!
//! fldigi quirk, replicated: regular-Throb TX for `-` sends SHIFT + symbol 9,
//! which the receiver decodes as `=` (throb.cxx:666-669 vs :372-373). `? @ \n`
//! round-trip; `-` becomes `=`.

use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::goertzel_power;
use crate::types::{Frame, FrameMeta, FramePayload, Sample};
use std::f32::consts::TAU;

const SAMPLE_RATE: u32 = 8000;

/// The six Throb submodes. ref: throb.cxx:141-220.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrobVariant {
    Throb1,
    Throb2,
    Throb4,
    ThrobX1,
    ThrobX2,
    ThrobX4,
}

/// Which per-symbol pulse envelope the modulator uses. ref: throb.cxx:542-579.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pulse {
    /// Raised-cosine ramps over `len/5` at each end, flat in between (Throb1/2, ThrobX1/2).
    Semi,
    /// Full-symbol Hann window (Throb4, ThrobX4).
    Full,
}

impl ThrobVariant {
    /// `symlen` in samples @ 8 kHz. ref: throb.h:42-44 (SYMLEN_1/2/4).
    pub fn symlen(self) -> usize {
        use ThrobVariant::*;
        match self {
            Throb1 | ThrobX1 => 8192,
            Throb2 | ThrobX2 => 4096,
            Throb4 | ThrobX4 => 2048,
        }
    }

    /// True for the ThrobX family (11 tones / 55 symbols, no shift, idle/space flip).
    pub fn is_throbx(self) -> bool {
        matches!(self, ThrobVariant::ThrobX1 | ThrobVariant::ThrobX2 | ThrobVariant::ThrobX4)
    }

    fn pulse(self) -> Pulse {
        use ThrobVariant::*;
        match self {
            Throb4 | ThrobX4 => Pulse::Full,
            _ => Pulse::Semi,
        }
    }

    /// Tone frequency offsets (Hz) relative to the carrier. ref: throb.cxx:941-944.
    pub fn freqs(self) -> &'static [f32] {
        use ThrobVariant::*;
        match self {
            Throb1 | Throb2 => &THROB_FREQS_NAR,
            Throb4 => &THROB_FREQS_WID,
            ThrobX1 | ThrobX2 => &THROBX_FREQS_NAR,
            ThrobX4 => &THROBX_FREQS_WID,
        }
    }

    pub fn num_tones(self) -> usize {
        self.freqs().len()
    }

    pub fn baud(self) -> f32 {
        SAMPLE_RATE as f32 / self.symlen() as f32
    }

    /// Occupied bandwidth = span of the tone offsets. ref: throb.cxx:243.
    pub fn bandwidth(self) -> f32 {
        let f = self.freqs();
        f[f.len() - 1] - f[0]
    }

    pub fn from_label(s: &str) -> Option<ThrobVariant> {
        use ThrobVariant::*;
        Some(match s {
            "throb1" => Throb1,
            "throb2" => Throb2,
            "throb4" => Throb4,
            "throbx1" => ThrobX1,
            "throbx2" => ThrobX2,
            "throbx4" => ThrobX4,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        use ThrobVariant::*;
        match self {
            Throb1 => "throb1",
            Throb2 => "throb2",
            Throb4 => "throb4",
            ThrobX1 => "throbx1",
            ThrobX2 => "throbx2",
            ThrobX4 => "throbx4",
        }
    }

    pub fn all() -> &'static [ThrobVariant] {
        use ThrobVariant::*;
        &[Throb1, Throb2, Throb4, ThrobX1, ThrobX2, ThrobX4]
    }
}

// ---- verbatim reference tables (throb.cxx), 0-based tone indices -------------
// The tone-pair tables are transcribed with fldigi's 1-based tone numbers reduced
// by 1 to 0-based (send() subtracts 1). ref: throb.cxx:729-775 / :777-833.

/// ref: throb.cxx:729-775 (ThrobTonePairs[45][2], less 1).
pub const THROB_TONEPAIRS: [(u8, u8); 45] = [
    (4, 4), (3, 4), (0, 1), (0, 2), (0, 3), (3, 5), (0, 4), (0, 5), (0, 6), (2, 6),
    (0, 7), (1, 2), (1, 3), (1, 7), (1, 4), (4, 5), (1, 5), (1, 8), (2, 3), (2, 4),
    (0, 8), (2, 5), (7, 8), (2, 7), (2, 2), (1, 1), (0, 0), (2, 8), (3, 6), (3, 7),
    (3, 8), (4, 6), (4, 7), (4, 8), (5, 6), (5, 7), (5, 8), (6, 7), (6, 8), (7, 7),
    (6, 6), (5, 5), (3, 3), (8, 8), (1, 6),
];

/// ref: throb.cxx:777-833 (ThrobXTonePairs[55][2], less 1).
pub const THROBX_TONEPAIRS: [(u8, u8); 55] = [
    (5, 10), (0, 5), (1, 5), (1, 4), (1, 6), (1, 7), (4, 5), (1, 8), (1, 9), (3, 7),
    (3, 5), (1, 10), (2, 3), (2, 4), (2, 5), (5, 8), (5, 9), (2, 6), (2, 7), (2, 8),
    (5, 7), (5, 6), (2, 9), (2, 10), (3, 4), (3, 6), (3, 8), (3, 9), (0, 1), (0, 2),
    (0, 3), (0, 4), (0, 6), (0, 7), (0, 8), (0, 9), (1, 2), (1, 3), (3, 10), (4, 6),
    (4, 7), (4, 8), (4, 9), (4, 10), (6, 7), (6, 8), (6, 9), (6, 10), (7, 8), (7, 9),
    (7, 10), (8, 9), (8, 10), (9, 10), (0, 10),
];

/// ref: throb.cxx:835-881 (ThrobCharSet[45]); 0 = non-printing idle/shift.
pub const THROB_CHARSET: [u8; 45] = [
    0, b'A', b'B', b'C', b'D', 0, b'F', b'G', b'H', b'I', b'J', b'K', b'L', b'M', b'N',
    b'O', b'P', b'Q', b'R', b'S', b'T', b'U', b'V', b'W', b'X', b'Y', b'Z', b'1', b'2',
    b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b',', b'.', b'\'', b'/', b')', b'(',
    b'E', b' ',
];

/// ref: throb.cxx:883-939 (ThrobXCharSet[55]); index 0 idle, index 1 space.
pub const THROBX_CHARSET: [u8; 55] = [
    0, b' ', b'A', b'B', b'C', b'D', b'E', b'F', b'G', b'H', b'I', b'J', b'K', b'L', b'M',
    b'N', b'O', b'P', b'Q', b'R', b'S', b'T', b'U', b'V', b'W', b'X', b'Y', b'Z', b'1', b'2',
    b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b',', b'.', b'\'', b'/', b')', b'(', b'#',
    b'"', b'+', b'-', b';', b':', b'?', b'!', b'@', b'=', b'\n',
];

const THROB_FREQS_NAR: [f32; 9] = [-32.0, -24.0, -16.0, -8.0, 0.0, 8.0, 16.0, 24.0, 32.0];
const THROB_FREQS_WID: [f32; 9] = [-64.0, -48.0, -32.0, -16.0, 0.0, 16.0, 32.0, 48.0, 64.0];
const THROBX_FREQS_NAR: [f32; 11] = [
    -39.0625, -31.25, -23.4375, -15.625, -7.8125, 0.0, 7.8125, 15.625, 23.4375, 31.25, 39.0625,
];
const THROBX_FREQS_WID: [f32; 11] =
    [-78.125, -62.5, -46.875, -31.25, -15.625, 0.0, 15.625, 31.25, 46.875, 62.5, 78.125];

// Number of idle preamble symbols. ref: throb.cxx:49.
const PREAMBLE: usize = 4;

/// The emitted symbol-index sequence for `text`, replicating `tx_process`
/// (throb.cxx:619-721): a 4-idle preamble, then per character the shift/space/flip
/// framing. Public for the bit-exact KAT.
pub fn text_symbols(v: ThrobVariant, text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    if v.is_throbx() {
        // reset_syms: idle=0, space=1; flip swaps them. ref: throb.cxx:119-124, :96-114.
        let (mut idlesym, mut spacesym) = (0usize, 1usize);
        let flip = |i: &mut usize, s: &mut usize| {
            if *i == 0 {
                *i = 1;
                *s = 0;
            } else {
                *i = 0;
                *s = 1;
            }
        };
        for _ in 0..PREAMBLE {
            out.push(idlesym);
            flip(&mut idlesym, &mut spacesym);
        }
        for &b in text.as_bytes() {
            let mut c = b;
            if c.is_ascii_lowercase() {
                c = c.to_ascii_uppercase();
            }
            // Space (found) and unknown chars both map to the current spacesym and
            // flip idle/space; any other char is a direct index. ref: throb.cxx:698-717.
            let found = THROBX_CHARSET.iter().rposition(|&x| x == c);
            let sym = match found {
                Some(i) if c != b' ' => i,
                _ => {
                    let s = spacesym;
                    flip(&mut idlesym, &mut spacesym);
                    s
                }
            };
            out.push(sym);
        }
    } else {
        // reset_syms: idle=0, space=44. ref: throb.cxx:125-129.
        out.resize(PREAMBLE, 0usize); // 4 idle-preamble symbols (idlesym == 0)
        for &b in text.as_bytes() {
            match b {
                b'?' => {
                    out.push(5);
                    out.push(20);
                    continue;
                }
                b'@' => {
                    out.push(5);
                    out.push(13);
                    continue;
                }
                b'-' => {
                    out.push(5);
                    out.push(9);
                    continue;
                }
                b'\r' => continue,
                b'\n' => {
                    out.push(5);
                    out.push(0);
                    continue;
                }
                _ => {}
            }
            let mut c = b;
            if c.is_ascii_lowercase() {
                c = c.to_ascii_uppercase();
            }
            // linear search keeps the last match, then unknown -> space (index 44).
            let sym = THROB_CHARSET.iter().rposition(|&x| x == c).unwrap_or(44);
            out.push(sym);
        }
    }
    out
}

/// The tone pair (0-based) for a symbol index in this family. ref: throb.cxx:591-602.
pub fn symbol_tones(v: ThrobVariant, sym: usize) -> (u8, u8) {
    if v.is_throbx() {
        THROBX_TONEPAIRS[sym]
    } else {
        THROB_TONEPAIRS[sym]
    }
}

/// The emitted `(tone1,tone2)` sequence for `text` (bit-exact vs the golden vector).
pub fn text_tones(v: ThrobVariant, text: &str) -> Vec<(u8, u8)> {
    text_symbols(v, text).into_iter().map(|s| symbol_tones(v, s)).collect()
}

/// The per-symbol pulse envelope, `symlen` samples. ref: throb.cxx:542-579.
fn make_pulse(kind: Pulse, len: usize) -> Vec<f32> {
    let mut p = vec![0.0f32; len];
    match kind {
        Pulse::Semi => {
            let fifth = len / 5; // integer, as fldigi
            let four_fifth = len * 4 / 5;
            let denom = len as f32 / 5.0;
            for (i, pi) in p.iter_mut().enumerate() {
                *pi = if i < fifth {
                    let x = std::f32::consts::PI * i as f32 / denom;
                    0.5 * (1.0 - x.cos())
                } else if i < four_fifth {
                    1.0
                } else {
                    let j = (i - four_fifth) as f32;
                    let x = std::f32::consts::PI * j / denom;
                    0.5 * (1.0 + x.cos())
                };
            }
        }
        Pulse::Full => {
            for (i, pi) in p.iter_mut().enumerate() {
                *pi = 0.5 * (1.0 - (TAU * i as f32 / len as f32).cos());
            }
        }
    }
    p
}

fn caps(v: ThrobVariant) -> ModeCaps {
    ModeCaps {
        native_rate: SAMPLE_RATE,
        bandwidth_hz: v.bandwidth(),
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

pub struct ThrobMod {
    v: ThrobVariant,
    center_hz: f32,
}

impl ThrobMod {
    pub fn new(v: ThrobVariant, center_hz: f32) -> Self {
        ThrobMod { v, center_hz }
    }
}

impl Modulator for ThrobMod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("throb needs text")),
        };
        let symlen = self.v.symlen();
        let pulse = make_pulse(self.v.pulse(), symlen);
        let freqs = self.v.freqs();
        let mut out = Vec::with_capacity(text_symbols(self.v, text).len() * symlen);
        for sym in text_symbols(self.v, text) {
            let (t1, t2) = symbol_tones(self.v, sym);
            // ref: throb.cxx:609-616. out[i] = pulse[i]*(sin w1·i + sin w2·i)/2.
            let w1 = TAU * (self.center_hz + freqs[t1 as usize]) / SAMPLE_RATE as f32;
            let w2 = TAU * (self.center_hz + freqs[t2 as usize]) / SAMPLE_RATE as f32;
            for (i, &p) in pulse.iter().enumerate() {
                let s = p * ((w1 * i as f32).sin() + (w2 * i as f32).sin()) / 2.0;
                out.push(s);
            }
        }
        Ok(out)
    }
}

pub struct ThrobDemod {
    v: ThrobVariant,
    center_hz: f32,
    buf: Vec<Sample>,
    // ThrobX-only RX state: the idle/space flip indices (tracked identically to
    // the transmitter so idle/space symbols map correctly) and the last decoded
    // char (so a run of idles/spaces collapses to a single space). Unused on the
    // regular-Throb path, which decodes via the tonepair→charset lookup + shift
    // latch. ref: throb.cxx:54-63, :396-414.
    idlesym: usize,
    spacesym: usize,
    lastchar: u8,
    // Regular-Throb shift latch. ref: throb.cxx:365-385.
    shift: bool,
}

impl ThrobDemod {
    pub fn new(v: ThrobVariant, center_hz: f32) -> Self {
        let (idlesym, spacesym) = if v.is_throbx() { (0, 1) } else { (0, 44) };
        ThrobDemod {
            v,
            center_hz,
            buf: Vec::new(),
            idlesym,
            spacesym,
            lastchar: 0,
            shift: false,
        }
    }

    fn flip(&mut self) {
        if self.idlesym == 0 {
            self.idlesym = 1;
            self.spacesym = 0;
        } else {
            self.idlesym = 0;
            self.spacesym = 1;
        }
    }

    /// Find the two dominant tones in a symbol block. ref: throb.cxx:297-350.
    fn find_tones(&self, block: &[Sample]) -> (usize, usize) {
        let freqs = self.v.freqs();
        // Goertzel amplitude (= sqrt(power), mirroring fldigi's abs()) at each
        // passband tone `center + freqs[k]`.
        let powers: Vec<f32> = freqs
            .iter()
            .map(|&f| goertzel_power(block, self.center_hz + f, SAMPLE_RATE as f32).max(0.0).sqrt())
            .collect();
        let mut tone1 = 0usize;
        let mut max1 = 0.0f32;
        for (i, &a) in powers.iter().enumerate() {
            if a > max1 {
                max1 = a;
                tone1 = i;
            }
        }
        let mut tone2 = 0usize;
        let mut max2 = 0.0f32;
        for (i, &a) in powers.iter().enumerate() {
            if i == tone1 {
                continue;
            }
            if a > max2 {
                max2 = a;
                tone2 = i;
            }
        }
        // Regular Throb: a doubled single tone (amp1 > 2·amp2). ref: throb.cxx:324-329.
        if !self.v.is_throbx() && max1 > max2 * 2.0 {
            tone2 = tone1;
        }
        if tone1 > tone2 {
            std::mem::swap(&mut tone1, &mut tone2);
        }
        (tone1, tone2)
    }

    /// Decode one symbol's tone pair into 0..1 characters. ref: throb.cxx:357-417.
    fn decode(&mut self, tone1: usize, tone2: usize) -> Option<u8> {
        if self.v.is_throbx() {
            let pair = (tone1 as u8, tone2 as u8);
            let idx = THROBX_TONEPAIRS.iter().position(|&p| p == pair)?;
            if idx == self.spacesym || idx == self.idlesym {
                let emit = if self.lastchar != 0 && self.lastchar != b' ' {
                    self.lastchar = b' ';
                    Some(b' ')
                } else {
                    self.lastchar = 0;
                    None
                };
                self.flip();
                emit
            } else {
                let c = THROBX_CHARSET[idx];
                self.lastchar = c;
                Some(c)
            }
        } else {
            if self.shift {
                self.shift = false;
                return match (tone1, tone2) {
                    (0, 8) => Some(b'?'),
                    (1, 7) => Some(b'@'),
                    (2, 6) => Some(b'='),
                    (4, 4) => Some(b'\n'),
                    _ => None,
                };
            }
            if tone1 == 3 && tone2 == 5 {
                self.shift = true;
                return None;
            }
            let pair = (tone1 as u8, tone2 as u8);
            let idx = THROB_TONEPAIRS.iter().position(|&p| p == pair)?;
            let c = THROB_CHARSET[idx];
            if c == 0 {
                None // idle
            } else {
                Some(c)
            }
        }
    }

    fn drain_symbols(&mut self) -> Vec<Frame> {
        let symlen = self.v.symlen();
        let mut out = Vec::new();
        let mut consumed = 0;
        while self.buf.len() - consumed >= symlen {
            // Detect on the buffer slice in place (no per-symbol copy).
            let (t1, t2) = self.find_tones(&self.buf[consumed..consumed + symlen]);
            if let Some(c) = self.decode(t1, t2) {
                push_char(&mut out, c, self.v);
            }
            consumed += symlen;
        }
        self.buf.drain(..consumed);
        out
    }
}

fn push_char(out: &mut Vec<Frame>, c: u8, v: ThrobVariant) {
    out.push(Frame {
        payload: FramePayload::Text((c as char).to_string()),
        meta: FrameMeta {
            crc_ok: true,
            decoder: Some(v.label().to_string()),
            ..Default::default()
        },
    });
}

impl Demodulator for ThrobDemod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.buf.extend_from_slice(samples);
        self.drain_symbols()
    }

    fn reset(&mut self) {
        self.buf.clear();
        let (idlesym, spacesym) = if self.v.is_throbx() { (0, 1) } else { (0, 44) };
        self.idlesym = idlesym;
        self.spacesym = spacesym;
        self.lastchar = 0;
        self.shift = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- bit-exact KATs mirrored from the fldigi golden vector --------------
    // These run in plain `cargo test` (CI does not enable `testutil`); the
    // reference-vector KAT in tests/kat.rs asserts against the same JSON.
    // Provenance: tests/vectors/throb.json.

    #[test]
    fn tonepair_tables_are_sized() {
        assert_eq!(THROB_TONEPAIRS.len(), 45);
        assert_eq!(THROBX_TONEPAIRS.len(), 55);
        assert_eq!(THROB_CHARSET.len(), 45);
        assert_eq!(THROBX_CHARSET.len(), 55);
        // Idle (regular) is the doubled center tone; SHIFT is the {4,6}->(3,5) pair.
        assert_eq!(THROB_TONEPAIRS[0], (4, 4));
        assert_eq!(THROB_TONEPAIRS[5], (3, 5));
    }

    #[test]
    fn text_tones_match_reference_throb_cq() {
        // ref: throb.json message "CQ DE K1ABC", mode "throb".
        let want: Vec<(u8, u8)> = vec![
            (4, 4), (4, 4), (4, 4), (4, 4), (0, 2), (1, 8), (1, 6), (0, 3), (8, 8), (1, 6),
            (1, 2), (2, 8), (3, 4), (0, 1), (0, 2),
        ];
        assert_eq!(text_tones(ThrobVariant::Throb1, "CQ DE K1ABC"), want);
    }

    #[test]
    fn text_tones_match_reference_throbx_cq() {
        // ref: throb.json message "CQ DE K1ABC", mode "throbx".
        let got = text_tones(ThrobVariant::ThrobX1, "CQ DE K1ABC");
        // 4 preamble idles (idle flips 0<->1) + 11 chars.
        assert_eq!(got.len(), 15);
        // First 4 are the alternating idle symbols 0,1,0,1.
        assert_eq!(got[0], THROBX_TONEPAIRS[0]);
        assert_eq!(got[1], THROBX_TONEPAIRS[1]);
        assert_eq!(got[2], THROBX_TONEPAIRS[0]);
        assert_eq!(got[3], THROBX_TONEPAIRS[1]);
        // 'C' -> ThrobXCharSet index 4 -> (1,6).
        assert_eq!(got[4], THROBX_TONEPAIRS[4]);
    }

    #[test]
    fn regular_shift_chars_prefix_shift_symbol() {
        // '?' -> SHIFT(5) + 20; ref: throb.cxx:654-657.
        let syms = text_symbols(ThrobVariant::Throb1, "?");
        assert_eq!(&syms[PREAMBLE..], &[5, 20]);
    }

    #[test]
    fn labels_round_trip() {
        for &v in ThrobVariant::all() {
            assert_eq!(ThrobVariant::from_label(v.label()), Some(v));
        }
        assert_eq!(ThrobVariant::from_label("throb9"), None);
        assert_eq!(ThrobVariant::all().len(), 6);
    }

    #[test]
    fn params_are_correct() {
        assert_eq!(ThrobVariant::Throb1.symlen(), 8192);
        assert_eq!(ThrobVariant::Throb4.symlen(), 2048);
        assert_eq!(ThrobVariant::Throb1.num_tones(), 9);
        assert_eq!(ThrobVariant::ThrobX1.num_tones(), 11);
        assert!(ThrobVariant::ThrobX2.is_throbx());
        assert!(!ThrobVariant::Throb2.is_throbx());
        assert!((ThrobVariant::Throb4.baud() - 3.90625).abs() < 1e-4);
    }

    // ---- loopback gates -----------------------------------------------------

    fn loopback(v: ThrobVariant, msg: &str) -> String {
        let mut tx = ThrobMod::new(v, 1500.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = ThrobDemod::new(v, 1500.0);
        let frames = rx.feed(&samples);
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn loopback_recovers_text_all_submodes() {
        // Uppercase + digits + space round-trip cleanly on every submode.
        let msg = "CQ DE K1ABC 73";
        for &v in ThrobVariant::all() {
            assert_eq!(loopback(v, msg), msg, "submode {}", v.label());
        }
    }

    #[test]
    fn loopback_regular_shift_chars_round_trip() {
        // '?' '@' '\n' round-trip through the SHIFT symbol.
        assert_eq!(loopback(ThrobVariant::Throb2, "WHO?"), "WHO?");
        assert_eq!(loopback(ThrobVariant::Throb2, "A@B"), "A@B");
    }

    #[test]
    fn loopback_throbx_punctuation() {
        // ThrobX carries punctuation directly (no shift).
        assert_eq!(loopback(ThrobVariant::ThrobX2, "HI! (73)"), "HI! (73)");
    }

    #[test]
    fn lowercase_folds_to_upper() {
        assert_eq!(loopback(ThrobVariant::Throb2, "cq"), "CQ");
    }

    #[test]
    fn throbx_consecutive_spaces_collapse_like_fldigi() {
        // ThrobX collapses a run of idle/space symbols to a single space (the RX
        // `lastchar` logic must stay in sync with the TX flip). ref: throb.cxx:399-407.
        assert_eq!(loopback(ThrobVariant::ThrobX2, "A   B"), "A B");
    }
}
