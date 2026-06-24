//! Per-channel spectrum (waterfall) control: a clonable handle the core writes
//! and the RX worker reads, so `ConfigureSpectrum` toggles a *running* worker
//! with no respawn — the same pattern as [`super::gain::AudioGain`].
//!
//! The worker can't be handed a command directly (it only reads its capture
//! channel), so enable/disable rides on this shared cell. The worker checks
//! `generation()` once per chunk (one relaxed load); only when it changes does it
//! lock the cell to (re)build or drop its `SpectrumTap`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Requested spectrum parameters for a channel (raw; defaults/clamping are
/// resolved against the demod native rate when the tap is built).
#[derive(Clone, Copy, Debug)]
pub struct SpectrumCfg {
    pub bin_count: u32,
    pub fft_size: u32,
    pub rate_hz: u32,
    pub freq_lo_hz: f32,
    pub freq_hi_hz: f32,
}

struct Inner {
    /// Bumped on every enable/disable so a worker can detect changes cheaply.
    generation: AtomicU64,
    cfg: Mutex<Option<SpectrumCfg>>,
}

/// A clonable handle to one channel's spectrum config. Clones share the cell.
#[derive(Clone)]
pub struct SpectrumControl {
    inner: Arc<Inner>,
}

impl Default for SpectrumControl {
    fn default() -> Self {
        SpectrumControl {
            inner: Arc::new(Inner { generation: AtomicU64::new(0), cfg: Mutex::new(None) }),
        }
    }
}

impl SpectrumControl {
    /// Enable (or reconfigure) the stream. Bumps the generation.
    pub fn enable(&self, cfg: SpectrumCfg) {
        *self.inner.cfg.lock().unwrap() = Some(cfg);
        self.inner.generation.fetch_add(1, Ordering::Release);
    }

    /// Disable the stream. Bumps the generation.
    pub fn disable(&self) {
        *self.inner.cfg.lock().unwrap() = None;
        self.inner.generation.fetch_add(1, Ordering::Release);
    }

    /// The current generation counter (one relaxed load; cheap to poll per chunk).
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }

    /// The current config, if enabled.
    pub fn snapshot(&self) -> Option<SpectrumCfg> {
        *self.inner.cfg.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SpectrumCfg {
        SpectrumCfg { bin_count: 256, fft_size: 2048, rate_hz: 15, freq_lo_hz: 0.0, freq_hi_hz: 0.0 }
    }

    #[test]
    fn default_is_disabled() {
        let c = SpectrumControl::default();
        assert!(c.snapshot().is_none());
    }

    #[test]
    fn enable_disable_visible_through_clone_and_bumps_generation() {
        let c = SpectrumControl::default();
        let worker = c.clone();
        let g0 = worker.generation();
        c.enable(cfg());
        assert!(worker.snapshot().is_some());
        assert_ne!(worker.generation(), g0);
        let g1 = worker.generation();
        c.disable();
        assert!(worker.snapshot().is_none());
        assert_ne!(worker.generation(), g1);
    }
}
