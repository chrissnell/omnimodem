//! Data-carrier detect with hysteresis.
//!
//! A sliding window of recent "good symbol" flags (e.g. clean transitions, in-
//! band energy, valid eye openings) is kept as a bitmask; its popcount is the
//! quality score. Separate on/off thresholds give Schmitt-trigger hysteresis:
//! detect asserts once the score rises above `on` and stays asserted until it
//! falls below `off`. Modeled on Graywolf's AFSK/9600 DCD scorer.

/// Shift-register popcount DCD with separate on/off thresholds.
pub struct DcdScorer {
    /// Recent flags, newest in the LSB. Window length = `window`.
    reg: u64,
    window: u32,
    on_thresh: u32,
    off_thresh: u32,
    detected: bool,
}

impl DcdScorer {
    /// `window` <= 64. `on` and `off` are popcount thresholds with `off <= on`
    /// for proper hysteresis.
    pub fn new(window: u32, on: u32, off: u32) -> Self {
        assert!((1..=64).contains(&window), "window must be 1..=64");
        assert!(off <= on, "off threshold must be <= on threshold");
        assert!(on <= window, "on threshold must be <= window");
        DcdScorer { reg: 0, window, on_thresh: on, off_thresh: off, detected: false }
    }

    /// Number of "good" flags currently in the window.
    pub fn score(&self) -> u32 {
        let mask = if self.window == 64 { u64::MAX } else { (1u64 << self.window) - 1 };
        (self.reg & mask).count_ones()
    }

    /// Push one observation and return the (possibly updated) detect state.
    pub fn update(&mut self, good: bool) -> bool {
        self.reg = (self.reg << 1) | u64::from(good);
        let s = self.score();
        if self.detected {
            if s < self.off_thresh {
                self.detected = false;
            }
        } else if s > self.on_thresh {
            self.detected = true;
        }
        self.detected
    }

    pub fn is_detected(&self) -> bool {
        self.detected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asserts_above_on_threshold() {
        let mut d = DcdScorer::new(16, 12, 6);
        // 12 good flags only reaches the threshold, not above it.
        for _ in 0..12 {
            assert!(!d.update(true));
        }
        assert_eq!(d.score(), 12);
        // The 13th pushes strictly above `on`.
        assert!(d.update(true));
    }

    #[test]
    fn hysteresis_holds_between_thresholds() {
        let mut d = DcdScorer::new(16, 12, 6);
        for _ in 0..16 {
            d.update(true);
        }
        assert!(d.is_detected());
        // Bleed in bad flags: while score stays >= off it remains detected.
        // After 4 bad, score is 12 (>= off=6) -> still on.
        for _ in 0..4 {
            assert!(d.update(false));
        }
        assert!(d.is_detected());
        // Keep going until score drops below off=6.
        // window=16: after k bad flags score=16-k; below 6 needs k>=11.
        for _ in 0..6 {
            d.update(false);
        }
        // 10 bad total => score 6, still >= off, still detected.
        assert!(d.is_detected());
        assert!(!d.update(false)); // 11th bad => score 5 < off => drops.
    }

    #[test]
    fn does_not_chatter_at_boundary() {
        let mut d = DcdScorer::new(8, 6, 2);
        // Steady mediocre signal at score ~4 should not toggle once a state
        // is established. Start clean (detected), then hover.
        for _ in 0..8 {
            d.update(true);
        }
        assert!(d.is_detected());
        // Pattern that keeps score around 4 (between off=2 and on=6): it stays
        // detected because we never dip below off.
        let pat = [true, false, true, false, true, false, true, false];
        for _ in 0..4 {
            for &b in &pat {
                d.update(b);
            }
        }
        assert!(d.is_detected());
    }
}
