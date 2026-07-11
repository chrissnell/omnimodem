//! Mode S / ADS-B 24-bit CRC.
//!
//! Every Mode S downlink frame carries a 24-bit parity field in its trailing
//! three bytes. The parity is the remainder of the message polynomial divided
//! by the Mode S generator polynomial `0xFFF409` (ICAO Annex 10, Vol IV). For
//! extended squitter (DF17 / DF18) the parity has no interrogator overlay, so a
//! correctly received frame checksums to zero over all of its bytes.
//!
//! The table-driven byte-wise routine here matches dump1090's `modescrc`.

/// Mode S CRC generator polynomial (implicit `x^24` term omitted).
pub const GENERATOR_POLY: u32 = 0x00FF_F409;

/// Precomputed byte lookup table for the Mode S CRC.
static CRC_TABLE: [u32; 256] = build_table();

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 16;
        let mut j = 0;
        while j < 8 {
            if c & 0x0080_0000 != 0 {
                c = (c << 1) ^ GENERATOR_POLY;
            } else {
                c <<= 1;
            }
            j += 1;
        }
        table[i] = c & 0x00FF_FFFF;
        i += 1;
    }
    table
}

/// Compute the 24-bit Mode S checksum over `msg`.
///
/// A valid extended-squitter frame (with parity in the last three bytes)
/// returns `0`. The routine reduces `msg(x) * x^24 mod g(x)`, so the checksum
/// of just the data bytes (parity omitted) is exactly the parity to append.
pub fn checksum(msg: &[u8]) -> u32 {
    let mut rem: u32 = 0;
    for &b in msg {
        let idx = (b ^ ((rem >> 16) as u8)) as usize;
        rem = ((rem << 8) ^ CRC_TABLE[idx]) & 0x00FF_FFFF;
    }
    rem
}

/// Write the correct parity into the trailing three bytes of `frame`.
///
/// The parity is `data(x) * x^24 mod g(x)` — the checksum of the data bytes
/// alone — which makes the completed frame checksum to zero. `frame` must be
/// 7 or 14 bytes.
pub fn append_parity(frame: &mut [u8]) {
    let n = frame.len();
    debug_assert!(n >= 3, "frame too short for parity");
    let p = checksum(&frame[..n - 3]);
    frame[n - 3] = (p >> 16) as u8;
    frame[n - 2] = (p >> 8) as u8;
    frame[n - 1] = p as u8;
}

/// Locate a single-bit error in an `nbits`-bit frame from its CRC residual.
///
/// The Mode S CRC is linear over GF(2): flipping bit `p` of a frame XORs a fixed
/// *syndrome* `checksum(e_p)` into the residual, where `e_p` is the frame that is
/// all zeros but for bit `p`. A correctly received extended-squitter frame
/// checksums to zero, so a frame corrupted in exactly one bit has a residual
/// equal to that bit's syndrome. Invert that map: given a nonzero `residual`,
/// return the bit position whose syndrome matches, or `None` when no single flip
/// explains it (a clean frame, or two-or-more-bit damage — never guessed at).
///
/// This is the syndrome-locate discipline dump1090's default `--fix` uses: repair
/// only what a *unique* single-bit flip accounts for. Two-bit search (which
/// dump1090's `--fix-2bit` does and its default abandons) is deliberately not
/// implemented — it fabricates frames, the false positives R5 is built to avoid.
pub fn locate_single_bit_error(residual: u32, nbits: usize) -> Option<usize> {
    if residual == 0 {
        return None;
    }
    // Bit p from the frame start (0 = MSB of byte 0) has syndrome `checksum(e_p)`.
    // e_p is the message x^(nbits-1-p), so its checksum is the CRC of x^(nbits-1-p)
    // — computed directly rather than tabled, since repair is a cold path (only
    // parity-failing candidates) and nbits <= 112 keeps the scan trivially cheap.
    (0..nbits).find(|&p| single_bit_syndrome(p, nbits) == residual)
}

/// CRC syndrome of the `nbits`-bit frame that is all zeros except bit `p`.
fn single_bit_syndrome(p: usize, nbits: usize) -> u32 {
    let mut buf = vec![0u8; nbits / 8];
    buf[p / 8] |= 1 << (7 - (p % 8));
    checksum(&buf)
}

/// Attempt to correct a single-bit error in `frame` (7 or 14 bytes) in place.
///
/// Returns `true` and leaves `frame` checksumming to zero when exactly one bit
/// flip repairs it; returns `false` and leaves `frame` untouched otherwise (a
/// clean frame, or damage no single flip explains). See
/// [`locate_single_bit_error`] for why only single-bit repair is attempted.
pub fn try_repair_single_bit(frame: &mut [u8]) -> bool {
    let nbits = frame.len() * 8;
    match locate_single_bit_error(checksum(frame), nbits) {
        Some(p) => {
            frame[p / 8] ^= 1 << (7 - (p % 8));
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean 14-byte frame (DF17, ICAO 4840D6, "KLM1023"), checksum 0.
    fn clean_frame() -> Vec<u8> {
        vec![
            0x8D, 0x48, 0x40, 0xD6, 0x20, 0x2C, 0xC3, 0x71, 0xC3, 0x2C, 0xE0, 0x57, 0x60, 0x98,
        ]
    }

    #[test]
    fn repairs_every_single_bit_flip() {
        let clean = clean_frame();
        let nbits = clean.len() * 8;
        for p in 0..nbits {
            let mut f = clean.clone();
            f[p / 8] ^= 1 << (7 - (p % 8));
            assert_ne!(checksum(&f), 0, "bit {p} flip should break parity");
            assert!(try_repair_single_bit(&mut f), "bit {p} should be repairable");
            assert_eq!(checksum(&f), 0, "bit {p} repaired frame must checksum clean");
            assert_eq!(f, clean, "bit {p} repair must restore the original frame");
        }
    }

    #[test]
    fn locates_matching_bit_position() {
        let clean = clean_frame();
        let nbits = clean.len() * 8;
        for p in [0usize, 7, 40, 111] {
            let mut f = clean.clone();
            f[p / 8] ^= 1 << (7 - (p % 8));
            assert_eq!(locate_single_bit_error(checksum(&f), nbits), Some(p));
        }
    }

    #[test]
    fn declines_clean_and_double_bit_frames() {
        let clean = clean_frame();
        // A clean frame has nothing to repair.
        let mut f = clean.clone();
        assert!(!try_repair_single_bit(&mut f));
        assert_eq!(f, clean);
        // A two-bit error has no single-flip explanation, so repair declines
        // rather than guessing (the false-positive discipline R5 preserves).
        let mut two = clean.clone();
        two[0] ^= 0x01;
        two[9] ^= 0x40;
        assert_ne!(checksum(&two), 0);
        let before = two.clone();
        assert!(!try_repair_single_bit(&mut two));
        assert_eq!(two, before, "declined repair must not mutate the frame");
    }

    #[test]
    fn repairs_short_frames_too() {
        let mut f = [0u8; 7];
        f[0] = (11 << 3) | 0x05;
        f[1] = 0x3C;
        f[2] = 0x64;
        f[3] = 0x44;
        append_parity(&mut f);
        let clean = f;
        f[4] ^= 0x08;
        assert!(try_repair_single_bit(&mut f));
        assert_eq!(f, clean);
    }
}
