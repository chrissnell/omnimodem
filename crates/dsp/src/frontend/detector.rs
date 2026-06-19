//! FM discriminator (phase-difference detector) and an envelope detector with
//! adaptive attack/decay squelch. The discriminator drives FSK/RTTY/WEFAX; the
//! envelope detector drives OOK modes (CW/Hell).

use crate::types::{Cplx, Sample};

/// Phase-difference FM discriminator: `atan2` of `z[n]·conj(z[n-1])` gives the
/// per-sample phase advance, proportional to instantaneous frequency.
pub struct FmDiscriminator {
    prev: Cplx,
}

impl Default for FmDiscriminator {
    fn default() -> Self {
        Self::new()
    }
}

impl FmDiscriminator {
    pub fn new() -> Self {
        FmDiscriminator { prev: Cplx::new(0.0, 0.0) }
    }

    /// Returns the instantaneous frequency in radians/sample. Multiply by
    /// `rate/(2π)` for Hz.
    pub fn push(&mut self, z: Cplx) -> f32 {
        let d = z * self.prev.conj();
        self.prev = z;
        d.im.atan2(d.re)
    }
}

/// Magnitude envelope follower with independent attack/decay smoothing, plus an
/// adaptive noise floor used as a squelch gate.
pub struct EnvelopeDetector {
    env: f32,
    floor: f32,
    attack: f32,
    decay: f32,
    floor_coeff: f32,
    open_ratio: f32,
    warmup: u32,
}

impl EnvelopeDetector {
    /// `attack`/`decay` smooth the envelope; `floor_coeff` slews the adaptive
    /// noise floor; the gate opens when `env > floor * open_ratio`.
    pub fn new(attack: f32, decay: f32, floor_coeff: f32, open_ratio: f32) -> Self {
        EnvelopeDetector { env: 0.0, floor: 0.0, attack, decay, floor_coeff, open_ratio, warmup: 128 }
    }

    /// Feed one real sample; returns the smoothed envelope.
    pub fn push(&mut self, x: Sample) -> f32 {
        let mag = x.abs();
        let coeff = if mag > self.env { self.attack } else { self.decay };
        self.env += (mag - self.env) * coeff;
        // The floor estimates the quiescent noise envelope. During a short
        // warmup it locks straight onto the envelope so the gate starts from a
        // valid noise reference; thereafter it only adapts while the gate is
        // *closed*, so it converges on noise and never chases a strong signal
        // once the squelch opens (classic adaptive squelch with hysteresis).
        if self.warmup > 0 {
            self.floor += (self.env - self.floor) * 0.1;
            self.warmup -= 1;
        } else if !self.squelch_open() {
            self.floor += (self.env - self.floor) * self.floor_coeff;
        }
        self.env
    }

    /// True when the current envelope clears the adaptive squelch.
    pub fn squelch_open(&self) -> bool {
        self.env > self.floor * self.open_ratio
    }

    pub fn envelope(&self) -> f32 {
        self.env
    }

    pub fn floor(&self) -> f32 {
        self.floor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn discriminator_output_proportional_to_frequency() {
        let rate = 8000.0;
        let mut disc = FmDiscriminator::new();
        // Complex tone at f0: phase advance per sample = 2π f0/rate.
        let f0 = 500.0;
        let mut acc = 0.0;
        let n = 4000;
        for k in 0..n {
            let ph = TAU * f0 * k as f32 / rate;
            acc += disc.push(Cplx::new(ph.cos(), ph.sin()));
        }
        let avg = acc / n as f32;
        let expected = TAU * f0 / rate;
        assert!((avg - expected).abs() < 1e-2, "got {avg}, want {expected}");

        // Doubling the frequency doubles the discriminator output.
        let mut disc2 = FmDiscriminator::new();
        let mut acc2 = 0.0;
        for k in 0..n {
            let ph = TAU * (2.0 * f0) * k as f32 / rate;
            acc2 += disc2.push(Cplx::new(ph.cos(), ph.sin()));
        }
        let avg2 = acc2 / n as f32;
        assert!((avg2 / avg - 2.0).abs() < 0.05);
    }

    #[test]
    fn envelope_detector_recovers_ook_and_squelches_noise() {
        let rate = 8000.0;
        let f = 800.0;
        // Floor tracks noise quickly while gated closed; gate opens at 3x floor.
        let mut det = EnvelopeDetector::new(0.05, 0.01, 0.02, 3.0);
        let mut open_during_off = false;
        let mut open_during_on = false;
        for k in 0..2000 {
            let phase = TAU * f * k as f32 / rate;
            // key-on for the middle 800 samples.
            let keyed = (600..1400).contains(&k);
            let x = if keyed { (phase).sin() } else { 0.001 * (phase * 1.3).sin() };
            det.push(x);
            if (900..1300).contains(&k) {
                open_during_on |= det.squelch_open();
            }
            // Skip the first samples while the floor seeds onto the noise level.
            if (200..500).contains(&k) {
                open_during_off |= det.squelch_open();
            }
        }
        assert!(open_during_on, "squelch should open on key-down");
        assert!(!open_during_off, "squelch should stay closed on noise");
    }
}
