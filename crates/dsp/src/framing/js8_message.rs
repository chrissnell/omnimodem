//! JS8 message packing — the base 87-bit frame (`genjs8`'s 12-char/6-bit path).
//!
//! Port of the message-bit assembly in js8call `lib/js8/genjs8.f90` (upstream @
//! a7ff1be): a 12-character message over the 67-symbol alphabet packed as
//! 12 × 6-bit words (72 bits), a 3-bit frame type (`i3bit`), and a 12-bit CRC
//! ([`crate::fec::ldpc_js8::js8_crc12`]) → the 87 message bits fed to
//! `encode174`. Bit layout (`1003 format(12b6.6,b3.3,b12.12)`):
//! `[72 message | 3 i3bit | 12 CRC]`, all MSB-first.
//!
//! This is the low-level frame codec. JS8's directed-protocol frame *types*
//! (heartbeat/compound/directed, and the JSC-compressed `FrameData`) pack their
//! 72-bit payloads via `varicode.cpp` and layer on top of this — that layer is
//! ported separately. `i3bit` values: ref: varicode.h:50-57.
//!
//! **CRC note:** the 12-bit CRC uses the augmented `crc12` transcription
//! ([`js8_crc12`]); its on-air authority is the cross-decode gate (boost is
//! unavailable to capture a native golden CRC). Pack/unpack are self-consistent.

use crate::fec::ldpc_js8::js8_crc12;
use crate::framing::js8_frames::{
    directed_cmd_name, is_snr_command, unpack_compound_frame, unpack_directed_frame, CompoundFrame,
    DirectedFrame, FRAME_DIRECTED,
};

/// JS8's 67-symbol message alphabet. Char → 0-based index; only the low 6 bits
/// of the index survive `genjs8`'s `b6.6` packing (indices 64–66 alias to
/// 0–2, a reference quirk). ref: genjs8.f90:34.
pub const JS8_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-+/?.";

/// Message length in characters (12 × 6 bits = 72 bits).
pub const MSG_CHARS: usize = 12;
/// Total message bits fed to the LDPC encoder.
pub const KK: usize = 87;

fn alphabet_index(c: u8) -> u32 {
    // Uppercase mapping mirrors genjs8's expectation that callers pre-format the
    // message into the alphabet; unknown chars pack as 0 (like the Fortran's
    // `index()==0 → 0` after `v-1` would underflow, but callers should avoid
    // them). We clamp to a valid index and keep only the low 6 bits at pack time.
    JS8_ALPHABET.iter().position(|&a| a == c).map(|p| p as u32).unwrap_or(0)
}

/// Pack a (≤12-char) message + 3-bit frame type into the 87 LDPC message bits.
/// The message is space-padded to 12 chars. ref: genjs8.f90:33-61.
pub fn pack_msgbits(msg: &str, i3bit: u8) -> [u8; KK] {
    // 12 chars, padded with the alphabet's space-equivalent (index 0 slot is
    // '0'; genjs8 pads `msgsent` with spaces, but spaces aren't in the alphabet
    // — callers supply a formatted 12-char field. We pad with '0' to a defined
    // 6-bit word so pack/unpack round-trip.)
    let bytes = msg.as_bytes();
    let mut words = [0u32; MSG_CHARS];
    for (i, w) in words.iter_mut().enumerate() {
        let c = bytes.get(i).copied().unwrap_or(b'0');
        *w = alphabet_index(c) & 0x3F; // low 6 bits (b6.6)
    }

    let mut bits = [0u8; KK];
    // 72 message bits: 12 words × 6 bits, MSB-first.
    for (i, &w) in words.iter().enumerate() {
        for b in 0..6 {
            bits[i * 6 + b] = ((w >> (5 - b)) & 1) as u8;
        }
    }
    // 3 i3bit bits (bits 72..75), MSB-first.
    for b in 0..3 {
        bits[72 + b] = (i3bit >> (2 - b)) & 1;
    }

    // CRC-12 over the 11-byte buffer: 9 bytes of message (72 bits), then
    // byte[9] = i3bit<<5, byte[10] = 0. ref: genjs8.f90:44-56.
    let mut buf = [0u8; 11];
    for (i, byte) in buf.iter_mut().take(9).enumerate() {
        let mut v = 0u8;
        for b in 0..8 {
            v = (v << 1) | bits[i * 8 + b];
        }
        *byte = v;
    }
    buf[9] = (i3bit & 0x07) << 5;
    buf[10] = 0;
    let crc = js8_crc12(&buf);

    // 12 CRC bits (bits 75..87), MSB-first.
    for b in 0..12 {
        bits[75 + b] = ((crc >> (11 - b)) & 1) as u8;
    }
    bits
}

/// The 3-bit frame type carried at bits 72..75.
pub fn frame_type(bits: &[u8; KK]) -> u8 {
    (bits[72] << 2) | (bits[73] << 1) | bits[74]
}

/// Unpack the 87 LDPC message bits back to `(message, i3bit)`, verifying the
/// CRC. Returns `None` if the CRC check fails. The message is the 12-char field
/// (trailing `'0'` padding preserved — callers trim as appropriate).
pub fn unpack_msgbits(bits: &[u8; KK]) -> Option<(String, u8)> {
    let i3bit = frame_type(bits);
    // Recompute and check the CRC.
    let mut buf = [0u8; 11];
    for (i, byte) in buf.iter_mut().take(9).enumerate() {
        let mut v = 0u8;
        for b in 0..8 {
            v = (v << 1) | bits[i * 8 + b];
        }
        *byte = v;
    }
    buf[9] = (i3bit & 0x07) << 5;
    buf[10] = 0;
    let want = js8_crc12(&buf);
    let mut got = 0u16;
    for b in 0..12 {
        got = (got << 1) | bits[75 + b] as u16;
    }
    if got != want {
        return None;
    }
    // Recover the 12 chars.
    let mut msg = String::with_capacity(MSG_CHARS);
    for i in 0..MSG_CHARS {
        let mut w = 0u32;
        for b in 0..6 {
            w = (w << 1) | bits[i * 6 + b] as u32;
        }
        msg.push(JS8_ALPHABET[w as usize] as char);
    }
    Some((msg, i3bit))
}

// ---------------------------------------------------------------------------
// JSC data-frame transport (FrameData / fast-data)
// ---------------------------------------------------------------------------
//
// JS8 sends arbitrary-length text as a sequence of 72-bit frames whose payload
// is JSC-compressed (`framing::jsc`). In the "fast-data" design the `i3bit`
// header carries the flags and the whole 72-bit payload is compressed data +
// pad — there is no in-payload header. ref: varicode.cpp `packCompressedMessage`
// (prefix `{}`), `unpackFastDataMessage`, `buildMessageFrames`.

/// `i3bit` transmission-type flags (a 3-bit field). ref: varicode.h:36-39.
pub const JS8_FIRST: u8 = 1; // first frame of a message
pub const JS8_LAST: u8 = 2; // last frame of a message
pub const JS8_DATA: u8 = 4; // JS8CallData: flagged (no in-payload frame-type header)

/// Frame payload width in bits (`= 12 chars × 6 = 64-bit value + 8-bit rem`).
pub const FRAME_BITS: usize = 72;

/// Pack a raw 72-bit frame payload + `i3bit` into the 87 LDPC message bits
/// (`[72 payload | 3 i3bit | 12 CRC]`). This is the general form of
/// [`pack_msgbits`]; directed / data frames supply their own 72 payload bits.
pub fn pack_frame(payload: &[bool; FRAME_BITS], i3bit: u8) -> [u8; KK] {
    let mut bits = [0u8; KK];
    for (i, &b) in payload.iter().enumerate() {
        bits[i] = b as u8;
    }
    for b in 0..3 {
        bits[72 + b] = (i3bit >> (2 - b)) & 1;
    }
    // CRC-12 over the same 11-byte buffer form as pack_msgbits (byte9 = i3bit<<5).
    let mut buf = [0u8; 11];
    for (i, byte) in buf.iter_mut().take(9).enumerate() {
        let mut v = 0u8;
        for b in 0..8 {
            v = (v << 1) | bits[i * 8 + b];
        }
        *byte = v;
    }
    buf[9] = (i3bit & 0x07) << 5;
    let crc = js8_crc12(&buf);
    for b in 0..12 {
        bits[75 + b] = ((crc >> (11 - b)) & 1) as u8;
    }
    bits
}

/// Pack the front of `text` into one fast-data frame: JSC-compress, append as
/// many whole codewords as fit under 72 bits, then pad (`0` then all `1`).
/// Returns `(72 payload bits, chars_consumed)`. ref: packCompressedMessage.
pub fn pack_fast_data(text: &str) -> ([bool; FRAME_BITS], usize) {
    use crate::framing::jsc;
    let mut frame: Vec<bool> = Vec::with_capacity(FRAME_BITS);
    let mut chars = 0usize;
    for (bits, n) in jsc::compress(text) {
        if frame.len() + bits.len() < FRAME_BITS {
            frame.extend(bits);
            chars += n as usize;
        } else {
            break;
        }
    }
    // Pad: first pad bit 0, the rest 1 (unpad seeks the last 0 from the end).
    let pad = FRAME_BITS - frame.len();
    for i in 0..pad {
        frame.push(i != 0);
    }
    let mut out = [false; FRAME_BITS];
    out.copy_from_slice(&frame);
    (out, chars)
}

/// Unpack a fast-data frame payload: strip the pad (everything after the last
/// `0`) and JSC-decompress. ref: unpackFastDataMessage (`JS8_FAST_DATA_CAN_USE_HUFF=0`).
pub fn unpack_fast_data(payload: &[bool; FRAME_BITS]) -> String {
    use crate::framing::jsc;
    // n = lastIndexOf(0); bits = payload[0..n].
    let n = payload.iter().rposition(|&b| !b).unwrap_or(0);
    jsc::decompress(&payload[0..n])
}

/// Split `text` into successive fast-data frame payloads (each 72 bits). The
/// number of frames depends on how densely JSC packs the text.
pub fn pack_data_frames(text: &str) -> Vec<[bool; FRAME_BITS]> {
    let bytes = text.as_bytes();
    let mut frames = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let rest = std::str::from_utf8(&bytes[pos..]).unwrap_or("");
        let (frame, chars) = pack_fast_data(rest);
        // Guard against a stall (a codeword that can't fit shouldn't happen for
        // in-dictionary text, but never loop forever).
        let advance = chars.max(1);
        frames.push(frame);
        pos += advance;
        if chars == 0 {
            break;
        }
    }
    frames
}

/// Reassemble the text carried by an ordered list of fast-data frame payloads.
pub fn unpack_data_frames(frames: &[[bool; FRAME_BITS]]) -> String {
    frames.iter().map(unpack_fast_data).collect()
}

/// Verify the 12-bit CRC of an 87-bit frame (shared by the unpack routers).
fn crc_ok(bits: &[u8; KK]) -> bool {
    let i3bit = frame_type(bits);
    let mut buf = [0u8; 11];
    for (i, byte) in buf.iter_mut().take(9).enumerate() {
        let mut v = 0u8;
        for b in 0..8 {
            v = (v << 1) | bits[i * 8 + b];
        }
        *byte = v;
    }
    buf[9] = (i3bit & 0x07) << 5;
    let want = js8_crc12(&buf);
    let mut got = 0u16;
    for b in 0..12 {
        got = (got << 1) | bits[75 + b] as u16;
    }
    got == want
}

fn payload_bits(bits: &[u8; KK]) -> [bool; FRAME_BITS] {
    let mut payload = [false; FRAME_BITS];
    for (i, p) in payload.iter_mut().enumerate() {
        *p = bits[i] != 0;
    }
    payload
}

/// A decoded JS8 frame, routed by frame type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Js8Frame {
    /// Free-text (JSC-compressed) data frame.
    Data(String),
    /// Heartbeat / compound frame (a compound callsign + type/num/bits3).
    Compound(CompoundFrame),
    /// Directed call (from → to + command [+ number/SNR]).
    Directed(DirectedFrame),
}

impl Js8Frame {
    /// A human-readable rendering of the frame (approximate JS8Call display;
    /// the exact wording is a UI concern validated by cross-decode).
    pub fn display(&self) -> String {
        match self {
            Js8Frame::Data(t) => t.clone(),
            Js8Frame::Compound(c) => c.callsign.clone(),
            Js8Frame::Directed(d) => {
                let cmd = directed_cmd_name(d.cmd).unwrap_or("");
                let mut s = format!("{}: {}{}", d.from, d.to, cmd);
                if d.inum != 0 {
                    let val = d.inum as i32 - 31;
                    if is_snr_command(d.cmd) {
                        s.push_str(&format!(" {val:+03}"));
                    } else {
                        s.push_str(&format!(" {val}"));
                    }
                }
                s
            }
        }
    }
}

/// CRC-check an 87-bit frame and decode it into a structured [`Js8Frame`],
/// routing by frame type: a `JS8_DATA`-flagged frame is JSC free-text; otherwise
/// the payload's 3-bit type field selects Directed vs Compound/heartbeat.
/// Returns `(frame, i3bit)`, or `None` if the CRC fails or the frame is malformed.
pub fn decode_frame(bits: &[u8; KK]) -> Option<(Js8Frame, u8)> {
    if !crc_ok(bits) {
        return None;
    }
    let i3bit = frame_type(bits);
    let payload = payload_bits(bits);
    if i3bit & JS8_DATA != 0 {
        return Some((Js8Frame::Data(unpack_fast_data(&payload)), i3bit));
    }
    // Non-data: the frame type lives in the payload's first 3 bits.
    let ftype = ((payload[0] as u8) << 2) | ((payload[1] as u8) << 1) | payload[2] as u8;
    if ftype == FRAME_DIRECTED {
        unpack_directed_frame(&payload).map(|d| (Js8Frame::Directed(d), i3bit))
    } else {
        unpack_compound_frame(&payload).map(|c| (Js8Frame::Compound(c), i3bit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pack → unpack round-trips a message drawn from the low-64 alphabet
    /// (uppercase + digits), and the CRC gate rejects a corrupted frame.
    #[test]
    fn pack_unpack_roundtrip() {
        for (msg, i3) in [("K1ABCFN4200", 3u8), ("CQCQCQ00000", 0), ("0123456789AB", 4)] {
            let bits = pack_msgbits(msg, i3);
            assert_eq!(bits.len(), 87);
            assert_eq!(frame_type(&bits), i3);
            let padded = format!("{msg:0<12}");
            let (got, gi3) = unpack_msgbits(&bits).expect("crc ok");
            assert_eq!(got, padded);
            assert_eq!(gi3, i3);
        }
    }

    /// A short text fits in one fast-data frame and round-trips through the
    /// JSC transport (compress → pad → unpad → decompress).
    #[test]
    fn fast_data_single_frame_roundtrip() {
        let text = "HELLO WORLD";
        let (payload, chars) = pack_fast_data(text);
        assert_eq!(chars, text.len(), "one frame should consume the whole short message");
        assert_eq!(unpack_fast_data(&payload), text);
    }

    /// A long text spans multiple fast-data frames and reassembles exactly
    /// (JSC is lossless for in-dictionary uppercase text).
    #[test]
    fn multi_frame_data_roundtrip() {
        let text = "CQ CQ CQ DE K1ABC K1ABC THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG 73";
        let frames = pack_data_frames(text);
        assert!(frames.len() > 1, "expected multiple frames, got {}", frames.len());
        assert_eq!(unpack_data_frames(&frames), text);
    }

    /// A raw 72-bit data payload packs into 87 msgbits with the data flag and a
    /// valid CRC, and the frame type reads back.
    #[test]
    fn data_frame_msgbits_roundtrip() {
        let (payload, _n) = pack_fast_data("TEST 123");
        let bits = pack_frame(&payload, JS8_DATA | JS8_FIRST | JS8_LAST);
        assert_eq!(frame_type(&bits), JS8_DATA | JS8_FIRST | JS8_LAST);
        // The 72 payload bits survive into msgbits[0..72].
        for (i, &b) in payload.iter().enumerate() {
            assert_eq!(bits[i], b as u8);
        }
        // CRC is self-consistent: unpack_msgbits verifies it (payload isn't a
        // valid 12-char field, but the CRC must still check).
        assert!(unpack_msgbits(&bits).is_some(), "data-frame CRC must verify");
    }

    /// `decode_frame` routes a data frame to `Js8Frame::Data`.
    #[test]
    fn decode_frame_routes_data() {
        let (payload, _) = pack_fast_data("CQ K1ABC");
        let bits = pack_frame(&payload, JS8_DATA | JS8_FIRST | JS8_LAST);
        let (frame, _i3) = decode_frame(&bits).expect("crc ok");
        assert_eq!(frame, Js8Frame::Data("CQ K1ABC".to_string()));
        assert_eq!(frame.display(), "CQ K1ABC");
    }

    /// `decode_frame` routes a directed frame (non-data i3bit) to
    /// `Js8Frame::Directed` by the payload type field, with correct fields.
    #[test]
    fn decode_frame_routes_directed() {
        use crate::framing::js8_frames::{directed_cmd_code, pack_directed_frame};
        let cmd = directed_cmd_code(" SNR").unwrap();
        let payload = pack_directed_frame("K1ABC", "W1AW", cmd, 26).unwrap(); // 26-31 = -5 dB
        let bits = pack_frame(&payload, JS8_FIRST | JS8_LAST); // no DATA flag
        let (frame, _i3) = decode_frame(&bits).expect("crc ok");
        match &frame {
            Js8Frame::Directed(d) => {
                assert_eq!(d.from, "K1ABC");
                assert_eq!(d.to, "W1AW");
                assert_eq!(d.cmd, cmd);
                assert_eq!(d.inum, 26);
            }
            other => panic!("expected Directed, got {other:?}"),
        }
        assert_eq!(frame.display(), "K1ABC: W1AW SNR -05");
    }

    /// `decode_frame` routes a compound/heartbeat frame to `Js8Frame::Compound`.
    #[test]
    fn decode_frame_routes_compound() {
        use crate::framing::js8_frames::{pack_compound_frame, FRAME_HEARTBEAT};
        let payload = pack_compound_frame("K1ABC", FRAME_HEARTBEAT, 0, 0).unwrap();
        let bits = pack_frame(&payload, JS8_FIRST | JS8_LAST);
        let (frame, _i3) = decode_frame(&bits).expect("crc ok");
        match &frame {
            Js8Frame::Compound(c) => assert_eq!(c.callsign, "K1ABC"),
            other => panic!("expected Compound, got {other:?}"),
        }
    }

    /// A single flipped message bit fails the CRC check.
    #[test]
    fn crc_rejects_corruption() {
        let mut bits = pack_msgbits("HELLOWORLD00", 3);
        bits[10] ^= 1;
        assert!(unpack_msgbits(&bits).is_none());
    }

    /// The 72 message bits pack MSB-first, 6 bits per alphabet index.
    #[test]
    fn message_bits_are_msb_first_6bit() {
        // '9' is alphabet index 9 = 0b001001.
        let bits = pack_msgbits("9", 0);
        assert_eq!(&bits[0..6], &[0, 0, 1, 0, 0, 1]);
    }
}

