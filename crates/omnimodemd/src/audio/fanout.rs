//! Capture fan-out: one capture `Receiver` -> N independent consumers. Opt-in;
//! the common 1:1 path skips it entirely. Each consumer gets every chunk; a
//! consumer that drops is removed on the next send.

use super::AudioChunk;
use std::sync::mpsc::{Receiver, SyncSender};

/// Spawn a pump that reads `source` and forwards each chunk to every consumer.
/// Returns the consumers' receivers. `n` >= 1.
pub fn fan_out(source: Receiver<AudioChunk>, n: usize) -> Vec<Receiver<AudioChunk>> {
    assert!(n >= 1);
    let mut senders: Vec<SyncSender<AudioChunk>> = Vec::with_capacity(n);
    let mut receivers = Vec::with_capacity(n);
    for _ in 0..n {
        let (tx, rx) = std::sync::mpsc::sync_channel(super::CHUNK_QUEUE_DEPTH);
        senders.push(tx);
        receivers.push(rx);
    }
    std::thread::spawn(move || {
        while let Ok(chunk) = source.recv() {
            // Forward to all consumers; keep only those still connected.
            senders.retain(|tx| {
                !matches!(
                    tx.try_send(chunk.clone()),
                    Err(std::sync::mpsc::TrySendError::Disconnected(_))
                )
            });
        }
    });
    receivers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::backend::AudioBackend;
    use crate::audio::file::FileBackend;

    #[test]
    fn single_consumer_is_passthrough_equivalent() {
        let input: Vec<i16> = (0..1500).collect();
        let cap = FileBackend::from_samples(input.clone(), 48_000)
            .open_capture(48_000)
            .unwrap();
        let mut outs = fan_out(cap.rx, 1);
        let rx = outs.pop().unwrap();
        let mut got = Vec::new();
        while let Ok(c) = rx.recv() {
            got.extend_from_slice(&c);
        }
        assert_eq!(got, input);
    }

    #[test]
    fn two_consumers_each_get_every_sample() {
        let input: Vec<i16> = (0..1500).collect();
        let cap = FileBackend::from_samples(input.clone(), 48_000)
            .open_capture(48_000)
            .unwrap();
        let outs = fan_out(cap.rx, 2);
        let mut collected: Vec<Vec<i16>> = Vec::new();
        for rx in outs {
            let mut got = Vec::new();
            while let Ok(c) = rx.recv() {
                got.extend_from_slice(&c);
            }
            collected.push(got);
        }
        assert_eq!(collected[0], input);
        assert_eq!(collected[1], input);
    }
}
