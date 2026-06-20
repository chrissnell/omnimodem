//! Three distinct scrambler primitives. They are NOT interchangeable.
//!
//! 1. [`SelfSyncScrambler`] — G3RUH multiplicative LFSR `x¹⁷ + x¹² + 1`. No
//!    seed agreement needed: the descrambler self-synchronises within 17 bits
//!    of any start state because each output bit depends only on a sliding
//!    window of recent *channel* bits.
//! 2. [`FrameResetScrambler`] — IL2P additive (synchronous) LFSR
//!    `x⁹ + x⁴ + 1` with a fixed seed, reset at the start of every frame.
//! 3. [`AdditivePrbs`] — generic additive PRBS / decorrelation whitener
//!    (M17, Olivia) over an arbitrary tap polynomial and seed.
//!
//! All operate on `&[u8]` bit streams of `0`/`1`. Bit order is the order of
//! the slice; callers map bytes↔bits per their wire format.

/// G3RUH self-synchronising multiplicative scrambler, `x¹⁷ + x¹² + 1`.
///
/// Multiplicative: scrambled bit = data ⊕ s[t-12] ⊕ s[t-17] where `s` is the
/// *scrambled* (channel) stream. Descrambling inverts using the received
/// channel bits, so it needs no shared seed and recovers after a 17-bit slip.
pub struct SelfSyncScrambler {
    shift: u32, // last 17 channel bits, bit0 = most recent
}

impl Default for SelfSyncScrambler {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfSyncScrambler {
    const TAP12: u32 = 1 << 11; // x^12 -> 12th previous bit
    const TAP17: u32 = 1 << 16; // x^17
    const MASK: u32 = (1 << 17) - 1;

    pub fn new() -> Self {
        SelfSyncScrambler { shift: 0 }
    }

    fn taps(&self) -> u8 {
        let a = u32::from(self.shift & Self::TAP12 != 0);
        let b = u32::from(self.shift & Self::TAP17 != 0);
        (a ^ b) as u8
    }

    /// Scramble one data bit, returning the channel bit and updating state.
    pub fn scramble_bit(&mut self, data: u8) -> u8 {
        let out = (data & 1) ^ self.taps();
        self.shift = ((self.shift << 1) | out as u32) & Self::MASK;
        out
    }

    /// Descramble one channel bit, returning the recovered data bit.
    pub fn descramble_bit(&mut self, chan: u8) -> u8 {
        let data = (chan & 1) ^ self.taps();
        self.shift = ((self.shift << 1) | (chan & 1) as u32) & Self::MASK;
        data
    }

    pub fn scramble(&mut self, bits: &[u8]) -> Vec<u8> {
        bits.iter().map(|&b| self.scramble_bit(b)).collect()
    }

    pub fn descramble(&mut self, bits: &[u8]) -> Vec<u8> {
        bits.iter().map(|&b| self.descramble_bit(b)).collect()
    }
}

/// IL2P additive frame-reset scrambler, `x⁹ + x⁴ + 1`, fixed seed.
///
/// Additive/synchronous: a keystream LFSR runs independently of the data and
/// is XORed in. Both ends seed identically and reset per frame, so scramble
/// and descramble are the *same* operation.
pub struct FrameResetScrambler {
    state: u16,
}

impl FrameResetScrambler {
    const SEED: u16 = 0x1FF; // 9-bit all-ones (IL2P initial fill)
    const MASK: u16 = (1 << 9) - 1;

    pub fn new() -> Self {
        FrameResetScrambler { state: Self::SEED }
    }

    /// Re-seed for the start of a new frame.
    pub fn reset(&mut self) {
        self.state = Self::SEED;
    }

    /// Next keystream bit (Fibonacci LFSR, taps at x⁹ and x⁴).
    fn next_key(&mut self) -> u8 {
        let b9 = (self.state >> 8) & 1;
        let b4 = (self.state >> 3) & 1;
        let fb = b9 ^ b4;
        let out = b9 as u8;
        self.state = ((self.state << 1) | fb) & Self::MASK;
        out
    }

    pub fn apply(&mut self, bits: &[u8]) -> Vec<u8> {
        bits.iter().map(|&b| (b & 1) ^ self.next_key()).collect()
    }
}

impl Default for FrameResetScrambler {
    fn default() -> Self {
        Self::new()
    }
}

/// Generic additive PRBS whitener over an arbitrary Fibonacci LFSR.
///
/// `poly` is the feedback tap mask (bit `i` set => tap on stage `i+1`),
/// `seed` the initial fill, `width` the register length in bits. Additive:
/// scramble == descramble given identical (poly, seed, width).
pub struct AdditivePrbs {
    poly: u32,
    state: u32,
    width: u8,
    seed: u32,
}

impl AdditivePrbs {
    pub fn new(poly: u32, seed: u32, width: u8) -> Self {
        let mask = (1u32 << width) - 1;
        AdditivePrbs { poly: poly & mask, state: (seed & mask).max(1), width, seed: seed & mask }
    }

    /// M17 / Olivia style: `x⁹ + x⁵ + 1`, all-ones seed, 9-bit register.
    pub fn m17() -> Self {
        Self::new((1 << 8) | (1 << 4), (1 << 9) - 1, 9)
    }

    pub fn reset(&mut self) {
        self.state = self.seed.max(1);
    }

    fn next_key(&mut self) -> u8 {
        let out = ((self.state >> (self.width - 1)) & 1) as u8;
        let fb = (self.state & self.poly).count_ones() & 1;
        let mask = (1u32 << self.width) - 1;
        self.state = ((self.state << 1) | fb) & mask;
        out
    }

    pub fn apply(&mut self, bits: &[u8]) -> Vec<u8> {
        bits.iter().map(|&b| (b & 1) ^ self.next_key()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bits() -> Vec<u8> {
        // A deterministic, structured-then-random-ish bit pattern.
        let mut v = Vec::new();
        let mut x: u32 = 0xACE1;
        for _ in 0..400 {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            v.push(((x >> 16) & 1) as u8);
        }
        v
    }

    #[test]
    fn g3ruh_roundtrip() {
        let bits = sample_bits();
        let chan = SelfSyncScrambler::new().scramble(&bits);
        let back = SelfSyncScrambler::new().descramble(&chan);
        assert_eq!(back, bits);
    }

    #[test]
    fn g3ruh_self_synchronizes_after_17_bits() {
        let bits = sample_bits();
        let mut chan = SelfSyncScrambler::new().scramble(&bits);
        // Corrupt the first 17 channel bits (flip each).
        for c in chan.iter_mut().take(17) {
            *c ^= 1;
        }
        let recovered = SelfSyncScrambler::new().descramble(&chan);
        // Errors can propagate at most 17 bits past each corrupted bit
        // (window 17), so everything from index 34 onward must be clean.
        assert_eq!(&recovered[34..], &bits[34..]);
    }

    #[test]
    fn g3ruh_descrambler_self_syncs_from_arbitrary_state() {
        // Two descramblers fed the same channel from different states must
        // agree after at most 17 bits.
        let bits = sample_bits();
        let chan = SelfSyncScrambler::new().scramble(&bits);
        let clean = SelfSyncScrambler::new().descramble(&chan);
        let mut dirty = SelfSyncScrambler { shift: 0x1_5555 };
        let out = dirty.descramble(&chan);
        assert_eq!(&out[17..], &clean[17..]);
    }

    #[test]
    fn il2p_frame_reset_roundtrip_and_repeatable() {
        let bits = sample_bits();
        let mut enc = FrameResetScrambler::new();
        let scrambled = enc.apply(&bits);
        let mut dec = FrameResetScrambler::new();
        assert_eq!(dec.apply(&scrambled), bits);

        // Resetting reproduces the identical keystream (fixed seed per frame).
        let mut a = FrameResetScrambler::new();
        let k1 = a.apply(&[0u8; 32]);
        a.reset();
        let k2 = a.apply(&[0u8; 32]);
        assert_eq!(k1, k2);
        // Keystream is non-trivial (not all zero).
        assert!(k1.contains(&1));
    }

    #[test]
    fn additive_prbs_roundtrip() {
        let bits = sample_bits();
        let mut enc = AdditivePrbs::m17();
        let scrambled = enc.apply(&bits);
        let mut dec = AdditivePrbs::m17();
        assert_eq!(dec.apply(&scrambled), bits);
        assert_ne!(scrambled, bits); // actually whitened
    }
}
