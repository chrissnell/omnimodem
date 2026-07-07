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

use super::js8_callsign::{pack_alphanumeric50, unpack_alphanumeric50};

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

#[cfg(test)]
mod tests {
    use super::*;

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
    #[test]
    fn num_split_recombine() {
        let payload = pack_compound_frame("K1ABC", FRAME_COMPOUND, 0b1010_1010_101_10011, 0).unwrap();
        let got = unpack_compound_frame(&payload).unwrap();
        assert_eq!(got.num, 0b1010_1010_101_10011);
    }
}
