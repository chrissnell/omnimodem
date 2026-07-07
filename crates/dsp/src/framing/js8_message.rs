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
