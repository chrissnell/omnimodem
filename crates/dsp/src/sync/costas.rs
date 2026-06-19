//! Costas-loop carrier recovery and AFC.
//!
//! [`CostasLoop`] is a second-order BPSK Costas loop: it mixes the input down
//! by its NCO, forms the BPSK phase-error `I*Q` (a decision-free
//! discriminator), and drives a proportional-integral loop filter so the NCO
//! tracks the carrier. The integrator state *is* the frequency estimate, so a
//! fixed offset is recovered directly. [`Afc`] wraps the loop with a slow
//! frequency-offset tracker that follows linear drift within tolerance.

use crate::types::Cplx;
use std::f32::consts::TAU;

/// Second-order BPSK Costas loop.
pub struct CostasLoop {
    /// NCO phase, radians.
    phase: f32,
    /// NCO angular frequency, radians/sample (the frequency estimate).
    freq: f32,
    /// Proportional gain (alpha).
    alpha: f32,
    /// Integral gain (beta).
    beta: f32,
    /// Frequency clamp (radians/sample).
    max_freq: f32,
    last_err: f32,
}

impl CostasLoop {
    /// `loop_bw` is the normalized loop bandwidth (fraction of sample rate,
    /// e.g. 0.01). `max_freq_hz`/`fs` bound the NCO. A critically-damped
    /// (zeta=1/sqrt2) second-order filter is derived from the bandwidth.
    pub fn new(loop_bw: f32, max_freq_norm: f32) -> Self {
        let zeta = std::f32::consts::FRAC_1_SQRT_2;
        let wn = loop_bw; // natural frequency (normalized)
        let denom = 1.0 + 2.0 * zeta * wn + wn * wn;
        let alpha = 4.0 * zeta * wn / denom;
        let beta = 4.0 * wn * wn / denom;
        CostasLoop {
            phase: 0.0,
            freq: 0.0,
            alpha,
            beta,
            max_freq: max_freq_norm * TAU,
            last_err: 0.0,
        }
    }

    /// Frequency estimate in cycles/sample (normalized frequency).
    pub fn freq_norm(&self) -> f32 {
        self.freq / TAU
    }

    /// Last phase-error sample (radians-ish), for lock detection.
    pub fn phase_error(&self) -> f32 {
        self.last_err
    }

    /// Mix one complex baseband sample down by the NCO and advance the loop.
    /// Returns the de-rotated sample (carrier-corrected).
    pub fn process(&mut self, x: Cplx) -> Cplx {
        // De-rotate by current NCO phase.
        let nco = Cplx::new(self.phase.cos(), -self.phase.sin());
        let y = x * nco;
        // BPSK error: I*Q (sign of I removed by the product on a real-axis
        // constellation). Equivalent to 0.5*sin(2*phase_err).
        let err = y.re * y.im;
        self.last_err = err;
        // PI loop filter.
        self.freq += self.beta * err;
        self.freq = self.freq.clamp(-self.max_freq, self.max_freq);
        self.phase += self.freq + self.alpha * err;
        // Wrap phase.
        if self.phase > TAU {
            self.phase -= TAU;
        } else if self.phase < -TAU {
            self.phase += TAU;
        }
        y
    }
}

/// Slow automatic frequency control: tracks a drifting carrier offset by
/// averaging the Costas loop's frequency estimate, then re-centering. The
/// `estimate` follows linear drift with a one-pole low pass so transient jitter
/// is rejected while the trend is tracked.
pub struct Afc {
    loop_: CostasLoop,
    estimate: f32,
    smoothing: f32,
}

impl Afc {
    pub fn new(loop_bw: f32, max_freq_norm: f32, smoothing: f32) -> Self {
        Afc {
            loop_: CostasLoop::new(loop_bw, max_freq_norm),
            estimate: 0.0,
            smoothing: smoothing.clamp(0.0, 1.0),
        }
    }

    /// Smoothed normalized-frequency estimate (cycles/sample).
    pub fn estimate_norm(&self) -> f32 {
        self.estimate
    }

    pub fn process(&mut self, x: Cplx) -> Cplx {
        let y = self.loop_.process(x);
        let inst = self.loop_.freq_norm();
        self.estimate += self.smoothing * (inst - self.estimate);
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a BPSK carrier at normalized frequency `f0` with random +-1
    /// data, optionally drifting linearly to `f1` over the run.
    fn bpsk(n: usize, f0: f32, f1: f32, sps: usize) -> Vec<Cplx> {
        let mut out = Vec::with_capacity(n);
        let mut phase = 0.0f32;
        let mut seed = 0x1234_5678u32;
        let mut sym = 1.0f32;
        for i in 0..n {
            if i % sps == 0 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                sym = if seed & 0x8000_0000 != 0 { 1.0 } else { -1.0 };
            }
            let f = f0 + (f1 - f0) * (i as f32 / n as f32);
            phase += TAU * f;
            out.push(Cplx::new(sym, 0.0) * Cplx::new(phase.cos(), phase.sin()));
        }
        out
    }

    #[test]
    fn costas_recovers_fixed_offset() {
        let f0 = 0.01; // cycles/sample
        let sig = bpsk(20_000, f0, f0, 8);
        let mut loop_ = CostasLoop::new(0.01, 0.05);
        let mut y = Cplx::new(0.0, 0.0);
        for &x in &sig {
            y = loop_.process(x);
        }
        let _ = y;
        // Frequency estimate matches the offset.
        assert!(
            (loop_.freq_norm() - f0).abs() < 0.002,
            "freq est {} != {f0}",
            loop_.freq_norm()
        );
    }

    #[test]
    fn costas_phase_error_converges_to_zero() {
        let f0 = 0.008;
        let sig = bpsk(30_000, f0, f0, 8);
        let mut loop_ = CostasLoop::new(0.008, 0.05);
        let mut tail_err = 0.0f32;
        let n = sig.len();
        for (i, &x) in sig.iter().enumerate() {
            loop_.process(x);
            if i >= n - 2000 {
                tail_err += loop_.phase_error().abs();
            }
        }
        let mean = tail_err / 2000.0;
        assert!(mean < 0.05, "residual phase error {mean} too large");
    }

    #[test]
    fn afc_tracks_linear_drift() {
        // Drift from +0.006 to +0.012 cycles/sample.
        let f0 = 0.006;
        let f1 = 0.012;
        let sig = bpsk(40_000, f0, f1, 8);
        let mut afc = Afc::new(0.01, 0.05, 0.001);
        for &x in &sig {
            afc.process(x);
        }
        // Final estimate should be near the final instantaneous frequency.
        assert!(
            (afc.estimate_norm() - f1).abs() < 0.003,
            "AFC estimate {} far from final {f1}",
            afc.estimate_norm()
        );
    }
}
