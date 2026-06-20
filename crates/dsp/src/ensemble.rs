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
    /// Highest sample-offset admitted so far; the prune horizon.
    latest: u64,
    seen: Vec<(u64, u64)>, // (content_hash, offset)
}

impl DedupWindow {
    pub fn new(window_samples: u64) -> Self {
        DedupWindow { window_samples, latest: 0, seen: Vec::new() }
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
        self.latest = self.latest.max(off);
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

    /// Drop records older than `cutoff` to bound memory on a long stream. A
    /// record at offset `o` is kept while it could still be within `±window`
    /// of an offset at or below `cutoff`.
    pub fn prune(&mut self, cutoff_offset: u64) {
        self.seen.retain(|&(_, o)| o + self.window_samples >= cutoff_offset);
    }

    /// Prune everything that has fallen outside the dedup window behind the
    /// latest admitted offset. Called after each `feed` so the `seen` set stays
    /// bounded on a continuous stream (a record can only ever dedup a frame
    /// within `window_samples`, so older records are dead weight).
    pub fn prune_to_latest(&mut self) {
        self.prune(self.latest);
    }

    /// Number of retained dedup records (memory-bound observability / tests).
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    pub fn clear(&mut self) {
        self.seen.clear();
        self.latest = 0;
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
        // Evict dedup records that have aged out of the window so `seen` stays
        // bounded over a long stream (the prune the plan's comment promised).
        self.dedup.prune_to_latest();
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

    #[test]
    fn dedup_set_stays_bounded_over_a_long_stream() {
        // Admit 10_000 distinct frames marching forward in offset; with a
        // 100-sample window the retained set must stay tiny, not grow with the
        // stream length (regression guard for the never-pruned `seen` set).
        let window = 100u64;
        let mut dw = DedupWindow::new(window);
        for i in 0..10_000u64 {
            let off = i * 50; // each frame 50 samples past the previous
            let f = frame_at(format!("MSG{i}").as_bytes(), off);
            assert!(dw.admit(&f), "distinct frames are always novel");
            dw.prune_to_latest();
        }
        // Only frames whose offset is within `window` behind the latest can
        // survive: latest = 9999*50, cutoff = latest - 100, so at most a couple
        // of records remain — certainly not 10_000.
        assert!(dw.len() <= 4, "dedup set grew unbounded: {} records", dw.len());
    }
}
