//! JS8 directed-protocol frame assembly — the 72-bit compound/heartbeat frame.
//!
//! Port of `varicode.cpp` `packCompoundFrame` / `unpackCompoundFrame` (upstream
//! `js8call/js8call` @ a7ff1be). A compound frame carries a 50-bit compound
//! callsign ([`super::js8_callsign::pack_alphanumeric50`]) plus a frame type, a
//! 16-bit `num`, and 3 spare `bits3`, laid out over the 72-bit frame payload:
//!
//! ```text
//! [type:3][callsign50:50][num_hi:11] [num_lo:5][bits3:3]   = 72
//! ```
//!
//! The **frame type** lives in the payload's first 3 bits; the transmitted
//! `i3bit` header (see [`super::js8_message`]) separately carries the
//! First/Last/Data transmission flags. Heartbeat and Compound frames both use
//! this layout — they differ only in the `type` value and how the caller fills
//! `num`/`bits3`. Round-trip gated; on-air authority is the cross-decode gate.

use super::js8_callsign::{pack_alphanumeric50, pack_callsign, unpack_alphanumeric50, unpack_callsign};

/// Frame types (payload bits 0..3). ref: varicode.h:50-57.
pub const FRAME_HEARTBEAT: u8 = 0; // [000]
pub const FRAME_COMPOUND: u8 = 1; // [001]
pub const FRAME_COMPOUND_DIRECTED: u8 = 2; // [010]
pub const FRAME_DIRECTED: u8 = 3; // [011]
pub const FRAME_DATA: u8 = 4; // [10X]
pub const FRAME_DATA_COMPRESSED: u8 = 6; // [11X]

/// 72-bit frame payload.
pub type FramePayload72 = [bool; 72];

fn put_bits(dst: &mut [bool], start: usize, value: u64, n: usize) {
    for i in 0..n {
        dst[start + i] = (value >> (n - 1 - i)) & 1 == 1;
    }
}

fn get_bits(src: &[bool], start: usize, n: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..n {
        v = (v << 1) | src[start + i] as u64;
    }
    v
}

/// Assemble a compound frame payload. Returns `None` for a non-compound `type`
/// (Data/Directed) or an un-packable callsign. ref: varicode.cpp:1469-1498.
pub fn pack_compound_frame(callsign: &str, ftype: u8, num: u16, bits3: u8) -> Option<FramePayload72> {
    if ftype == FRAME_DATA || ftype == FRAME_DIRECTED {
        return None;
    }
    let packed_callsign = pack_alphanumeric50(callsign);
    if packed_callsign == 0 {
        return None;
    }
    let packed_11 = (num >> 5) as u64; // high 11 bits
    let packed_5 = (num & 0x1f) as u64; // low 5 bits

    let mut payload = [false; 72];
    put_bits(&mut payload, 0, ftype as u64, 3);
    put_bits(&mut payload, 3, packed_callsign, 50);
    put_bits(&mut payload, 53, packed_11, 11);
    put_bits(&mut payload, 64, packed_5, 5);
    put_bits(&mut payload, 69, (bits3 & 0x07) as u64, 3);
    Some(payload)
}

/// A decoded compound frame: callsign, frame type, `num`, and the 3 spare bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundFrame {
    pub callsign: String,
    pub ftype: u8,
    pub num: u16,
    pub bits3: u8,
}

/// Decode a compound frame payload. Returns `None` if the type is Data/Directed
/// (i.e. not a compound frame). ref: varicode.cpp:1500-1540.
pub fn unpack_compound_frame(payload: &FramePayload72) -> Option<CompoundFrame> {
    let ftype = get_bits(payload, 0, 3) as u8;
    if ftype == FRAME_DATA || ftype == FRAME_DIRECTED {
        return None;
    }
    let packed_callsign = get_bits(payload, 3, 50);
    let packed_11 = get_bits(payload, 53, 11) as u16;
    let packed_5 = get_bits(payload, 64, 5) as u16;
    let bits3 = get_bits(payload, 69, 3) as u8;
    let callsign = unpack_alphanumeric50(packed_callsign);
    let num = (packed_11 << 5) | packed_5;
    Some(CompoundFrame { callsign, ftype, num, bits3 })
}

// ---------------------------------------------------------------------------
// Directed frames (directed calls: TO CMD [NUM])
// ---------------------------------------------------------------------------
//
// A directed frame carries two 28-bit packed callsigns (from/to), a 5-bit
// command, and a rem byte `[portable_from:1][portable_to:1][inum:6]`:
//
//     [type:3][from:28][to:28][cmd:5] [pfrom:1][pto:1][inum:6]   = 72
//
// The `inum` field is the command's number argument offset by 31 (an SNR or a
// generic count); callers encode/decode the offset. The text→(to,cmd,num) parse
// (a `QRegularExpression` in the reference) is a UI concern layered on top.
// ref: varicode.cpp packDirectedMessage / unpackDirectedMessage.

/// Directed command table `(name, code)`. Codes 0..32 pack into the 5-bit field;
/// `-1` marks the faux HEARTBEAT/HB/CQ entries (not real directed commands).
/// ref: varicode.cpp:46-112 (`directed_cmds`).
pub const DIRECTED_CMDS: &[(&str, i8)] = &[
    (" HEARTBEAT", -1), (" HB", -1), (" CQ", -1),
    (" SNR?", 0), ("?", 0),
    (" DIT DIT", 1),
    (" NACK", 2),
    (" HEARING?", 3),
    (" GRID?", 4),
    (">", 5),
    (" STATUS?", 6),
    (" STATUS", 7),
    (" HEARING", 8),
    (" MSG", 9),
    (" MSG TO:", 10),
    (" QUERY", 11),
    (" QUERY MSGS", 12), (" QUERY MSGS?", 12),
    (" QUERY CALL", 13),
    (" ACK", 14),
    (" GRID", 15),
    (" INFO?", 16),
    (" INFO", 17),
    (" FB", 18),
    (" HW CPY?", 19),
    (" SK", 20),
    (" RR", 21),
    (" QSL?", 22),
    (" QSL", 23),
    (" CMD", 24),
    (" SNR", 25),
    (" NO", 26),
    (" YES", 27),
    (" 73", 28),
    (" HEARTBEAT SNR", 29),
    (" AGN?", 30),
    ("  ", 31), (" ", 31),
];

/// Command codes that carry an SNR value in `inum` (`inum - 31` dB).
/// ref: varicode.cpp:123 (`snr_cmds`).
pub fn is_snr_command(code: u8) -> bool {
    code == 25 || code == 29
}

/// Look up a directed command's 5-bit code by name (only real commands, 0..32).
pub fn directed_cmd_code(name: &str) -> Option<u8> {
    DIRECTED_CMDS
        .iter()
        .find(|(n, c)| *n == name && *c >= 0)
        .map(|(_, c)| *c as u8)
}

/// The first (canonical) command name for a code, for display on decode.
pub fn directed_cmd_name(code: u8) -> Option<&'static str> {
    DIRECTED_CMDS.iter().find(|(_, c)| *c == code as i8).map(|(n, _)| *n)
}

/// A decoded directed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectedFrame {
    pub from: String,
    pub to: String,
    pub cmd: u8,
    /// Raw 6-bit number field; `inum - 31` is the SNR/count when non-zero.
    pub inum: u8,
    pub portable_from: bool,
    pub portable_to: bool,
}

/// Assemble a directed frame payload from resolved fields. Returns `None` if a
/// callsign can't be packed. `cmd` is a 5-bit code, `inum` a 6-bit value.
/// ref: varicode.cpp:1600-1640.
pub fn pack_directed_frame(from: &str, to: &str, cmd: u8, inum: u8) -> Option<FramePayload72> {
    let (packed_from, portable_from) = pack_callsign(from);
    let (packed_to, portable_to) = pack_callsign(to);
    if packed_from == 0 || packed_to == 0 {
        return None;
    }
    let mut payload = [false; 72];
    put_bits(&mut payload, 0, FRAME_DIRECTED as u64, 3);
    put_bits(&mut payload, 3, packed_from as u64, 28);
    put_bits(&mut payload, 31, packed_to as u64, 28);
    put_bits(&mut payload, 59, (cmd % 32) as u64, 5);
    // rem byte: [pfrom:1][pto:1][inum:6]
    payload[64] = portable_from;
    payload[65] = portable_to;
    put_bits(&mut payload, 66, (inum & 0x3f) as u64, 6);
    Some(payload)
}

/// Decode a directed frame payload, or `None` if the type field isn't Directed.
/// ref: varicode.cpp:1641-1682.
pub fn unpack_directed_frame(payload: &FramePayload72) -> Option<DirectedFrame> {
    if get_bits(payload, 0, 3) as u8 != FRAME_DIRECTED {
        return None;
    }
    let packed_from = get_bits(payload, 3, 28) as u32;
    let packed_to = get_bits(payload, 31, 28) as u32;
    let cmd = get_bits(payload, 59, 5) as u8;
    let portable_from = payload[64];
    let portable_to = payload[65];
    let inum = get_bits(payload, 66, 6) as u8;
    Some(DirectedFrame {
        from: unpack_callsign(packed_from, portable_from),
        to: unpack_callsign(packed_to, portable_to),
        cmd,
        inum,
        portable_from,
        portable_to,
    })
}

// ---------------------------------------------------------------------------
// TX text parsing: "TO CMD [NUM]" → a directed frame
// ---------------------------------------------------------------------------
//
// The reference parses operator text with `directed_re`; here the structured
// `TO CMD [NUM]` grammar is hand-parsed (no regex dependency). This covers the
// well-formed forms operators type (`W1AW SNR -5`, `W1AW QSL?`, `@ALLCALL
// HEARING?`); exact regex-edge parity is a cross-decode concern. ref:
// varicode.cpp:137-147 (`directed_re`), packNum.

/// Encode a signed number into the 6-bit `inum` field: `clamp(n,-30,31)+31`
/// (so `0` is a present-but-zero value, distinct from "no number" = 0-length).
/// ref: varicode.cpp packNum.
pub fn pack_num(n: i32) -> u8 {
    (n.clamp(-30, 31) + 31) as u8
}

/// A parsed directed message: the `to` callsign, the command code, and the
/// `inum` field (0 = no number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDirected {
    pub to: String,
    pub cmd: u8,
    pub inum: u8,
}

/// Command names (trimmed) with their codes, longest-first so multi-word and
/// `?`-suffixed commands match before their prefixes.
fn commands_by_len() -> Vec<(&'static str, u8)> {
    let mut v: Vec<(&'static str, u8)> = DIRECTED_CMDS
        .iter()
        .filter(|(_, c)| *c >= 0)
        .map(|(n, c)| (n.trim(), *c as u8))
        .filter(|(n, _)| !n.is_empty())
        .collect();
    v.sort_by_key(|c| std::cmp::Reverse(c.0.len()));
    v
}

/// Parse `text` as a directed message `TO CMD [NUM]`. Returns `None` if it
/// doesn't start with a callsign followed by a recognised command.
///
/// Known divergences from the reference `directed_re` (the exact frame is
/// gated by cross-decode, and both this and the reference produce the same
/// frame for the canonical forms operators type):
/// - the SNR number must be the whole remainder (`W1AW SNR -5`); a trailing
///   token after the number (`W1AW SNR -5 GRID`) yields `inum = 0` here,
///   whereas the reference regex still captures the number;
/// - grid arguments (`optional_grid_pattern`) are not parsed — grids are
///   carried by compound frames, not the base directed frame, in this port.
pub fn parse_directed(text: &str) -> Option<ParsedDirected> {
    let text = text.trim_start();
    // callsign: [@]?[A-Z0-9/]+
    let bytes = text.as_bytes();
    let mut i = 0;
    if bytes.first() == Some(&b'@') {
        i = 1;
    }
    let start = if i == 1 { 1 } else { 0 };
    while i < bytes.len() && (bytes[i].is_ascii_uppercase() || bytes[i].is_ascii_digit() || bytes[i] == b'/') {
        i += 1;
    }
    if i == start {
        return None; // no callsign body
    }
    let to = text[..i].to_string();
    let rest = &text[i..];

    // Optional single leading space before the command.
    let r = rest.strip_prefix(' ').unwrap_or(rest);

    for (name, code) in commands_by_len() {
        if let Some(after) = r.strip_prefix(name) {
            // A word-command must be followed by space/end; punctuation-suffixed
            // commands (`?`, `:`, `>`) match directly.
            let last = name.bytes().last().unwrap();
            let word_boundary_ok = !last.is_ascii_alphanumeric()
                || after.is_empty()
                || after.starts_with(' ');
            if !word_boundary_ok {
                continue;
            }
            // Optional number (used by SNR): trailing signed integer.
            let num_str = after.trim();
            let inum = if is_snr_command(code) && !num_str.is_empty() {
                match num_str.parse::<i32>() {
                    Ok(n) => pack_num(n),
                    Err(_) => 0,
                }
            } else {
                0
            };
            return Some(ParsedDirected { to, cmd: code, inum });
        }
    }
    None
}

/// Build a directed frame + `i3bit` from operator text and the station's own
/// call, or `None` if the text isn't a directed message. `first`/`last` mark the
/// transmission boundaries (both true for a single window).
pub fn build_directed(mycall: &str, text: &str, first: bool, last: bool) -> Option<(FramePayload72, u8)> {
    let p = parse_directed(text)?;
    let payload = pack_directed_frame(mycall, &p.to, p.cmd, p.inum)?;
    let mut i3 = 0u8;
    if first {
        i3 |= super::js8_message::JS8_FIRST;
    }
    if last {
        i3 |= super::js8_message::JS8_LAST;
    }
    Some((payload, i3))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::js8_message::{decode_frame, pack_frame, Js8Frame};

    /// Compound frames round-trip: callsign, type, num, and bits3 all survive.
    #[test]
    fn compound_frame_roundtrip() {
        for (call, ftype, num, bits3) in [
            ("K1ABC", FRAME_HEARTBEAT, 0u16, 0u8),
            ("VE3/K1ABC", FRAME_COMPOUND, 1234, 5),
            ("W1AW", FRAME_COMPOUND_DIRECTED, 0xFFFF, 7),
            ("@ALLCALL", FRAME_HEARTBEAT, 42, 3),
        ] {
            let payload = pack_compound_frame(call, ftype, num, bits3).expect("packable");
            let got = unpack_compound_frame(&payload).expect("decodable");
            assert_eq!(got.callsign, call, "callsign mismatch");
            assert_eq!(got.ftype, ftype, "type mismatch");
            assert_eq!(got.num, num, "num mismatch");
            assert_eq!(got.bits3, bits3, "bits3 mismatch");
        }
    }

    /// Data / Directed types are rejected by the compound packer/unpacker.
    #[test]
    fn rejects_non_compound_types() {
        assert!(pack_compound_frame("K1ABC", FRAME_DATA, 0, 0).is_none());
        assert!(pack_compound_frame("K1ABC", FRAME_DIRECTED, 0, 0).is_none());
        // A payload whose type field is Directed unpacks to None.
        let mut payload = pack_compound_frame("K1ABC", FRAME_COMPOUND, 0, 0).unwrap();
        put_bits(&mut payload, 0, FRAME_DIRECTED as u64, 3);
        assert!(unpack_compound_frame(&payload).is_none());
    }

    /// The 16-bit `num` splits into 11 high + 5 low bits and recombines exactly.
    // The 11_5 underscore grouping mirrors that bit split on purpose, so keep it.
    #[allow(clippy::unusual_byte_groupings)]
    #[test]
    fn num_split_recombine() {
        let payload = pack_compound_frame("K1ABC", FRAME_COMPOUND, 0b1010_1010_101_10011, 0).unwrap();
        let got = unpack_compound_frame(&payload).unwrap();
        assert_eq!(got.num, 0b1010_1010_101_10011);
    }

    /// Directed frames round-trip: from/to callsigns, command, and inum survive.
    #[test]
    fn directed_frame_roundtrip() {
        let snr = directed_cmd_code(" SNR").unwrap();
        for (from, to, cmd, inum) in [
            ("K1ABC", "W1AW", directed_cmd_code(" SNR?").unwrap(), 0u8),
            ("K1ABC", "W1AW", snr, 42),
            ("N5AC", "VK3ABC", directed_cmd_code(" 73").unwrap(), 0),
            ("K1ABC", "@ALLCALL", directed_cmd_code(" QSL?").unwrap(), 0),
        ] {
            let payload = pack_directed_frame(from, to, cmd, inum).expect("packable");
            let got = unpack_directed_frame(&payload).expect("decodable");
            assert_eq!(got.from, from, "from mismatch");
            assert_eq!(got.to, to, "to mismatch");
            assert_eq!(got.cmd, cmd, "cmd mismatch");
            assert_eq!(got.inum, inum, "inum mismatch");
        }
    }

    /// Portable `/P` on the `to` field round-trips via the frame's portable bit.
    #[test]
    fn directed_portable_roundtrip() {
        let cmd = directed_cmd_code(" RR").unwrap();
        let payload = pack_directed_frame("K1ABC", "W1AW/P", cmd, 0).unwrap();
        let got = unpack_directed_frame(&payload).unwrap();
        assert_eq!(got.to, "W1AW/P");
        assert!(got.portable_to);
    }

    /// Command table: SNR commands are flagged, names resolve to codes and back.
    #[test]
    fn directed_command_table() {
        assert!(is_snr_command(directed_cmd_code(" SNR").unwrap()));
        assert!(is_snr_command(directed_cmd_code(" HEARTBEAT SNR").unwrap()));
        assert!(!is_snr_command(directed_cmd_code(" 73").unwrap()));
        assert_eq!(directed_cmd_code(" ACK"), Some(14));
        assert_eq!(directed_cmd_code(" HEARTBEAT"), None); // faux (-1), not packable
        // Every real code decodes to some canonical name.
        for &(_, c) in DIRECTED_CMDS {
            if c >= 0 {
                assert!(directed_cmd_name(c as u8).is_some());
            }
        }
    }

    /// A non-directed payload (Compound type) is rejected by the directed unpacker.
    #[test]
    fn directed_rejects_compound() {
        let payload = pack_compound_frame("K1ABC", FRAME_COMPOUND, 0, 0).unwrap();
        assert!(unpack_directed_frame(&payload).is_none());
    }

    /// `parse_directed` recognises the common TO CMD [NUM] forms.
    #[test]
    fn parse_directed_forms() {
        let snr = directed_cmd_code(" SNR").unwrap();
        let cases = [
            ("W1AW SNR -5", "W1AW", snr, pack_num(-5)),
            ("W1AW SNR?", "W1AW", directed_cmd_code(" SNR?").unwrap(), 0),
            ("W1AW QSL?", "W1AW", directed_cmd_code(" QSL?").unwrap(), 0),
            ("W1AW 73", "W1AW", directed_cmd_code(" 73").unwrap(), 0),
            ("W1AW ACK", "W1AW", directed_cmd_code(" ACK").unwrap(), 0),
            ("@ALLCALL HEARING?", "@ALLCALL", directed_cmd_code(" HEARING?").unwrap(), 0),
            ("W1AW HW CPY?", "W1AW", directed_cmd_code(" HW CPY?").unwrap(), 0),
        ];
        for (text, to, cmd, inum) in cases {
            let p = parse_directed(text).unwrap_or_else(|| panic!("failed to parse {text:?}"));
            assert_eq!(p.to, to, "to for {text:?}");
            assert_eq!(p.cmd, cmd, "cmd for {text:?}");
            assert_eq!(p.inum, inum, "inum for {text:?}");
        }
    }

    /// Plain free text (no command) does not parse as a directed message.
    #[test]
    fn parse_directed_rejects_freetext() {
        assert!(parse_directed("HELLO WORLD").is_none());
        assert!(parse_directed("CQ CQ DE K1ABC").is_none());
    }

    /// End-to-end: parse operator text → build a directed frame → decode it back
    /// to the rendered directed message.
    #[test]
    fn build_directed_roundtrip_through_decode() {
        let (payload, i3) = build_directed("K1ABC", "W1AW SNR -5", true, true).unwrap();
        let bits = pack_frame(&payload, i3);
        let (frame, _i3) = decode_frame(&bits).expect("crc ok");
        assert_eq!(frame, Js8Frame::Directed(unpack_directed_frame(&payload).unwrap()));
        assert_eq!(frame.display(), "K1ABC: W1AW SNR -05");
    }

    /// `build_directed` declines free text (so the caller falls back to a data frame).
    #[test]
    fn build_directed_declines_freetext() {
        assert!(build_directed("K1ABC", "HELLO WORLD", true, true).is_none());
    }

    /// Convert a reference `pack72bits` 12-char frame string into the 72-bit
    /// payload: each char is a 6-bit `alphabet72` index, MSB-first, giving
    /// `value(64) || rem(8)`.
    fn frame_str_to_payload(s: &str) -> FramePayload72 {
        use super::super::js8_message::JS8_ALPHABET;
        let mut out = [false; 72];
        for (ci, ch) in s.bytes().enumerate() {
            let idx = JS8_ALPHABET.iter().position(|&a| a == ch).unwrap() as u8;
            for b in 0..6 {
                out[ci * 6 + b] = (idx >> (5 - b)) & 1 == 1;
            }
        }
        out
    }

    /// Bit-exact vs the real `varicode.cpp` `packCompoundFrame` (Qt build). Golden
    /// 12-char frame strings from `scratch/refvectors/js8/framesqt/frames_dump.cpp`.
    #[test]
    fn pack_compound_frame_matches_reference() {
        // (callsign, type, num, bits3, reference 12-char frame)
        let cases: &[(&str, u8, u16, u8, &str)] = &[
            ("K1ABC", FRAME_HEARTBEAT, 0, 0, "2URtg4DOO000"),
            ("VE3/K1ABC", FRAME_COMPOUND, 1234, 5, "Bu6RmCOxm2QL"),
        ];
        for (call, ftype, num, bits3, refstr) in cases {
            let got = pack_compound_frame(call, *ftype, *num, *bits3).unwrap();
            assert_eq!(got, frame_str_to_payload(refstr), "packCompoundFrame({call}) mismatch");
        }
    }

    /// Bit-exact vs the real `varicode.cpp` `packDirectedMessage` (Qt build).
    #[test]
    fn pack_directed_frame_matches_reference() {
        // build_directed("K1ABC", text) must equal the reference frame.
        let cases: &[(&str, &str)] = &[
            ("W1AW SNR -5", "Vk64SVAPtVaQ"),
            ("W1AW SNR?", "Vk64SVAPtU00"),
        ];
        for (text, refstr) in cases {
            let (payload, _i3) = build_directed("K1ABC", text, true, true).unwrap();
            assert_eq!(payload, frame_str_to_payload(refstr), "packDirectedMessage({text}) mismatch");
        }
    }
}
