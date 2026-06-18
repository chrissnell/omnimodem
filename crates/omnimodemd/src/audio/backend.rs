//! The `AudioBackend` trait and its handles. Hardware backends (cpal) and
//! virtual backends (file/stdin/null) all implement this one seam.

use super::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH};
use crate::ids::DeviceId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;

/// A running capture: pull `AudioChunk`s until the backend stops or the handle
/// drops (which tears the stream down).
pub struct CaptureHandle {
    pub rx: Receiver<AudioChunk>,
    /// Actual working rate the stream opened at (may differ from requested).
    pub sample_rate: u32,
    /// Dropping this stops the underlying stream. Backends store their stop
    /// hook here as a boxed closure so the trait stays object-safe.
    _stop: Box<dyn FnOnce() + Send>,
}

impl CaptureHandle {
    pub fn new(
        rx: Receiver<AudioChunk>,
        sample_rate: u32,
        stop: impl FnOnce() + Send + 'static,
    ) -> Self {
        CaptureHandle { rx, sample_rate, _stop: Box::new(stop) }
    }
}

/// A running playback. Submit owned i16 buffers; the backend drains them to the
/// DAC. `submitted`/`drained` are cumulative sample counts forming the
/// watermark the no-sleep TX cycle waits on (Task 13). Lifted from Graywolf's
/// `AudioSink`.
pub struct PlaybackHandle {
    tx: SyncSender<AudioChunk>,
    submitted: Arc<AtomicUsize>,
    drained: Arc<AtomicUsize>,
    pub sample_rate: u32,
}

impl PlaybackHandle {
    pub fn new(
        tx: SyncSender<AudioChunk>,
        submitted: Arc<AtomicUsize>,
        drained: Arc<AtomicUsize>,
        sample_rate: u32,
    ) -> Self {
        PlaybackHandle { tx, submitted, drained, sample_rate }
    }

    /// Queue samples for playback. Returns the cumulative submitted watermark.
    pub fn submit(&self, samples: AudioChunk) -> Result<usize, AudioError> {
        let n = samples.len();
        let total = self.submitted.fetch_add(n, Ordering::Relaxed) + n;
        self.tx.send(samples).map_err(|e| AudioError::Io(e.to_string()))?;
        Ok(total)
    }

    /// Cumulative samples the DAC callback has consumed.
    pub fn drained_samples(&self) -> usize {
        self.drained.load(Ordering::Relaxed)
    }
}

/// The pluggable audio backend. One device == one backend instance.
pub trait AudioBackend: Send {
    /// Open a capture stream at (up to) `requested_rate`, mono.
    fn open_capture(&self, requested_rate: u32) -> Result<CaptureHandle, AudioError>;
    /// Open a playback stream at (up to) `requested_rate`, mono.
    fn open_playback(&self, requested_rate: u32) -> Result<PlaybackHandle, AudioError>;
    /// The identity this backend represents.
    fn device_id(&self) -> DeviceId;
}

/// A backend with no hardware: capture yields the silence the test feeds it,
/// playback drains instantly. Used by every consumer test in this plan.
pub struct NullBackend {
    id: DeviceId,
    rate: u32,
}

impl NullBackend {
    pub fn new(rate: u32) -> Self {
        NullBackend { id: DeviceId::placeholder(), rate }
    }
}

impl AudioBackend for NullBackend {
    fn open_capture(&self, _requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        // Empty receiver whose sender is dropped: capture is immediately EOF.
        let (_tx, rx) = std::sync::mpsc::sync_channel(CHUNK_QUEUE_DEPTH);
        Ok(CaptureHandle::new(rx, self.rate, || {}))
    }

    fn open_playback(&self, _requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        // A draining thread that instantly "plays" whatever is submitted, so
        // the watermark contract holds without a DAC.
        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let submitted = Arc::new(AtomicUsize::new(0));
        let drained = Arc::new(AtomicUsize::new(0));
        let d2 = drained.clone();
        std::thread::spawn(move || {
            while let Ok(buf) = rx.recv() {
                d2.fetch_add(buf.len(), Ordering::Relaxed);
            }
        });
        Ok(PlaybackHandle::new(tx, submitted, drained, self.rate))
    }

    fn device_id(&self) -> DeviceId {
        self.id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_capture_is_immediate_eof() {
        let b = NullBackend::new(48_000);
        let cap = b.open_capture(48_000).unwrap();
        assert_eq!(cap.sample_rate, 48_000);
        // Sender dropped inside open_capture => recv errors (EOF), no hang.
        assert!(cap.rx.recv().is_err());
    }

    #[test]
    fn null_playback_drains_to_the_submitted_watermark() {
        let b = NullBackend::new(48_000);
        let pb = b.open_playback(48_000).unwrap();
        let wm = pb.submit(vec![0i16; 480]).unwrap();
        assert_eq!(wm, 480);
        // The drain thread catches up to the watermark.
        let mut spins = 0;
        while pb.drained_samples() < wm && spins < 10_000 {
            std::thread::yield_now();
            spins += 1;
        }
        assert_eq!(pb.drained_samples(), 480);
    }
}
