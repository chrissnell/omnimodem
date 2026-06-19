//! Generic "hydra" ensemble (design §"Best-of-class reception, generalized").
//! Runs N demod configurations over the same samples and unions/dedups their
//! frames. The *pattern* is mode-agnostic; the member *profiles* are supplied
//! by the mode's registry module (AFSK Profile A/B, PSK loop bandwidths, ...).

use crate::mode::{Demodulator, ModeCaps};
use crate::types::Frame;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

/// Dedup by `(content-hash, sample-offset)` within a window of `window_samples`
/// (≈3 symbol times — design §"Content+offset frame dedup"). Windowed modes
/// dedup by decoded-message + time-slot instead; that variant lives with the
/// windowed mode, not here.
pub struct DedupWindow {
    window_samples: u64,
    seen: Vec<(u64, u64)>, // (content_hash, offset)
}

impl DedupWindow {
    pub fn new(window_samples: u64) -> Self {
        DedupWindow { window_samples, seen: Vec::new() }
    }

    fn content_hash(frame: &Frame) -> u64 {
        let mut h = DefaultHasher::new();
        frame.payload.hash_into(&mut h);
        h.finish()
    }

    /// Returns true if this frame is novel (and records it); false if a near
    /// duplicate was already seen within the offset window.
    pub fn admit(&mut self, frame: &Frame) -> bool {
        let ch = Self::content_hash(frame);
        let off = frame.meta.sample_offset;
        let dup = self
            .seen
            .iter()
            .any(|&(c, o)| c == ch && off.abs_diff(o) <= self.window_samples);
        if dup {
            false
        } else {
            self.seen.push((ch, off));
            true
        }
    }

    /// Drop records older than `cutoff` to bound memory on a long stream.
    pub fn prune(&mut self, cutoff_offset: u64) {
        self.seen.retain(|&(_, o)| o + self.window_samples >= cutoff_offset);
    }

    pub fn clear(&mut self) {
        self.seen.clear();
    }
}

pub struct ParallelDemodulator<D: Demodulator> {
    members: Vec<D>,
    dedup: DedupWindow,
}

impl<D: Demodulator> ParallelDemodulator<D> {
    /// All members must share a `native_rate`; the first member's caps are the
    /// ensemble's caps. `dedup_window_samples` ≈ 3 symbol periods.
    pub fn new(members: Vec<D>, dedup_window_samples: u64) -> Self {
        assert!(!members.is_empty(), "ensemble needs at least one member");
        ParallelDemodulator { members, dedup: DedupWindow::new(dedup_window_samples) }
    }

    /// Borrow the members (e.g. for per-member metrics).
    pub fn members(&self) -> &[D] {
        &self.members
    }
}

impl<D: Demodulator> Demodulator for ParallelDemodulator<D> {
    fn caps(&self) -> ModeCaps {
        self.members[0].caps()
    }

    fn feed(&mut self, samples: &[crate::types::Sample]) -> Vec<Frame> {
        let mut out = Vec::new();
        for m in &mut self.members {
            for f in m.feed(samples) {
                if self.dedup.admit(&f) {
                    out.push(f);
                }
            }
        }
        out
    }

    fn reset(&mut self) {
        for m in &mut self.members {
            m.reset();
        }
        self.dedup.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::{DemodShape, Duplex, ModeCaps};
    use crate::types::{Frame, FrameMeta, FramePayload, Sample};

    /// A stub member that emits one fixed frame at a fixed offset on first feed.
    struct OneShot {
        frame: Frame,
        fired: bool,
    }
    impl Demodulator for OneShot {
        fn caps(&self) -> ModeCaps {
            ModeCaps {
                native_rate: 48_000,
                bandwidth_hz: 1.0,
                tx: false,
                duplex: Duplex::Half,
                shape: DemodShape::Streaming,
            }
        }
        fn feed(&mut self, _s: &[Sample]) -> Vec<Frame> {
            if self.fired {
                return vec![];
            }
            self.fired = true;
            vec![self.frame.clone()]
        }
        fn reset(&mut self) {
            self.fired = false;
        }
    }

    fn frame_at(bytes: &[u8], offset: u64) -> Frame {
        Frame {
            payload: FramePayload::Packet(bytes.to_vec()),
            meta: FrameMeta { sample_offset: offset, ..Default::default() },
        }
    }

    #[test]
    fn identical_frames_within_window_dedup_to_one() {
        let a = OneShot { frame: frame_at(b"HELLO", 1000), fired: false };
        let b = OneShot { frame: frame_at(b"HELLO", 1010), fired: false }; // 10 < window
        let mut ens = ParallelDemodulator::new(vec![a, b], 100);
        let out = ens.feed(&[0.0; 8]);
        assert_eq!(out.len(), 1, "near-duplicate should be deduped");
    }

    #[test]
    fn same_content_far_apart_is_kept() {
        let a = OneShot { frame: frame_at(b"HELLO", 1000), fired: false };
        let b = OneShot { frame: frame_at(b"HELLO", 5000), fired: false };
        let mut ens = ParallelDemodulator::new(vec![a, b], 100);
        assert_eq!(ens.feed(&[0.0; 8]).len(), 2);
    }

    #[test]
    fn different_content_same_offset_is_kept() {
        let a = OneShot { frame: frame_at(b"HELLO", 1000), fired: false };
        let b = OneShot { frame: frame_at(b"WORLD", 1000), fired: false };
        let mut ens = ParallelDemodulator::new(vec![a, b], 100);
        assert_eq!(ens.feed(&[0.0; 8]).len(), 2);
    }
}
