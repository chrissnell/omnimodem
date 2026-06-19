//! Known-sequence / sync-word / preamble correlator with a Hamming-distance
//! fuzzy threshold.
//!
//! Many framings begin with a fixed bit pattern: FX.25 prefixes a 64-bit
//! correlation tag (CTAG), M17 uses 16-bit sync words, IL2P uses the 24-bit
//! `0xF15E48`. A receiver slides the reference over the incoming bit stream and
//! declares a match where the Hamming distance falls within a tolerance, so a
//! few bit errors in the preamble do not lose the frame. [`SyncWord`] holds a
//! reference of up to 64 bits; [`SyncCorrelator`] streams bits and fires on the
//! first within-threshold alignment.

/// Canonical FX.25 correlation tag (Tag_01, 64-bit, MSB-first on the wire).
pub const FX25_CTAG_01: u64 = 0xB74D_B7DF_8A53_2F3E;

/// M17 stream-mode link-setup sync word (16-bit).
pub const M17_LSF_SYNC: u64 = 0x55F7;

/// IL2P sync word (24-bit).
pub const IL2P_SYNC: u64 = 0x00F1_5E48;

/// A fixed bit pattern of `len` bits (LSB at bit 0 of `bits`, but compared
/// MSB-first across `len`), with a fuzzy match tolerance.
#[derive(Clone, Copy)]
pub struct SyncWord {
    bits: u64,
    len: u32,
    max_dist: u32,
}

impl SyncWord {
    /// `pattern` holds `len` significant bits in its low `len` bits.
    pub fn new(pattern: u64, len: u32, max_dist: u32) -> Self {
        assert!((1..=64).contains(&len));
        let mask = if len == 64 { u64::MAX } else { (1u64 << len) - 1 };
        SyncWord { bits: pattern & mask, len, max_dist }
    }

    pub fn fx25() -> Self {
        SyncWord::new(FX25_CTAG_01, 64, 0)
    }

    pub fn m17() -> Self {
        SyncWord::new(M17_LSF_SYNC, 16, 0)
    }

    pub fn il2p() -> Self {
        SyncWord::new(IL2P_SYNC, 24, 0)
    }

    /// Same pattern with a different fuzzy tolerance.
    pub fn with_tolerance(self, max_dist: u32) -> Self {
        SyncWord { max_dist, ..self }
    }

    /// Number of significant bits in the reference (always >= 1).
    pub fn bit_len(&self) -> u32 {
        self.len
    }

    /// Hamming distance between the reference and a candidate window holding
    /// `len` bits in its low bits.
    pub fn distance(&self, window: u64) -> u32 {
        let mask = if self.len == 64 { u64::MAX } else { (1u64 << self.len) - 1 };
        (self.bits ^ (window & mask)).count_ones()
    }

    /// True if `window` is within the fuzzy threshold.
    pub fn matches(&self, window: u64) -> bool {
        self.distance(window) <= self.max_dist
    }
}

/// Streaming correlator: push bits MSB-first; reports the distance whenever the
/// trailing `len` bits are within threshold.
pub struct SyncCorrelator {
    word: SyncWord,
    shift: u64,
    filled: u32,
}

impl SyncCorrelator {
    pub fn new(word: SyncWord) -> Self {
        SyncCorrelator { word, shift: 0, filled: 0 }
    }

    /// Push one bit (newest into the LSB). Returns `Some(distance)` when the
    /// window is full and matches within tolerance.
    pub fn push(&mut self, bit: bool) -> Option<u32> {
        self.shift = (self.shift << 1) | u64::from(bit);
        if self.filled < self.word.len {
            self.filled += 1;
        }
        if self.filled < self.word.len {
            return None;
        }
        let d = self.word.distance(self.shift);
        (d <= self.word.max_dist).then_some(d)
    }

    /// Feed a slice of bits; return the bit index (0-based, position of the
    /// last bit of the window) of the first within-threshold match.
    pub fn scan(&mut self, bits: &[bool]) -> Option<usize> {
        for (i, &b) in bits.iter().enumerate() {
            if self.push(b).is_some() {
                return Some(i);
            }
        }
        None
    }
}

/// Expand a `len`-bit `pattern` (MSB-first) to a `Vec<bool>`.
pub fn pattern_bits(pattern: u64, len: u32) -> Vec<bool> {
    (0..len).rev().map(|i| (pattern >> i) & 1 == 1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_scores_zero() {
        let w = SyncWord::fx25();
        assert_eq!(w.distance(FX25_CTAG_01), 0);
        assert!(w.matches(FX25_CTAG_01));
    }

    #[test]
    fn corrupted_ctag_matches_under_threshold_random_does_not() {
        let w = SyncWord::fx25().with_tolerance(3);
        // Flip 2 bits.
        let corrupted = FX25_CTAG_01 ^ (1 << 5) ^ (1 << 40);
        assert_eq!(w.distance(corrupted), 2);
        assert!(w.matches(corrupted));
        // A random unrelated word is far away and must not match.
        let random = 0x0123_4567_89AB_CDEFu64;
        assert!(w.distance(random) > 3);
        assert!(!w.matches(random));
    }

    #[test]
    fn streaming_correlator_finds_aligned_ctag() {
        let w = SyncWord::fx25().with_tolerance(3);
        let mut corr = SyncCorrelator::new(w);
        // Preamble noise, then the (slightly corrupted) tag, then payload.
        let mut bits = Vec::new();
        bits.extend(pattern_bits(0xAAAA_AAAA, 32)); // junk
        let corrupted = FX25_CTAG_01 ^ (1 << 12); // 1 bit error
        let tag_start = bits.len();
        bits.extend(pattern_bits(corrupted, 64));
        bits.extend(pattern_bits(0xDEAD_BEEF, 32));
        let hit = corr.scan(&bits).expect("tag should be found");
        // Match fires when the last tag bit enters the window.
        assert_eq!(hit, tag_start + 63);
    }

    #[test]
    fn m17_and_il2p_sync_words() {
        let m = SyncWord::m17();
        assert_eq!(m.bit_len(), 16);
        assert_eq!(m.distance(M17_LSF_SYNC), 0);
        assert!(m.matches(M17_LSF_SYNC));
        assert!(!m.matches(M17_LSF_SYNC ^ 1));

        let il = SyncWord::il2p();
        assert_eq!(il.bit_len(), 24);
        assert_eq!(il.distance(IL2P_SYNC), 0);
        // 24-bit IL2P value is exactly 0xF15E48.
        assert_eq!(IL2P_SYNC, 0xF1_5E48);
    }

    #[test]
    fn fuzzy_threshold_rejects_at_distance_boundary() {
        let w = SyncWord::new(0xFFFF, 16, 2);
        // distance 2 matches, distance 3 does not.
        assert!(w.matches(0xFFFF ^ 0b11)); // 2 bits
        assert!(!w.matches(0xFFFF ^ 0b111)); // 3 bits
    }
}
