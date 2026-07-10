//! Tunable NCO + complex down-converter: multiply a real input by e^{-j2πf t}
//! to shift `tune_hz` to DC for passband isolation / click-to-tune.

use crate::frontend::osc::Oscillator;
use crate::types::Cplx;

pub struct DownConverter {
    osc: Oscillator,
    rate: f32,
}

impl DownConverter {
    pub fn new(tune_hz: f32, rate_hz: f32) -> Self {
        DownConverter { osc: Oscillator::new(tune_hz, rate_hz), rate: rate_hz }
    }

    pub fn retune(&mut self, tune_hz: f32) {
        self.osc.set_freq(tune_hz, self.rate);
    }

    /// Real sample -> complex baseband (x * (cos - j sin)).
    pub fn push(&mut self, x: f32) -> Cplx {
        let (c, s) = self.osc.next();
        Cplx::new(x * c, -x * s)
    }

    /// Complex sample -> complex baseband. Multiplies by e^{-j2πf t} (the same
    /// NCO `push` uses), shifting `tune_hz` to DC for IQ channel selection.
    /// `z * (cos - j sin)` expanded to avoid a temporary Cplx.
    pub fn push_cplx(&mut self, z: Cplx) -> Cplx {
        let (c, s) = self.osc.next();
        Cplx::new(z.re * c + z.im * s, z.im * c - z.re * s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn tone_at_tune_freq_becomes_dc() {
        let rate = 8000.0;
        let f0 = 1500.0;
        let mut dc = DownConverter::new(f0, rate);
        // Average of the down-converted tone has large magnitude (energy at DC).
        let mut acc = Cplx::new(0.0, 0.0);
        for n in 0..8000 {
            let x = (TAU * f0 * n as f32 / rate).cos();
            acc += dc.push(x);
        }
        let mag = (acc / 8000.0).norm();
        assert!(mag > 0.4, "DC energy {mag} should be ~0.5 for a unit tone");
    }

    #[test]
    fn offset_tone_does_not_land_at_dc() {
        let rate = 8000.0;
        let mut dc = DownConverter::new(1500.0, rate);
        let mut acc = Cplx::new(0.0, 0.0);
        for n in 0..8000 {
            // Tone 300 Hz away from the tune frequency averages to ~0 at DC.
            let x = (TAU * 1800.0 * n as f32 / rate).cos();
            acc += dc.push(x);
        }
        assert!((acc / 8000.0).norm() < 0.05);
    }

    #[test]
    fn complex_tone_at_tune_freq_becomes_dc() {
        use crate::types::Cplx;
        let rate = 240_000.0;
        let f0 = 30_000.0; // signal sits +30 kHz above center
        let mut dc = DownConverter::new(f0, rate);
        let mut acc = Cplx::new(0.0, 0.0);
        let n = 24_000;
        for k in 0..n {
            let ph = std::f32::consts::TAU * f0 * k as f32 / rate;
            let x = Cplx::new(ph.cos(), ph.sin()); // e^{+j2π f0 k/rate}
            acc += dc.push_cplx(x);
        }
        let mag = (acc / n as f32).norm();
        assert!(mag > 0.9, "tuned complex tone should sit at DC, got {mag}");
    }

    #[test]
    fn complex_offset_tone_averages_away() {
        use crate::types::Cplx;
        let rate = 240_000.0;
        let mut dc = DownConverter::new(30_000.0, rate);
        let mut acc = Cplx::new(0.0, 0.0);
        let n = 24_000;
        for k in 0..n {
            let ph = std::f32::consts::TAU * 90_000.0 * k as f32 / rate; // 60 kHz off tune
            let x = Cplx::new(ph.cos(), ph.sin());
            acc += dc.push_cplx(x);
        }
        assert!((acc / n as f32).norm() < 0.05);
    }
}
