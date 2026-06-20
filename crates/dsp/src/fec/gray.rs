//! Gray coding and differential BPSK.
//!
//! Gray code: adjacent integers differ by exactly one bit, which minimises
//! symbol-error → bit-error inflation in M-ary constellations.
//!
//! Differential BPSK: data is carried in the *change* of phase between
//! consecutive symbols, so an absolute phase ambiguity (180°) does not break
//! decoding. We model phase as a `0`/`1` polarity stream.

/// Binary-reflected Gray encode of an integer.
pub fn gray_encode(n: u32) -> u32 {
    n ^ (n >> 1)
}

/// Inverse of [`gray_encode`].
pub fn gray_decode(mut g: u32) -> u32 {
    let mut n = 0;
    while g != 0 {
        n ^= g;
        g >>= 1;
    }
    n
}

/// Differential BPSK encode: output symbol `i` is the running XOR of all data
/// bits up to and including `i`, starting from an implicit reference symbol 0.
/// (A data `1` flips polarity; a `0` holds.)
pub fn diff_bpsk_encode(bits: &[u8]) -> Vec<u8> {
    let mut sym = 0u8;
    let mut out = Vec::with_capacity(bits.len());
    for &b in bits {
        sym ^= b & 1;
        out.push(sym);
    }
    out
}

/// Differential BPSK decode: recover data as the XOR difference between
/// adjacent symbols (reference symbol before the first is `0`).
pub fn diff_bpsk_decode(syms: &[u8]) -> Vec<u8> {
    let mut prev = 0u8;
    let mut out = Vec::with_capacity(syms.len());
    for &s in syms {
        out.push((s ^ prev) & 1);
        prev = s;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_roundtrips_0_to_256() {
        for n in 0..256u32 {
            assert_eq!(gray_decode(gray_encode(n)), n);
        }
    }

    #[test]
    fn gray_adjacent_single_bit_change() {
        for n in 0..255u32 {
            let diff = gray_encode(n) ^ gray_encode(n + 1);
            assert_eq!(diff.count_ones(), 1, "n={n}");
        }
    }

    #[test]
    fn gray_known_values() {
        assert_eq!(gray_encode(0), 0);
        assert_eq!(gray_encode(1), 1);
        assert_eq!(gray_encode(2), 3);
        assert_eq!(gray_encode(3), 2);
        assert_eq!(gray_encode(4), 6);
    }

    #[test]
    fn diff_bpsk_roundtrips() {
        let bits = [1, 0, 1, 1, 0, 0, 1, 0, 1];
        assert_eq!(diff_bpsk_decode(&diff_bpsk_encode(&bits)), bits);
    }

    #[test]
    fn diff_bpsk_phase_ambiguity_immune() {
        // Inverting every symbol (180° ambiguity) must not change the data.
        let bits = [1, 1, 0, 1, 0, 0, 1];
        let syms = diff_bpsk_encode(&bits);
        let inverted: Vec<u8> = syms.iter().map(|s| s ^ 1).collect();
        // Decoder seeds from reference 0; inverted stream decodes the first
        // bit flipped but all *differences* are preserved.
        let d = diff_bpsk_decode(&inverted);
        assert_eq!(&d[1..], &bits[1..]);
    }
}
