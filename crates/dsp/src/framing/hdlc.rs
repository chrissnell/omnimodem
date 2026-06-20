//! HDLC framing: flag delimiting, bit-stuffing, and the CRC-16/X.25 FCS.
//!
//! Bit order: HDLC/AX.25 is **LSB-first on the wire**. Each payload byte is
//! serialised least-significant-bit first; the 16-bit FCS is computed with
//! CRC-16/X.25 (`crate::fec::crc::CRC16_X25`) over the payload bytes and
//! appended **LSB-first** as two bytes (low byte first, each byte LSB-first).
//!
//! Framing rules:
//!   - `0x7E` (`01111110`) is the flag, emitted LSB-first as `0,1,1,1,1,1,1,0`.
//!   - Between flags, a `0` is stuffed after any run of five consecutive `1`s
//!     so the payload can never imitate a flag. The destuffer removes it.
//!
//! [`hdlc_frame`] returns the on-wire bit vector (one `u8` per bit, 0/1).
//! [`hdlc_deframe`] scans a bitstream for flag-delimited frames, destuffs,
//! validates the FCS, and returns the recovered payloads.

use crate::fec::crc::{crc, CRC16_X25};

const FLAG: u8 = 0x7E;

/// Append the CRC-16/X.25 FCS to `payload` (LSB-first, low byte first) and
/// return the byte vector that will be bit-serialised.
fn with_fcs(payload: &[u8]) -> Vec<u8> {
    let fcs = crc(&CRC16_X25, payload) as u16;
    let mut v = payload.to_vec();
    v.push((fcs & 0xFF) as u8); // low byte first
    v.push((fcs >> 8) as u8);
    v
}

/// Serialise a byte LSB-first into the bit sink.
fn push_byte_lsb(bits: &mut Vec<u8>, b: u8) {
    for i in 0..8 {
        bits.push((b >> i) & 1);
    }
}

/// Build an HDLC frame: `FLAG | stuffed(payload+FCS) | FLAG`, returned as a
/// vector of bits (0/1), LSB-first on the wire.
pub fn hdlc_frame(payload: &[u8]) -> Vec<u8> {
    let data = with_fcs(payload);
    let mut bits = Vec::new();
    push_byte_lsb(&mut bits, FLAG);

    let mut ones = 0u8;
    for &b in &data {
        for i in 0..8 {
            let bit = (b >> i) & 1;
            bits.push(bit);
            if bit == 1 {
                ones += 1;
                if ones == 5 {
                    bits.push(0); // stuff
                    ones = 0;
                }
            } else {
                ones = 0;
            }
        }
    }
    push_byte_lsb(&mut bits, FLAG);
    bits
}

/// Scan a bitstream for flag-delimited HDLC frames, destuff, verify the FCS,
/// and return the payloads (FCS stripped). Frames failing the FCS are dropped.
pub fn hdlc_deframe(bits: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    // Sliding 8-bit window to detect the flag pattern (LSB-first).
    let mut window: u8 = 0;
    let mut filled = 0usize;
    let mut collecting = false;
    let mut ones = 0u8;
    let mut payload_bits: Vec<u8> = Vec::new();

    for &bit in bits {
        // Track an 8-bit LSB-first window for flag detection.
        window = (window >> 1) | (bit << 7);
        filled += 1;

        if filled >= 8 && window == FLAG {
            if collecting {
                // Close the frame. While the closing flag's bits streamed in,
                // its leading 7 bits (`0,1,1,1,1,1,1`) were appended to
                // payload_bits before the window matched on the 8th bit; trim
                // them before decoding.
                if payload_bits.len() >= 7 {
                    payload_bits.truncate(payload_bits.len() - 7);
                    finish_frame(&mut payload_bits, &mut frames);
                }
            }
            collecting = true;
            payload_bits.clear();
            ones = 0;
            continue;
        }

        if collecting {
            // Destuff: drop a 0 that follows five 1s. Flag detection above is
            // the sole authority on frame boundaries, so a 1 after five 1s is
            // left in place (it is part of an in-progress flag the window will
            // catch and trim).
            if ones == 5 && bit == 0 {
                ones = 0;
                continue; // stuffed bit removed
            }
            payload_bits.push(bit);
            if bit == 1 {
                ones += 1;
            } else {
                ones = 0;
            }
        }
    }
    frames
}

/// Convert accumulated LSB-first payload bits (flags already trimmed) into
/// bytes, verify the FCS, and push the payload if valid. Any partial trailing
/// byte that is not a whole octet is ignored.
fn finish_frame(payload_bits: &mut Vec<u8>, frames: &mut Vec<Vec<u8>>) {
    let nbytes = payload_bits.len() / 8;
    if nbytes < 3 {
        payload_bits.clear();
        return; // too short to hold even an empty payload + FCS
    }
    let mut bytes = Vec::with_capacity(nbytes);
    for chunk in payload_bits[..nbytes * 8].chunks(8) {
        let mut b = 0u8;
        for (i, &bit) in chunk.iter().enumerate() {
            b |= bit << i;
        }
        bytes.push(b);
    }
    // Split FCS (last two bytes, low byte first).
    let split = bytes.len() - 2;
    let payload = &bytes[..split];
    let got = (bytes[split] as u16) | ((bytes[split + 1] as u16) << 8);
    let want = crc(&CRC16_X25, payload) as u16;
    if got == want {
        frames.push(payload.to_vec());
    }
    payload_bits.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_starts_and_ends_with_flag() {
        let bits = hdlc_frame(b"AB");
        let flag_lsb = [0u8, 1, 1, 1, 1, 1, 1, 0]; // 0x7E LSB-first
        assert_eq!(&bits[..8], &flag_lsb);
        assert_eq!(&bits[bits.len() - 8..], &flag_lsb);
    }

    #[test]
    fn roundtrip_recovers_payload() {
        let payload = b"Hello, AX.25!";
        let bits = hdlc_frame(payload);
        let out = hdlc_deframe(&bits);
        assert_eq!(out, vec![payload.to_vec()]);
    }

    #[test]
    fn single_bit_flip_fails_fcs() {
        let payload = b"test123";
        let mut bits = hdlc_frame(payload);
        // Flip a payload bit (well past the opening flag).
        bits[20] ^= 1;
        let out = hdlc_deframe(&bits);
        assert!(out.is_empty(), "corrupted frame must fail FCS");
    }

    #[test]
    fn stuffing_never_emits_six_ones_outside_flag() {
        // Payload of all-ones bytes forces maximal stuffing.
        let payload = vec![0xFFu8; 16];
        let bits = hdlc_frame(&payload);
        // Skip the opening (first 8) and closing (last 8) flags.
        let inner = &bits[8..bits.len() - 8];
        let mut run = 0;
        for &b in inner {
            if b == 1 {
                run += 1;
                assert!(run < 6, "six consecutive 1s in frame body");
            } else {
                run = 0;
            }
        }
        // And it still round-trips.
        assert_eq!(hdlc_deframe(&bits), vec![payload]);
    }

    #[test]
    fn property_random_payloads_roundtrip() {
        let mut state = 0x1234_5678u32;
        let mut rng = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for _ in 0..200 {
            let len = (rng() % 40) as usize + 1;
            let payload: Vec<u8> = (0..len).map(|_| (rng() & 0xFF) as u8).collect();
            let bits = hdlc_frame(&payload);
            assert_eq!(hdlc_deframe(&bits), vec![payload]);
        }
    }

    #[test]
    fn lsb_first_wire_order_documented_and_asserted() {
        // Payload byte 0x01 serialises LSB-first as 1,0,0,0,0,0,0,0.
        let bits = hdlc_frame(&[0x01]);
        let body = &bits[8..16]; // first payload byte, no stuffing possible
        assert_eq!(body, &[1u8, 0, 0, 0, 0, 0, 0, 0]);
    }
}
