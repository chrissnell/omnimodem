//! Digital PLL bit-clock recovery.
//!
//! A phase accumulator advances by a fixed increment every input sample and
//! wraps once per bit; the sampling instant is the wrap point (accumulator
//! mid-bit). On every input transition the accumulator is nudged toward the
//! ideal mid-cell phase so the wrap aligns with bit centers. The nudge is
//! gentle once locked and aggressive while searching, giving the loop inertia
//! that ignores a single jittered transition but still acquires quickly from
//! cold. Modeled on Graywolf's AFSK DPLL (`demod_afsk.rs`).

/// One-pole DPLL operating at a fixed samples-per-symbol ratio.
///
/// `update` is called once per input sample with whether the demodulated
/// level changed since the previous sample. It returns `Some(bit)` at each
/// sampling instant (once per bit period) and `None` otherwise. The emitted
/// `bit` is the demodulated level sampled at the bit center.
pub struct DpllClockRecovery {
    /// Phase accumulator in [0, 1); wraps once per bit.
    phase: f32,
    /// Nominal phase advance per input sample (1 / samples_per_symbol).
    inc: f32,
    /// Gain applied to the phase error while locked (gentle).
    locked_gain: f32,
    /// Gain applied while searching (aggressive).
    search_gain: f32,
    /// Lock confidence in [0, 1]; high => use locked_gain.
    confidence: f32,
    /// Last input level, for sampling at the wrap instant.
    last_level: bool,
}

impl DpllClockRecovery {
    /// `sps` = input samples per symbol (bit). Must be >= 2.
    pub fn new(sps: f32) -> Self {
        assert!(sps >= 2.0, "samples per symbol must be >= 2");
        DpllClockRecovery {
            phase: 0.0,
            inc: 1.0 / sps,
            locked_gain: 0.05,
            search_gain: 0.30,
            confidence: 0.0,
            last_level: false,
        }
    }

    /// True once the loop reports it has acquired the bit clock.
    pub fn is_locked(&self) -> bool {
        self.confidence > 0.5
    }

    /// Current accumulator phase in [0, 1), for tests/diagnostics.
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Advance one input sample. `level` is the current demodulated bit level;
    /// the DPLL detects transitions internally. Returns `Some(bit)` at the
    /// sampling instant.
    pub fn feed(&mut self, level: bool) -> Option<bool> {
        let transition = level != self.last_level;
        self.last_level = level;
        self.update_phase(transition);
        // Sample at the wrap.
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            Some(level)
        } else {
            None
        }
    }

    /// Lower-level entry point: caller supplies the transition flag directly.
    /// Returns `Some(level)` at the sampling instant, where `level` is the
    /// most recent level passed to `feed`/`set_level`.
    pub fn update(&mut self, transition: bool) -> Option<bool> {
        if transition {
            self.last_level = !self.last_level;
        }
        self.update_phase(transition);
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            Some(self.last_level)
        } else {
            None
        }
    }

    fn update_phase(&mut self, transition: bool) {
        self.phase += self.inc;
        if transition {
            // A transition should land at phase 0.5 (cell boundary is at the
            // wrap; data edges sit a half-cell from the sampling instant).
            // Pull the accumulator so the *next* wrap centers the bit.
            let err = 0.5 - self.phase.fract();
            let gain = self.locked_gain * self.confidence
                + self.search_gain * (1.0 - self.confidence);
            self.phase += err * gain;
            // Small, well-timed errors build confidence; large ones erode it.
            let aligned = 1.0 - (err.abs() / 0.5).min(1.0);
            self.confidence += (aligned - self.confidence) * 0.1;
            self.confidence = self.confidence.clamp(0.0, 1.0);
        }
        if self.phase < 0.0 {
            self.phase += 1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate an NRZ sample stream from `bits` at `sps` samples/bit, with the
    /// actual baud rate stretched by `skew` (1.0 = nominal). Returns the
    /// per-sample levels.
    fn render(bits: &[bool], sps: f32, skew: f32) -> Vec<bool> {
        let cell = sps * skew;
        let mut out = Vec::new();
        let total = (bits.len() as f32 * cell).floor() as usize;
        for n in 0..total {
            let idx = (n as f32 / cell).floor() as usize;
            out.push(bits[idx.min(bits.len() - 1)]);
        }
        out
    }

    #[test]
    fn recovers_bits_at_nominal_rate() {
        let bits: Vec<bool> = (0..200).map(|i| i % 3 == 0 || i % 5 == 0).collect();
        let samples = render(&bits, 8.0, 1.0);
        let mut dpll = DpllClockRecovery::new(8.0);
        let recovered: Vec<bool> = samples.iter().filter_map(|&l| dpll.feed(l)).collect();
        // Roughly one bit per cell.
        assert!((recovered.len() as i32 - bits.len() as i32).abs() <= 2);
    }

    #[test]
    fn locks_at_slightly_off_baud_rate() {
        // Alternating bits give a transition every cell — easy to track.
        let bits: Vec<bool> = (0..400).map(|i| i % 2 == 0).collect();
        let samples = render(&bits, 10.0, 1.03); // 3% fast
        let mut dpll = DpllClockRecovery::new(10.0);
        for &l in &samples {
            dpll.feed(l);
        }
        assert!(dpll.is_locked(), "DPLL failed to lock on 3%-skewed clock");
    }

    #[test]
    fn sampling_instant_converges_to_mid_bit() {
        // Drive transitions every cell; the wrap should settle so transitions
        // land near phase 0.5 (i.e. samples are taken mid-bit).
        let mut dpll = DpllClockRecovery::new(8.0);
        // Warm up to lock.
        let bits: Vec<bool> = (0..400).map(|i| i % 2 == 0).collect();
        let samples = render(&bits, 8.0, 1.0);
        let mut last_phase_at_transition = 0.0;
        let mut prev = false;
        for &l in &samples {
            let t = l != prev;
            prev = l;
            dpll.feed(l);
            if t {
                last_phase_at_transition = dpll.phase();
            }
        }
        // Once converged the transition phase sits near 0.5.
        assert!(
            (last_phase_at_transition - 0.5).abs() < 0.12,
            "transition phase {last_phase_at_transition} not near mid-bit"
        );
    }

    #[test]
    fn locked_inertia_resists_single_noisy_transition() {
        let mut dpll = DpllClockRecovery::new(8.0);
        // Lock firmly with a clean alternating pattern.
        let bits: Vec<bool> = (0..600).map(|i| i % 2 == 0).collect();
        let clean = render(&bits, 8.0, 1.0);
        let mut prev = false;
        for &l in &clean {
            let _ = l != prev;
            prev = l;
            dpll.feed(l);
        }
        assert!(dpll.is_locked());
        let phase_before = dpll.phase();
        let conf_before = dpll.confidence;
        // Inject one spurious transition (a glitch) and measure the kick.
        dpll.update(true);
        let kick = (dpll.phase() - (phase_before + dpll.inc)).abs();
        // Locked gain is small, so the kick is bounded well below a search-mode
        // correction would be.
        assert!(kick < 0.05, "locked DPLL moved {kick} on one glitch");
        // Confidence barely changes from a single event.
        assert!((dpll.confidence - conf_before).abs() < 0.15);
    }
}
