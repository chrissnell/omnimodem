//! Hard limiter for the correlator front-end (Graywolf demod_afsk.rs:459-461):
//! `sign(x)` preserving zero-crossing timing. Driving the correlator with a
//! limited signal makes the tone decision amplitude-independent.

use crate::types::Sample;

/// `+ -> +1`, `- -> -1`, `0 -> 0`.
#[inline]
pub fn hard_limit(x: Sample) -> Sample {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Convenience: limit a slice into a reused buffer (no per-call alloc).
pub fn hard_limit_into(input: &[Sample], out: &mut Vec<Sample>) {
    out.clear();
    out.extend(input.iter().map(|&x| hard_limit(x)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn sign_mapping() {
        assert_eq!(hard_limit(0.001), 1.0);
        assert_eq!(hard_limit(-0.001), -1.0);
        assert_eq!(hard_limit(0.0), 0.0);
        assert_eq!(hard_limit(5.0), 1.0);
        assert_eq!(hard_limit(-5.0), -1.0);
    }

    #[test]
    fn preserves_zero_crossings() {
        let rate = 8000.0;
        let f = 1000.0;
        let sig: Vec<f32> = (0..1000).map(|n| 0.3 * (TAU * f * n as f32 / rate).sin()).collect();
        let lim: Vec<f32> = sig.iter().map(|&x| hard_limit(x)).collect();
        // Every original sign change must appear at the same index in the
        // limited signal.
        for i in 1..sig.len() {
            let orig = sig[i - 1].signum() != sig[i].signum() && sig[i] != 0.0;
            let after = lim[i - 1] != lim[i] && lim[i] != 0.0;
            if orig {
                assert!(after, "zero crossing lost at {i}");
            }
        }
    }

    #[test]
    fn limited_correlator_recovers_tone_phase() {
        // Correlate a hard-limited tone against quadrature references; the phase
        // (atan2) should match the input tone phase regardless of amplitude.
        let rate = 8000.0;
        let f = 1200.0;
        let amp = 0.05; // tiny amplitude — limiter makes it amplitude-independent
        let n = 800;
        let (mut ci, mut cq) = (0.0f32, 0.0f32);
        for k in 0..n {
            let ph = TAU * f * k as f32 / rate;
            let x = hard_limit(amp * ph.sin());
            ci += x * ph.cos();
            cq += x * ph.sin();
        }
        // Input is sin => correlation with sin (quadrature) dominates.
        assert!(cq.abs() > ci.abs());
    }
}
