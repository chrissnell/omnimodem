//! Per-channel runtime audio gain: lock-free linear multipliers for RX and TX,
//! shared between the core (writer) and a worker thread (reader). Stored as
//! `f32` bits in an `AtomicU32` so the hot path reads with a single relaxed load
//! and `SetAudioGain` updates a running worker with no respawn.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// A clonable handle to one channel's RX/TX gain. Clones share the same cells.
#[derive(Clone)]
pub struct AudioGain {
    rx: Arc<AtomicU32>,
    tx: Arc<AtomicU32>,
}

impl Default for AudioGain {
    fn default() -> Self {
        AudioGain {
            rx: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            tx: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        }
    }
}

impl AudioGain {
    pub fn rx(&self) -> f32 {
        f32::from_bits(self.rx.load(Ordering::Relaxed))
    }
    pub fn tx(&self) -> f32 {
        f32::from_bits(self.tx.load(Ordering::Relaxed))
    }
    /// Set both gains. Non-finite or negative inputs are clamped to a safe range
    /// so a bad client cannot push NaN/inf into the sample path.
    pub fn set(&self, rx: f32, tx: f32) {
        self.rx.store(sanitize(rx).to_bits(), Ordering::Relaxed);
        self.tx.store(sanitize(tx).to_bits(), Ordering::Relaxed);
    }
}

/// Clamp to `[0.0, 16.0]`; map non-finite to unity. 16x (~+24 dB) is plenty of
/// headroom for a soundcard line level without letting a client send garbage.
fn sanitize(g: f32) -> f32 {
    if g.is_finite() {
        g.clamp(0.0, 16.0)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_unity() {
        let g = AudioGain::default();
        assert_eq!(g.rx(), 1.0);
        assert_eq!(g.tx(), 1.0);
    }

    #[test]
    fn set_is_visible_through_a_clone() {
        let g = AudioGain::default();
        let worker_view = g.clone();
        g.set(2.5, 0.5);
        assert_eq!(worker_view.rx(), 2.5);
        assert_eq!(worker_view.tx(), 0.5);
    }

    #[test]
    fn sanitizes_nan_and_clamps_range() {
        let g = AudioGain::default();
        g.set(f32::NAN, 1000.0);
        assert_eq!(g.rx(), 1.0); // NaN -> unity
        assert_eq!(g.tx(), 16.0); // clamped
        g.set(-3.0, 1.0);
        assert_eq!(g.rx(), 0.0); // negative -> 0 (mute)
    }
}
