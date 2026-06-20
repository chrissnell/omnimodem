//! Per-channel RX worker. Pulls `AudioChunk`s from a capture, resamples to the
//! mode's native rate, drives a streaming or windowed demod, honors the per-rig
//! RX/TX interlock (skip decode while we key the rig), and emits decoded frames
//! on the LOSSLESS frame broadcast.
//!
//! This is the first code that actually consumes captured audio — Phase 3 held
//! the capture handle idle. The worker thread runs until the capture stream
//! ends (its sender drops) or the handle is torn down.

use crate::audio::backend::CaptureHandle;
use crate::audio::AudioChunk;
use crate::core::event::FrameEvent;
use crate::ids::{ChannelId, DeviceId};
use crate::ptt::interlock::RxTxInterlock;
use omnimodem_dsp::frontend::resample::Resampler;
use omnimodem_dsp::mode::{BlockDemodulator, Demodulator};
use omnimodem_dsp::types::{FramePayload, Sample};
use std::thread::JoinHandle;
use tokio::sync::broadcast;

/// A running RX worker. Joining (or dropping the upstream capture) stops it.
pub struct RxWorker {
    join: Option<JoinHandle<()>>,
}

impl RxWorker {
    /// Spawn a streaming-demod RX worker. `capture` is moved into the thread;
    /// the thread runs until the capture stream ends.
    pub fn spawn_streaming(
        channel: ChannelId,
        rig: DeviceId,
        capture: CaptureHandle,
        mut demod: Box<dyn Demodulator>,
        interlock: RxTxInterlock,
        frames: broadcast::Sender<FrameEvent>,
    ) -> Self {
        let in_rate = capture.sample_rate;
        let native = demod.caps().native_rate;
        let join = std::thread::Builder::new()
            .name(format!("omnimodem-rx-{}", channel.0))
            .spawn(move || {
                let mut resampler =
                    (in_rate != native).then(|| Resampler::new(in_rate, native, 16));
                while let Ok(chunk) = capture.rx.recv() {
                    if interlock.is_muted(&rig) {
                        continue; // our TX is keyed on this rig; don't self-decode
                    }
                    let samples = resample(&mut resampler, to_f32(&chunk));
                    for f in demod.feed(&samples) {
                        emit(&frames, channel, &f.payload);
                    }
                }
            })
            .expect("spawn rx worker");
        RxWorker { join: Some(join) }
    }

    /// Spawn a windowed (block) RX worker. Buffers `window_s` of samples at the
    /// demod's native rate, calls `decode_window`, then advances by one window.
    /// Production aligns the window to the wall-clock slot via `SlotClock`; this
    /// base spawner aligns to the capture start, which is what the deterministic
    /// file-replay path needs.
    pub fn spawn_windowed(
        channel: ChannelId,
        rig: DeviceId,
        capture: CaptureHandle,
        mut demod: Box<dyn BlockDemodulator>,
        interlock: RxTxInterlock,
        frames: broadcast::Sender<FrameEvent>,
        window_s: f32,
    ) -> Self {
        let in_rate = capture.sample_rate;
        let native = demod.caps().native_rate;
        let win_samples = (native as f32 * window_s) as usize;
        let join = std::thread::Builder::new()
            .name(format!("omnimodem-rx-win-{}", channel.0))
            .spawn(move || {
                let mut resampler =
                    (in_rate != native).then(|| Resampler::new(in_rate, native, 16));
                let mut buf: Vec<Sample> = Vec::with_capacity(win_samples);
                let mut muted_window = false;
                while let Ok(chunk) = capture.rx.recv() {
                    if interlock.is_muted(&rig) {
                        muted_window = true; // a TX overlapped this window
                    }
                    buf.extend_from_slice(&resample(&mut resampler, to_f32(&chunk)));
                    while buf.len() >= win_samples {
                        let window: Vec<Sample> = buf.drain(..win_samples).collect();
                        if !muted_window {
                            for f in demod.decode_window(&window, 0) {
                                emit(&frames, channel, &f.payload);
                            }
                        }
                        muted_window = false;
                    }
                }
                // Decode a trailing partial window padded to full length (the
                // file path's single window may be exactly `win_samples`; this
                // also handles a short remainder).
                if !buf.is_empty() && !muted_window {
                    buf.resize(win_samples, 0.0);
                    for f in demod.decode_window(&buf, 0) {
                        emit(&frames, channel, &f.payload);
                    }
                }
            })
            .expect("spawn windowed rx worker");
        RxWorker { join: Some(join) }
    }

    /// Wait for the worker thread to finish (capture must have ended).
    pub fn join(mut self) {
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn resample(r: &mut Option<Resampler>, samples: Vec<Sample>) -> Vec<Sample> {
    match r.as_mut() {
        Some(r) => r.process(&samples),
        None => samples,
    }
}

fn to_f32(chunk: &AudioChunk) -> Vec<Sample> {
    chunk.iter().map(|&s| s as f32 / 32768.0).collect()
}

fn emit(frames: &broadcast::Sender<FrameEvent>, channel: ChannelId, payload: &FramePayload) {
    let _ = frames.send(FrameEvent::RxFrame { channel, data: frame_bytes(payload), timestamp_ns: 0 });
}

/// Flatten a decoded payload to the opaque bytes the proto `RxFrame.data`
/// carries. Text/message decode to UTF-8; packets/vocoder pass through.
fn frame_bytes(p: &FramePayload) -> Vec<u8> {
    match p {
        FramePayload::Packet(b) | FramePayload::Vocoder(b) => b.clone(),
        FramePayload::Text(t) => t.clone().into_bytes(),
        FramePayload::Message77(m) => m.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::backend::AudioBackend;
    use crate::audio::file::FileBackend;
    use omnimodem_dsp::mode::Modulator;
    use omnimodem_dsp::modes::afsk1200::{Afsk1200Demod, Afsk1200Mod};
    use omnimodem_dsp::types::Frame;

    fn to_i16(f: &[f32]) -> Vec<i16> {
        f.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
    }

    #[test]
    fn rx_worker_decodes_a_replayed_afsk_frame() {
        use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
        let ax = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("K1ABC", 7),
            digipeaters: vec![],
            info: b"rx worker test".to_vec(),
        };
        let f32s = Afsk1200Mod::new().modulate(&Frame::packet(ax.encode())).unwrap();
        let backend = FileBackend::from_samples(to_i16(&f32s), 48_000);
        let capture = backend.open_capture(48_000).unwrap();

        let (tx_b, mut rx_b) = broadcast::channel(64);
        let worker = RxWorker::spawn_streaming(
            ChannelId(0),
            DeviceId::placeholder(),
            capture,
            Box::new(Afsk1200Demod::ensemble(9)),
            RxTxInterlock::new(),
            tx_b,
        );
        worker.join();

        let mut got = Vec::new();
        while let Ok(FrameEvent::RxFrame { data, .. }) = rx_b.try_recv() {
            got.push(data);
        }
        assert!(got.iter().any(|d| d == &ax.encode()), "no matching frame: {got:?}");
    }

    #[test]
    fn rx_worker_mutes_while_interlocked() {
        // With the rig muted, the worker must emit no frames even though the
        // capture replays a valid signal.
        use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
        let ax = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("K1ABC", 1),
            digipeaters: vec![],
            info: b"muted".to_vec(),
        };
        let f32s = Afsk1200Mod::new().modulate(&Frame::packet(ax.encode())).unwrap();
        let backend = FileBackend::from_samples(to_i16(&f32s), 48_000);
        let capture = backend.open_capture(48_000).unwrap();

        let rig = DeviceId::placeholder();
        let interlock = RxTxInterlock::new();
        interlock.begin_tx(&rig); // keep the rig keyed for the whole replay

        let (tx_b, mut rx_b) = broadcast::channel(64);
        let worker = RxWorker::spawn_streaming(
            ChannelId(0),
            rig,
            capture,
            Box::new(Afsk1200Demod::ensemble(9)),
            interlock,
            tx_b,
        );
        worker.join();
        assert!(rx_b.try_recv().is_err(), "muted worker emitted a frame");
    }

    #[test]
    fn rx_worker_decodes_a_windowed_ft8_message() {
        use omnimodem_dsp::modes::ft8::{Ft8Demod, Ft8Mod, FT8_RATE, FT8_WINDOW_S};
        let wave = Ft8Mod::new().modulate(&Frame::text("CQ K1ABC FN42")).unwrap();
        let mut win = vec![0.0f32; (FT8_RATE as f32 * FT8_WINDOW_S) as usize];
        win[..wave.len()].copy_from_slice(&wave);
        let backend = FileBackend::from_samples(to_i16(&win), FT8_RATE);
        let capture = backend.open_capture(FT8_RATE).unwrap();

        let (tx_b, mut rx_b) = broadcast::channel(64);
        let worker = RxWorker::spawn_windowed(
            ChannelId(1),
            DeviceId::placeholder(),
            capture,
            Box::new(Ft8Demod::new()),
            RxTxInterlock::new(),
            tx_b,
            FT8_WINDOW_S,
        );
        worker.join();

        let mut texts = Vec::new();
        while let Ok(FrameEvent::RxFrame { data, .. }) = rx_b.try_recv() {
            texts.push(String::from_utf8_lossy(&data).to_string());
        }
        assert!(texts.iter().any(|t| t == "CQ K1ABC FN42"), "got {texts:?}");
    }
}
