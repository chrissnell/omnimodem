//! Shared 33-tone IFK (incremental frequency keying) core for IFKP and FSQ.
//!
//! Both modes advance the transmitted tone by `sym + OFFSET` each symbol
//! (`tone = (prev + sym + OFFSET) % 33`) and recover the symbol on receive by
//! differencing successive tones. This is the same idea as the DominoEX/THOR
//! IFK+ core (Phase 9), retargeted to 33 tones with `OFFSET = 1`. ref:
//! ifkp.cxx:706-710, fsq.cxx:1352-1356.
//!
//! The symbol→tone-index mapping is **bit-exact** vs the fldigi golden vectors
//! (asserted in `modes::ifkp` / `modes::fsq`); the tone→audio path (via
//! `frontend::modulate::MFsk`) is FP and gated on a loopback/AWGN decode only.

use crate::modes::fsk_util::{argmax, tone_powers};
use crate::types::Sample;

/// The number of IFK tones both modes use.
pub const NUMTONES: u32 = 33;

/// Advance one IFK tone: `tone = (prev + sym + offset) % NUMTONES`. ref:
/// ifkp.cxx:708, fsq.cxx:1355.
#[inline]
pub fn advance(prev: u32, sym: u8, offset: u32) -> u32 {
    (prev + sym as u32 + offset) % NUMTONES
}

/// Invert the IFK advance: `sym = (tone - prev - offset) mod NUMTONES`.
#[inline]
pub fn invert(prev: u32, tone: u32, offset: u32) -> u8 {
    ((tone + 2 * NUMTONES - prev - offset) % NUMTONES) as u8
}

/// The absolute IFK tone-index sequence for a symbol stream, seeded `prev = 0`
/// (the `prevtone = 0` both modems start from). ref: ifkp.cxx:706-710.
pub fn syms_to_tones(syms: &[u8], offset: u32) -> Vec<u32> {
    let mut prev = 0u32;
    syms.iter()
        .map(|&s| {
            let t = advance(prev, s, offset);
            prev = t;
            t
        })
        .collect()
}

/// Per-symbol IFK geometry shared by a mode's modulator and demodulator.
#[derive(Debug, Clone, Copy)]
pub struct IfkGeom {
    pub rate: f32,
    /// Emitted samples per symbol.
    pub symlen: usize,
    /// Center (carrier) frequency in Hz.
    pub center_hz: f32,
    /// Tone spacing in Hz.
    pub spacing_hz: f32,
}

impl IfkGeom {
    /// Lowest tone frequency: tone `k` sits at `center + (k - (NUMTONES-1)/2) *
    /// spacing`, so tone 0 is `center - (NUMTONES-1)/2 * spacing`.
    pub fn base_hz(&self) -> f32 {
        self.center_hz - 0.5 * (NUMTONES as f32 - 1.0) * self.spacing_hz
    }

    /// Occupied bandwidth: `NUMTONES * spacing`. ref: ifkp.cxx:270, fsq.cxx:252.
    pub fn bandwidth(&self) -> f32 {
        NUMTONES as f32 * self.spacing_hz
    }
}

/// A streaming IFK symbol demodulator: buffers audio, argmax-detects the tone of
/// each fully-buffered `symlen` block, and differences it against the previous
/// tone to recover the symbol (0..32). The framing/varicode step is the caller's
/// (each mode owns its `Framer`). Seeded `prev_tone = 0`.
pub struct IfkDemod {
    geom: IfkGeom,
    offset: u32,
    buf: Vec<Sample>,
    prev_tone: u32,
}

impl IfkDemod {
    pub fn new(geom: IfkGeom, offset: u32) -> Self {
        IfkDemod { geom, offset, buf: Vec::new(), prev_tone: 0 }
    }

    pub fn push_samples(&mut self, samples: &[Sample]) {
        self.buf.extend_from_slice(samples);
    }

    /// Drain every fully-buffered symbol, returning the recovered symbol stream.
    pub fn drain(&mut self) -> Vec<u8> {
        let sps = self.geom.symlen;
        let base = self.geom.base_hz();
        let mut out = Vec::new();
        let mut consumed = 0;
        while self.buf.len() - consumed >= sps {
            let block = &self.buf[consumed..consumed + sps];
            let powers = tone_powers(block, self.geom.rate, base, self.geom.spacing_hz, NUMTONES);
            let tone = argmax(&powers) as u32;
            out.push(invert(self.prev_tone, tone, self.offset));
            self.prev_tone = tone;
            consumed += sps;
        }
        self.buf.drain(..consumed);
        out
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.prev_tone = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_and_invert_are_inverse() {
        for offset in [1u32, 2] {
            let mut prev = 0u32;
            for sym in [0u8, 5, 28, 30, 31, 32, 1, 17] {
                let t = advance(prev, sym, offset);
                assert!(t < NUMTONES);
                assert_eq!(invert(prev, t, offset), sym % NUMTONES as u8);
                prev = t;
            }
        }
    }

    #[test]
    fn syms_to_tones_seeds_at_zero() {
        // ref: first IFKP message tone from the golden vector: sym 3 → tone 4
        // (0 + 3 + 1) with offset 1.
        assert_eq!(syms_to_tones(&[3], 1)[0], 4);
        assert_eq!(syms_to_tones(&[3, 29], 1), vec![4, 1]); // (4+29+1)%33 = 1
    }
}
