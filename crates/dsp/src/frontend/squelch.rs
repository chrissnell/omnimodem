//! Power squelch: opens/closes based on the smoothed power of the complex
//! channel signal, with hysteresis. When closed, the caller feeds silence to
//! the demod so an idle frequency doesn't push noise into decoders/DCD.

use crate::types::Cplx;

pub struct PowerSquelch {
    open_db: f32,
    close_db: f32,
    open: bool,
    alpha: f32,
    power: f32, // smoothed linear power
}

impl PowerSquelch {
    /// `threshold_db` is the open threshold (dBFS); the close threshold is
    /// `threshold_db - hysteresis_db`. `alpha` is the smoothing factor per block.
    pub fn new(threshold_db: f32, hysteresis_db: f32) -> Self {
        PowerSquelch {
            open_db: threshold_db,
            close_db: threshold_db - hysteresis_db,
            open: false,
            alpha: 0.5,
            power: 0.0,
        }
    }

    /// A squelch that is always open (threshold effectively -infinity). Used
    /// when the operator disables squelch.
    pub fn disabled() -> Self {
        let mut s = PowerSquelch::new(-200.0, 1.0);
        s.open = true;
        s
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Update from one block of the complex channel; returns the open state.
    pub fn observe(&mut self, channel: &[Cplx]) -> bool {
        if channel.is_empty() {
            return self.open;
        }
        let inst: f32 =
            channel.iter().map(|z| z.norm_sqr()).sum::<f32>() / channel.len() as f32;
        self.power = self.alpha * inst + (1.0 - self.alpha) * self.power;
        let db = 10.0 * (self.power + 1e-12).log10();
        if self.open {
            if db < self.close_db {
                self.open = false;
            }
        } else if db > self.open_db {
            self.open = true;
        }
        self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(mag: f32, n: usize) -> Vec<Cplx> {
        (0..n).map(|_| Cplx::new(mag, 0.0)).collect()
    }

    #[test]
    fn strong_signal_opens() {
        let mut sq = PowerSquelch::new(-20.0, 6.0);
        // 0 dBFS block, applied a few times to settle the smoother.
        for _ in 0..5 {
            sq.observe(&block(1.0, 256));
        }
        assert!(sq.is_open());
    }

    #[test]
    fn weak_signal_closes() {
        let mut sq = PowerSquelch::new(-20.0, 6.0);
        for _ in 0..5 {
            sq.observe(&block(0.001, 256)); // -60 dBFS
        }
        assert!(!sq.is_open());
    }

    #[test]
    fn hysteresis_holds_state_between_thresholds() {
        let mut sq = PowerSquelch::new(-20.0, 20.0); // open >-20, close <-40
        for _ in 0..5 {
            sq.observe(&block(1.0, 256)); // open
        }
        assert!(sq.is_open());
        // ~-30 dBFS: below open, above close → must stay open.
        for _ in 0..5 {
            sq.observe(&block(0.0316, 256));
        }
        assert!(sq.is_open());
    }

    #[test]
    fn disabled_is_always_open() {
        let mut sq = PowerSquelch::disabled();
        assert!(sq.observe(&block(0.0, 8)));
    }
}
