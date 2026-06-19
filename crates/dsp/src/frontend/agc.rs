//! Two distinct AGCs, selected by the registry rather than composed
//! (Graywolf demod_afsk.rs:507): a peak/valley envelope tracker for the
//! multi-slicer path, and a decision-feedback AGC (W7ION) that tracks the mark
//! and space references independently in the single-slicer path.

use crate::types::Sample;

/// Fast-attack / slow-decay peak and valley envelope tracker. The instantaneous
/// signal is normalized into `[-1, 1]` against the tracked peak/valley span.
pub struct PeakValleyAgc {
    peak: f32,
    valley: f32,
    attack: f32,
    decay: f32,
}

impl PeakValleyAgc {
    /// `attack`/`decay` are per-sample smoothing coefficients in `(0, 1]`;
    /// larger = faster. Attack should exceed decay (fast up, slow down).
    pub fn new(attack: f32, decay: f32) -> Self {
        PeakValleyAgc { peak: 0.0, valley: 0.0, attack, decay }
    }

    /// Update the envelope and return the normalized sample centered on the
    /// running midpoint, scaled so the tracked span maps to roughly `[-1, 1]`.
    pub fn process(&mut self, x: Sample) -> Sample {
        if x > self.peak {
            self.peak += (x - self.peak) * self.attack;
        } else {
            self.peak += (x - self.peak) * self.decay;
        }
        if x < self.valley {
            self.valley += (x - self.valley) * self.attack;
        } else {
            self.valley += (x - self.valley) * self.decay;
        }
        let mid = (self.peak + self.valley) * 0.5;
        let half = ((self.peak - self.valley) * 0.5).max(1e-6);
        (x - mid) / half
    }

    pub fn peak(&self) -> f32 {
        self.peak
    }

    pub fn valley(&self) -> f32 {
        self.valley
    }
}

/// Decision-feedback AGC: tracks the mark and space tone amplitudes with
/// independent envelopes. The caller feeds the current per-tone correlator
/// magnitudes and a hard decision; the AGC slews the reference for the *chosen*
/// tone, giving a per-tone gain reference that resists fades on one tone.
pub struct DecisionFeedbackAgc {
    mark_ref: f32,
    space_ref: f32,
    coeff: f32,
}

impl DecisionFeedbackAgc {
    /// `coeff` is the per-update smoothing in `(0, 1]`.
    pub fn new(coeff: f32) -> Self {
        DecisionFeedbackAgc { mark_ref: 0.0, space_ref: 0.0, coeff }
    }

    /// Update the reference for the decided tone (`mark = true` => mark tone).
    pub fn update(&mut self, mark_mag: f32, space_mag: f32, mark: bool) {
        if mark {
            self.mark_ref += (mark_mag - self.mark_ref) * self.coeff;
        } else {
            self.space_ref += (space_mag - self.space_ref) * self.coeff;
        }
    }

    pub fn mark_ref(&self) -> f32 {
        self.mark_ref
    }

    pub fn space_ref(&self) -> f32 {
        self.space_ref
    }

    /// Normalized soft difference between tones using the tracked references.
    pub fn normalized_diff(&self, mark_mag: f32, space_mag: f32) -> f32 {
        let m = mark_mag / self.mark_ref.max(1e-6);
        let s = space_mag / self.space_ref.max(1e-6);
        m - s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_valley_tracks_step_and_normalizes() {
        let mut agc = PeakValleyAgc::new(0.5, 0.01);
        // Feed an alternating +/-2 square; envelope should converge so output
        // saturates toward +/-1.
        let mut last = 0.0;
        for i in 0..2000 {
            let x = if i % 2 == 0 { 2.0 } else { -2.0 };
            last = agc.process(x);
        }
        assert!(last.abs() > 0.8, "normalized output {last} should approach +/-1");
        assert!(agc.peak() > 1.5 && agc.valley() < -1.5);
    }

    #[test]
    fn peak_attack_faster_than_decay() {
        let mut agc = PeakValleyAgc::new(0.5, 0.001);
        // One big sample: peak jumps quickly (attack).
        agc.process(1.0);
        let after_attack = agc.peak();
        assert!(after_attack > 0.4);
        // Feed zeros: peak decays only slowly.
        for _ in 0..10 {
            agc.process(0.0);
        }
        assert!(agc.peak() > after_attack * 0.9, "decay too fast");
    }

    #[test]
    fn decision_feedback_converges_to_tone_amplitudes() {
        let mut agc = DecisionFeedbackAgc::new(0.05);
        // Alternating mark (mag 1.0) and space (mag 0.4) decisions.
        for i in 0..1000 {
            if i % 2 == 0 {
                agc.update(1.0, 0.1, true);
            } else {
                agc.update(0.1, 0.4, false);
            }
        }
        assert!((agc.mark_ref() - 1.0).abs() < 0.05);
        assert!((agc.space_ref() - 0.4).abs() < 0.05);
        // With references converged, a strong mark gives a clearly positive diff.
        assert!(agc.normalized_diff(1.0, 0.1) > 0.0);
    }
}
