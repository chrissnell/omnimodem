//! Stdin audio backend: raw little-endian i16 mono PCM on stdin. Capture only
//! (you cannot play to stdin); playback errors `Unsupported`. Lifted from
//! Graywolf `audio/stdin_raw.rs`.

use super::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use super::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH};
use crate::ids::DeviceId;
use std::io::Read;

const CHUNK_BYTES: usize = 960 * 2; // ~20 ms at 48 kHz

/// Reads raw i16 PCM from stdin at a declared rate.
pub struct StdinBackend {
    rate: u32,
}

impl StdinBackend {
    pub fn new(rate: u32) -> Self {
        StdinBackend { rate }
    }
}

impl AudioBackend for StdinBackend {
    fn open_capture(&self, _requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(CHUNK_QUEUE_DEPTH);
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = vec![0u8; CHUNK_BYTES];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let chunk: AudioChunk = buf[..n]
                            .chunks_exact(2)
                            .map(|p| i16::from_le_bytes([p[0], p[1]]))
                            .collect();
                        if tx.send(chunk).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(CaptureHandle::new(rx, self.rate, || {}))
    }

    fn open_playback(&self, _requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        Err(AudioError::Unsupported)
    }

    fn device_id(&self) -> DeviceId {
        DeviceId::Placeholder { tag: "stdin:0".into() }
    }
}
