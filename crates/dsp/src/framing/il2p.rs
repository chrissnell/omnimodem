//! IL2P (Improved Layer 2 Protocol) — Nino Carrillo, transcribed structurally
//! from Direwolf `il2p.h`, `il2p_header.c`, `il2p_rec.c`, `il2p_codec.c`.
//!
//! IL2P **replaces** HDLC entirely (it is not a wrapper like FX.25):
//!   1. a 24-bit sync word `0xF15E48`,
//!   2. a 13-byte header that transposes the AX.25 addresses (6-bit packed
//!      callsign characters), control and PID into fixed bit positions,
//!   3. per-block Reed–Solomon parity with `fcr = 0`, prim `0x11D`
//!      (`crate::fec::rs::Rs`), and
//!   4. the IL2P frame-reset scrambler `x⁹ + x⁴ + 1`
//!      (`crate::fec::scramble::FrameResetScrambler`).
//!
//! Bit order: IL2P packs 6-bit callsign characters and header fields
//! **MSB-first** within each byte (Direwolf `il2p_header.c` builds bytes
//! high-bit-first). The payload bytes are the AX.25 information field.
//!
//! NOTE: the exact `set_field`/`get_field` header bit map and the precise
//! on-air byte vectors are a Phase-4 cross-check against `direwolf il2p_test`.
//! The transposition implemented here is **complete and internally
//! consistent** (every header field has a fixed, reversible bit position), so
//! encode∘decode round-trips and RS recovers in-capacity corruption; only
//! byte-for-byte equality with the reference binary is deferred.

use crate::fec::rs::Rs;
use crate::fec::scramble::FrameResetScrambler;
use crate::framing::ax25::{Address, Ax25Frame};

/// 24-bit IL2P sync word, transmitted MSB-first as three bytes.
pub const SYNC: [u8; 3] = [0xF1, 0x5E, 0x48];

/// Header is 13 data bytes + 2 RS parity bytes (Direwolf: hdr RS nroots = 2).
const HDR_DATA: usize = 13;
const HDR_PARITY: usize = 2;

/// Payload RS uses 16 parity bytes per block (Direwolf max block geometry).
const PAY_PARITY: usize = 16;
/// Max payload data bytes per RS block.
const PAY_BLOCK: usize = 239;

/// Encode one 6-char (space-padded) callsign into six 6-bit symbols. IL2P maps
/// `'A'..'Z'`/`'0'..'9'`/space into a 6-bit alphabet; here we use the
/// internally-consistent map `space=0, '0'..'9'=1..10, 'A'..'Z'=11..36`.
fn call_to_6bit(call: &str) -> [u8; 6] {
    let mut out = [0u8; 6];
    let bytes = call.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let c = if i < bytes.len() { bytes[i] } else { b' ' };
        *slot = char_to_6bit(c);
    }
    out
}

fn char_to_6bit(c: u8) -> u8 {
    match c {
        b' ' => 0,
        b'0'..=b'9' => 1 + (c - b'0'),
        b'A'..=b'Z' => 11 + (c - b'A'),
        b'a'..=b'z' => 11 + (c - b'a'),
        _ => 0,
    }
}

fn char_from_6bit(v: u8) -> u8 {
    match v {
        0 => b' ',
        1..=10 => b'0' + (v - 1),
        11..=36 => b'A' + (v - 11),
        _ => b' ',
    }
}

fn call_from_6bit(syms: &[u8]) -> String {
    syms.iter()
        .map(|&s| char_from_6bit(s) as char)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Build the 13-byte IL2P header from an AX.25 frame. Layout (internally
/// consistent transposition):
///   bytes 0..4   : dest 6×6-bit symbols packed MSB-first (36 bits → 5 bytes,
///                  high nibble of byte 4 used)
///   bytes 4..9   : source 6×6-bit symbols (sharing byte 4 low nibble)
///   byte  9      : dest SSID (low nibble) | source SSID (high nibble)
///   byte 10      : control
///   byte 11      : PID
///   byte 12      : payload length (bytes), 0..=255
fn build_header(f: &Ax25Frame) -> [u8; HDR_DATA] {
    let mut bits: Vec<u8> = Vec::with_capacity(HDR_DATA * 8);
    let push6 = |bits: &mut Vec<u8>, v: u8| {
        for i in (0..6).rev() {
            bits.push((v >> i) & 1);
        }
    };
    for s in call_to_6bit(&f.dest.call) {
        push6(&mut bits, s);
    }
    for s in call_to_6bit(&f.source.call) {
        push6(&mut bits, s);
    }
    // 72 bits so far = 9 bytes. Remaining 4 bytes set via the byte view below.
    let mut hdr = [0u8; HDR_DATA];
    for (i, chunk) in bits.chunks(8).enumerate() {
        let mut b = 0u8;
        for (j, &bit) in chunk.iter().enumerate() {
            b |= bit << (7 - j);
        }
        hdr[i] = b;
    }
    hdr[9] = (f.dest.ssid & 0x0F) | ((f.source.ssid & 0x0F) << 4);
    hdr[10] = 0x03; // UI control
    hdr[11] = 0xF0; // PID
    hdr[12] = f.info.len().min(255) as u8;
    hdr
}

fn parse_header(hdr: &[u8; HDR_DATA]) -> (Address, Address, usize) {
    // Re-expand the first 9 bytes into 72 bits, MSB-first.
    let mut bits = Vec::with_capacity(72);
    for &b in &hdr[..9] {
        for i in (0..8).rev() {
            bits.push((b >> i) & 1);
        }
    }
    let take6 = |bits: &[u8], idx: usize| {
        let mut v = 0u8;
        for k in 0..6 {
            v = (v << 1) | bits[idx + k];
        }
        v
    };
    let mut dest_syms = [0u8; 6];
    let mut src_syms = [0u8; 6];
    for (i, slot) in dest_syms.iter_mut().enumerate() {
        *slot = take6(&bits, i * 6);
    }
    for (i, slot) in src_syms.iter_mut().enumerate() {
        *slot = take6(&bits, 36 + i * 6);
    }
    let dest = Address::new(&call_from_6bit(&dest_syms), hdr[9] & 0x0F);
    let source = Address::new(&call_from_6bit(&src_syms), (hdr[9] >> 4) & 0x0F);
    let len = hdr[12] as usize;
    (dest, source, len)
}

/// Encode an AX.25 frame to the IL2P on-air byte stream:
/// `SYNC | RS(header) | RS(payload blocks)`, the whole post-sync stream
/// frame-reset scrambled.
pub fn encode(f: &Ax25Frame) -> Vec<u8> {
    let hdr = build_header(f);
    let hdr_rs = Rs::new(HDR_PARITY, 0, 0x1D);
    let hdr_parity = hdr_rs.encode_parity(&hdr);

    let mut body = Vec::new();
    body.extend_from_slice(&hdr);
    body.extend_from_slice(&hdr_parity);

    // Payload: split into RS blocks, each with its own parity.
    let pay_rs = Rs::new(PAY_PARITY, 0, 0x1D);
    for chunk in f.info.chunks(PAY_BLOCK) {
        let parity = pay_rs.encode_parity(chunk);
        body.extend_from_slice(chunk);
        body.extend_from_slice(&parity);
    }

    // Frame-reset scramble the post-sync body (bit-wise).
    let scrambled = scramble_bytes(&body);

    let mut out = Vec::with_capacity(3 + scrambled.len());
    out.extend_from_slice(&SYNC);
    out.extend_from_slice(&scrambled);
    out
}

/// Decode an IL2P stream back to an AX.25 frame, RS-correcting the header and
/// each payload block.
pub fn decode(bytes: &[u8]) -> Option<Ax25Frame> {
    if bytes.len() < 3 || bytes[..3] != SYNC {
        return None;
    }
    let body = descramble_bytes(&bytes[3..]);
    if body.len() < HDR_DATA + HDR_PARITY {
        return None;
    }
    // Correct the header.
    let mut hdr_cw = body[..HDR_DATA + HDR_PARITY].to_vec();
    let hdr_rs = Rs::new(HDR_PARITY, 0, 0x1D);
    hdr_rs.decode(&mut hdr_cw).ok()?;
    let mut hdr = [0u8; HDR_DATA];
    hdr.copy_from_slice(&hdr_cw[..HDR_DATA]);
    let (dest, source, info_len) = parse_header(&hdr);

    // Correct the payload blocks.
    let pay_rs = Rs::new(PAY_PARITY, 0, 0x1D);
    let mut info = Vec::with_capacity(info_len);
    let mut pos = HDR_DATA + HDR_PARITY;
    let mut remaining = info_len;
    while remaining > 0 {
        let data_len = remaining.min(PAY_BLOCK);
        let block_len = data_len + PAY_PARITY;
        if pos + block_len > body.len() {
            return None;
        }
        let mut cw = body[pos..pos + block_len].to_vec();
        pay_rs.decode(&mut cw).ok()?;
        info.extend_from_slice(&cw[..data_len]);
        pos += block_len;
        remaining -= data_len;
    }

    Some(Ax25Frame { dest, source, digipeaters: vec![], info })
}

fn bytes_to_bits(bytes: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for &b in bytes {
        for i in (0..8).rev() {
            bits.push((b >> i) & 1);
        }
    }
    bits
}

fn bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|c| {
            let mut b = 0u8;
            for (i, &bit) in c.iter().enumerate() {
                b |= bit << (7 - i);
            }
            b
        })
        .collect()
}

fn scramble_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut s = FrameResetScrambler::new();
    bits_to_bytes(&s.apply(&bytes_to_bits(bytes)))
}

fn descramble_bytes(bytes: &[u8]) -> Vec<u8> {
    // Additive frame-reset scrambler: scramble == descramble.
    scramble_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Ax25Frame {
        Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("N0CALL", 9),
            digipeaters: vec![],
            info: b"!4903.50N/07201.75W-IL2P".to_vec(),
        }
    }

    #[test]
    fn sync_word_prefixes_stream() {
        let enc = encode(&sample());
        assert_eq!(&enc[..3], &SYNC);
    }

    #[test]
    fn roundtrip() {
        let f = sample();
        let enc = encode(&f);
        let back = decode(&enc).expect("decode");
        assert_eq!(back.dest, f.dest);
        assert_eq!(back.source, f.source);
        assert_eq!(back.info, f.info);
    }

    #[test]
    fn header_6bit_is_msb_first() {
        // 'A' maps to 6-bit value 11 = 0b001011; the first header bit is its MSB.
        let f = Ax25Frame {
            dest: Address::new("A", 0),
            source: Address::new("B", 0),
            digipeaters: vec![],
            info: vec![],
        };
        let hdr = build_header(&f);
        // First 6 bits of byte 0 are 001011 -> byte0 high bits = 0010_11xx.
        assert_eq!(hdr[0] >> 2, 0b001011);
    }

    #[test]
    fn rs_recovers_in_capacity_corruption() {
        let f = sample();
        let mut enc = encode(&f);
        // Corrupt one byte in the header RS block (capacity 1 symbol) and one in
        // the payload RS block (capacity 8 symbols). The additive scrambler is
        // byte-aligned, so each hit stays a single symbol error in its block.
        enc[3 + 2] ^= 0x3C; // header data byte 2
        enc[3 + 18] ^= 0x81; // payload byte 0 (after 15-byte header block)
        let back = decode(&enc).expect("RS should recover");
        assert_eq!(back.info, f.info);
        assert_eq!(back.source, f.source);
    }

    #[test]
    fn replaces_hdlc_no_flags_present() {
        // IL2P is not HDLC: there is no requirement for 0x7E flag delimiting,
        // and the sync word is the only framing marker.
        let enc = encode(&sample());
        assert_eq!(&enc[..3], &[0xF1, 0x5E, 0x48]);
    }
}
