//! Multi-threshold slicer with a geometric space-gain table (Graywolf reuse,
//! `demod_afsk.rs`). A bank of `N` slicers decides mark-vs-space at different
//! relative gains, so a downstream ensemble can pick whichever slicing best
//! matches the actual mark/space balance of the channel.
//!
//! Each slicer `i` applies a multiplicative gain `g_i` to the space level and
//! decides `mark` when `mark > g_i · space`. The gains are spaced
//! geometrically around 1.0 (the center slicer is unity gain), covering both
//! strong-space and weak-space conditions.

/// A bank of `n` mark/space slicers with geometric space-gain thresholds.
pub struct MultiSlicer {
    gains: Vec<f32>,
}

impl MultiSlicer {
    /// Build `n` slicers (use an odd `n` so a unity-gain center exists). Gains
    /// are `ratio^(i - center)` for a fixed geometric `ratio`, monotonically
    /// increasing.
    pub fn new(n: usize) -> Self {
        assert!(n >= 1, "need at least one slicer");
        let center = (n - 1) as f32 / 2.0;
        // ~1.7 dB per step in amplitude; spans roughly ±(n/2)·1.7 dB.
        let ratio = 1.22f32;
        let gains = (0..n).map(|i| ratio.powf(i as f32 - center)).collect();
        MultiSlicer { gains }
    }

    /// Per-slicer thresholds (the space-gain multipliers), monotonically
    /// increasing.
    pub fn thresholds(&self) -> &[f32] {
        &self.gains
    }

    /// Decide mark (`true`) vs space (`false`) for each slicer.
    pub fn slice(&self, mark: f32, space: f32) -> Vec<bool> {
        self.gains.iter().map(|&g| mark > g * space).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_thresholds_monotone_geometric() {
        let s = MultiSlicer::new(9);
        let t = s.thresholds();
        assert_eq!(t.len(), 9);
        // strictly increasing
        for w in t.windows(2) {
            assert!(w[1] > w[0], "not monotone: {w:?}");
        }
        // center is unity gain
        assert!((t[4] - 1.0).abs() < 1e-6, "center gain {} != 1", t[4]);
        // geometric: constant ratio between neighbors
        let r0 = t[1] / t[0];
        for w in t.windows(2) {
            assert!((w[1] / w[0] - r0).abs() < 1e-5);
        }
        // spread covers strong-space (g<1) and weak-space (g>1)
        assert!(t[0] < 1.0 && t[8] > 1.0);
    }

    #[test]
    fn mark_dominant_slices_to_mark_at_center() {
        let s = MultiSlicer::new(9);
        let out = s.slice(2.0, 1.0); // mark clearly above unity-gain space
        assert!(out[4], "center slicer should choose mark");
        // A very strong mark beats even the highest-gain slicer.
        let strong = s.slice(100.0, 1.0);
        assert!(strong.iter().all(|&b| b));
    }

    #[test]
    fn space_dominant_slices_to_space() {
        let s = MultiSlicer::new(9);
        let out = s.slice(0.5, 1.0);
        assert!(!out[4], "center slicer should choose space");
        // A very strong space defeats even the lowest-gain slicer.
        let strong = s.slice(0.01, 1.0);
        assert!(strong.iter().all(|&b| !b));
    }

    #[test]
    fn marginal_signal_splits_across_bank() {
        // mark slightly above space: low-gain slicers say mark, high-gain say
        // space — the bank disagrees, which is the whole point.
        let s = MultiSlicer::new(9);
        let out = s.slice(1.1, 1.0);
        assert!(out[0], "weak-space slicer should pick mark");
        assert!(!out[8], "strong-space slicer should pick space");
    }
}
