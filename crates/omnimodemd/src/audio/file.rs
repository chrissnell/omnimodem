//! File audio backend: replay raw little-endian i16 mono PCM deterministically,
//! and capture-to-vec for playback round-trip tests. The basis of the design's
//! record/replay corpus.

use super::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use super::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH};
use crate::ids::DeviceId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Samples per replayed chunk (~20 ms at 48 kHz). Matches Graywolf's stdin chunk.
const CHUNK_SAMPLES: usize = 960;

/// Replays a fixed buffer as capture; collects submitted samples for playback.
pub struct FileBackend {
    samples: Arc<Vec<i16>>,
    rate: u32,
    /// Playback sink: submitted buffers are appended here so a test can assert
    /// what "played". Shared so the test can read it after `open_playback`.
    pub played: Arc<Mutex<Vec<i16>>>,
}

impl FileBackend {
    /// Build from an in-memory buffer (tests, synthetic corpus).
    pub fn from_samples(samples: Vec<i16>, rate: u32) -> Self {
        FileBackend {
            samples: Arc::new(samples),
            rate,
            played: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Build from a raw LE-i16 PCM file. `rate` is supplied out-of-band (raw
    /// PCM carries no header).
    pub fn from_raw_file(path: &std::path::Path, rate: u32) -> Result<Self, AudioError> {
        let bytes = std::fs::read(path).map_err(|e| AudioError::Io(e.to_string()))?;
        let samples = bytes
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]))
            .collect();
        Ok(FileBackend::from_samples(samples, rate))
    }
}

impl AudioBackend for FileBackend {
    fn open_capture(&self, _requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(CHUNK_QUEUE_DEPTH);
        let samples = self.samples.clone();
        std::thread::spawn(move || {
            for chunk in samples.chunks(CHUNK_SAMPLES) {
                // Blocking send: deterministic delivery; drop on disconnect.
                if tx.send(chunk.to_vec()).is_err() {
                    break;
                }
            }
            // Sender drops here => receiver sees EOF after the last chunk.
        });
        Ok(CaptureHandle::new(rx, self.rate, || {}))
    }

    fn open_playback(&self, _requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let submitted = Arc::new(AtomicUsize::new(0));
        let drained = Arc::new(AtomicUsize::new(0));
        let d2 = drained.clone();
        let played = self.played.clone();
        std::thread::spawn(move || {
            while let Ok(buf) = rx.recv() {
                d2.fetch_add(buf.len(), Ordering::Relaxed);
                played.lock().unwrap().extend_from_slice(&buf);
            }
        });
        Ok(PlaybackHandle::new(tx, submitted, drained, self.rate))
    }

    fn device_id(&self) -> DeviceId {
        DeviceId::Placeholder { tag: "file:0".into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_replays_all_samples_then_eofs() {
        let input: Vec<i16> = (0..2500).map(|i| i as i16).collect();
        let b = FileBackend::from_samples(input.clone(), 48_000);
        let cap = b.open_capture(48_000).unwrap();

        let mut got = Vec::new();
        while let Ok(chunk) = cap.rx.recv() {
            got.extend_from_slice(&chunk);
        }
        assert_eq!(got, input); // every sample, in order, then clean EOF
    }

    #[test]
    fn playback_collects_submitted_samples() {
        let b = FileBackend::from_samples(vec![], 48_000);
        let pb = b.open_playback(48_000).unwrap();
        pb.submit(vec![1, 2, 3]).unwrap();
        let wm = pb.submit(vec![4, 5]).unwrap();
        assert_eq!(wm, 5);
        while pb.drained_samples() < 5 {
            std::thread::yield_now();
        }
        assert_eq!(*b.played.lock().unwrap(), vec![1, 2, 3, 4, 5]);
    }
}
