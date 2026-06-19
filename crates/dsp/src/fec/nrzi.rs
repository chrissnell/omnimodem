//! AX.25 NRZI (non-return-to-zero, inverted).
//!
//! Convention (AX.25 / HDLC): a logical **0** bit causes the line to
//! **transition**; a logical **1** holds the previous level. Inputs and
//! outputs are slices of `0`/`1` bytes. The encoder starts from an assumed
//! prior level of `0`; the decoder reconstructs by comparing adjacent levels.

/// Encode logical bits to NRZI line levels. Output level for each bit is the
/// running level after applying that bit's transition rule.
pub fn nrzi_encode(bits: &[u8]) -> Vec<u8> {
    let mut level = 0u8;
    let mut out = Vec::with_capacity(bits.len());
    for &b in bits {
        if b == 0 {
            level ^= 1; // 0 toggles
        }
        // 1 holds
        out.push(level);
    }
    out
}

/// Decode NRZI line levels back to logical bits. A held level => `1`, a
/// transition => `0`. The implied prior level before the first symbol is `0`.
pub fn nrzi_decode(levels: &[u8]) -> Vec<u8> {
    let mut prev = 0u8;
    let mut out = Vec::with_capacity(levels.len());
    for &lvl in levels {
        out.push(u8::from(lvl == prev)); // no transition => 1
        prev = lvl;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_kat_transition_rule() {
        // Starting level 0. bits: 0 1 1 0 1
        //  0 -> toggle  -> 1
        //  1 -> hold    -> 1
        //  1 -> hold    -> 1
        //  0 -> toggle  -> 0
        //  1 -> hold    -> 0
        assert_eq!(nrzi_encode(&[0, 1, 1, 0, 1]), vec![1, 1, 1, 0, 0]);
    }

    #[test]
    fn roundtrip_hand() {
        let bits = [0, 1, 1, 0, 1, 0, 0, 1, 1, 1, 0];
        assert_eq!(nrzi_decode(&nrzi_encode(&bits)), bits);
    }

    #[test]
    fn all_ones_holds_level() {
        // No zeros => no transitions => level stays 0 throughout.
        assert_eq!(nrzi_encode(&[1, 1, 1, 1]), vec![0, 0, 0, 0]);
        assert_eq!(nrzi_decode(&[0, 0, 0, 0]), vec![1, 1, 1, 1]);
    }
}
