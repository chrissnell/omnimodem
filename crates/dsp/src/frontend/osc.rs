//! Phase-accumulator oscillator with a 256-entry f32 cos/sin table, indexed by
//! the top 8 bits of a u32 phase accumulator (Graywolf demod_afsk.rs:40-60).
//!
//! The table is `OnceLock`-initialized because const-eval cannot call `cos`.

use std::f32::consts::TAU;
use std::sync::OnceLock;

fn tables() -> &'static ([f32; 256], [f32; 256]) {
    static T: OnceLock<([f32; 256], [f32; 256])> = OnceLock::new();
    T.get_or_init(|| {
        let mut cos = [0.0f32; 256];
        let mut sin = [0.0f32; 256];
        for i in 0..256 {
            let a = TAU * i as f32 / 256.0;
            cos[i] = a.cos();
            sin[i] = a.sin();
        }
        (cos, sin)
    })
}

/// Phase-accumulator oscillator. One `u32` accumulator wraps to give a perfect
/// modulo-`2π` phase; the top 8 bits index the LUT.
pub struct Oscillator {
    phase: u32,
    /// Phase increment per sample = freq/rate * 2^32.
    step: u32,
}

impl Oscillator {
    pub fn new(freq_hz: f32, rate_hz: f32) -> Self {
        Oscillator { phase: 0, step: Self::step_for(freq_hz, rate_hz) }
    }

    pub fn set_freq(&mut self, freq_hz: f32, rate_hz: f32) {
        self.step = Self::step_for(freq_hz, rate_hz);
    }

    fn step_for(freq_hz: f32, rate_hz: f32) -> u32 {
        ((freq_hz / rate_hz) * (1u64 << 32) as f32) as u32
    }

    /// Advance one sample, returning `(cos, sin)` from the top-8-bit table index.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> (f32, f32) {
        let idx = (self.phase >> 24) as usize;
        self.phase = self.phase.wrapping_add(self.step);
        let (c, s) = tables();
        (c[idx], s[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_cycle_returns_near_origin_phase() {
        // 1 Hz at 256 Hz => exactly 256 samples per cycle, table-exact.
        let mut o = Oscillator::new(1.0, 256.0);
        let (c0, s0) = o.next();
        assert!((c0 - 1.0).abs() < 1e-6 && s0.abs() < 1e-6);
        for _ in 0..255 {
            o.next();
        }
        let (c, s) = o.next();
        assert!((c - 1.0).abs() < 1e-6 && s.abs() < 1e-6);
    }

    #[test]
    fn quarter_cycle_is_sin_one() {
        // 1 Hz at 256 Hz, advance 64 samples => 90 degrees: sin ~ 1, cos ~ 0.
        let mut o = Oscillator::new(1.0, 256.0);
        for _ in 0..64 {
            o.next();
        }
        let (c, s) = o.next();
        assert!(c.abs() < 1e-6 && (s - 1.0).abs() < 1e-6);
    }
}
