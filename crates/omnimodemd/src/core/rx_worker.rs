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
use crate::core::event::{FrameEvent, TelemetryEvent};
use crate::ids::{ChannelId, DeviceId};
use crate::metrics::ChannelMetrics;
use crate::ptt::interlock::RxTxInterlock;
use omnimodem_dsp::frontend::resample::Resampler;
use omnimodem_dsp::mode::{BlockDemodulator, Demodulator};
use omnimodem_dsp::types::{Frame, FramePayload, Sample};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::broadcast;

/// Shared per-channel metrics accumulator: the RX worker writes it on each
/// decode, the core reads its latest snapshot to answer `GetMetrics`.
pub type SharedMetrics = Arc<Mutex<ChannelMetrics>>;

/// How often the worker re-checks its stop flag while no audio is arriving.
const STOP_POLL: Duration = Duration::from_millis(200);

/// A running RX worker. `stop()` (or dropping the handle) signals the thread to
/// exit, which drops the `CaptureHandle` it owns and tears the capture stream
/// down — essential for the real (never-EOF) cpal capture, where without an
/// explicit stop the thread would run forever.
pub struct RxWorker {
    running: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl RxWorker {
    #[allow(clippy::too_many_arguments)]
    /// Spawn a streaming-demod RX worker. `capture` is moved into the thread;
    /// the thread runs until the capture ends or the worker is stopped.
    pub fn spawn_streaming(
        channel: ChannelId,
        rig: DeviceId,
        capture: CaptureHandle,
        mut demod: Box<dyn Demodulator>,
        interlock: RxTxInterlock,
        frames: broadcast::Sender<FrameEvent>,
        telemetry: broadcast::Sender<TelemetryEvent>,
        metrics: SharedMetrics,
        gain: crate::core::AudioGain,
    ) -> Self {
        let in_rate = capture.sample_rate;
        let native = demod.caps().native_rate;
        let running = Arc::new(AtomicBool::new(true));
        let run = running.clone();
        let join = std::thread::Builder::new()
            .name(format!("omnimodem-rx-{}", channel.0))
            .spawn(move || {
                let mut resampler =
                    (in_rate != native).then(|| Resampler::new(in_rate, native, 16));
                loop {
                    if !run.load(Ordering::Relaxed) {
                        break;
                    }
                    match capture.rx.recv_timeout(STOP_POLL) {
                        Ok(chunk) => {
                            if interlock.is_muted(&rig) {
                                continue; // our TX is keyed on this rig
                            }
                            let mut samples = resample(&mut resampler, to_f32(&chunk));
                            // Apply runtime RX gain (one relaxed load per chunk).
                            let g = gain.rx();
                            if g != 1.0 {
                                for s in samples.iter_mut() {
                                    *s *= g;
                                }
                            }
                            let mut produced = false;
                            for f in demod.feed(&samples) {
                                record(&metrics, &f);
                                emit(&frames, channel, &f.payload);
                                produced = true;
                            }
                            if produced {
                                emit_metrics(&telemetry, channel, &metrics);
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => continue, // re-check `run`
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                // Stream ended: flush any buffered partial decode (e.g. CW's
                // final word, which has no trailing terminator).
                let mut produced = false;
                for f in demod.flush() {
                    record(&metrics, &f);
                    emit(&frames, channel, &f.payload);
                    produced = true;
                }
                if produced {
                    emit_metrics(&telemetry, channel, &metrics);
                }
            })
            .expect("spawn rx worker");
        RxWorker { running, join: Some(join) }
    }

    #[allow(clippy::too_many_arguments)]
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
        telemetry: broadcast::Sender<TelemetryEvent>,
        metrics: SharedMetrics,
        window_s: f32,
        gain: crate::core::AudioGain,
    ) -> Self {
        let in_rate = capture.sample_rate;
        let native = demod.caps().native_rate;
        let win_samples = (native as f32 * window_s) as usize;
        let running = Arc::new(AtomicBool::new(true));
        let run = running.clone();
        let join = std::thread::Builder::new()
            .name(format!("omnimodem-rx-win-{}", channel.0))
            .spawn(move || {
                let mut resampler =
                    (in_rate != native).then(|| Resampler::new(in_rate, native, 16));
                let mut buf: Vec<Sample> = Vec::with_capacity(win_samples);
                let mut muted_window = false;
                let ended = loop {
                    if !run.load(Ordering::Relaxed) {
                        break false; // stopped: skip the trailing-window decode
                    }
                    match capture.rx.recv_timeout(STOP_POLL) {
                        Ok(chunk) => {
                            if interlock.is_muted(&rig) {
                                muted_window = true; // a TX overlapped this window
                            }
                            let mut chunk_samples = resample(&mut resampler, to_f32(&chunk));
                            let g = gain.rx();
                            if g != 1.0 {
                                for s in chunk_samples.iter_mut() {
                                    *s *= g;
                                }
                            }
                            buf.extend_from_slice(&chunk_samples);
                            while buf.len() >= win_samples {
                                let window: Vec<Sample> = buf.drain(..win_samples).collect();
                                if !muted_window {
                                    let mut produced = false;
                                    for f in demod.decode_window(&window, 0) {
                                        record(&metrics, &f);
                                        emit(&frames, channel, &f.payload);
                                        produced = true;
                                    }
                                    if produced {
                                        emit_metrics(&telemetry, channel, &metrics);
                                    }
                                }
                                muted_window = false;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break true,
                    }
                };
                // On a natural end, decode a trailing partial window padded to
                // full length (the file path's single window may be exactly
                // `win_samples`; this also handles a short remainder).
                if ended && !buf.is_empty() && !muted_window {
                    buf.resize(win_samples, 0.0);
                    let mut produced = false;
                    for f in demod.decode_window(&buf, 0) {
                        record(&metrics, &f);
                        emit(&frames, channel, &f.payload);
                        produced = true;
                    }
                    if produced {
                        emit_metrics(&telemetry, channel, &metrics);
                    }
                }
            })
            .expect("spawn windowed rx worker");
        RxWorker { running, join: Some(join) }
    }

    /// Signal the worker to stop and wait for it (used in tests / graceful paths).
    pub fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }

    /// Wait for the worker to finish on its own (capture must end on its own,
    /// e.g. the file backend's finite replay).
    pub fn join(mut self) {
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for RxWorker {
    fn drop(&mut self) {
        // Signal stop and detach. The thread exits within one `STOP_POLL` and
        // drops its `CaptureHandle`, stopping the underlying stream — so the
        // core's `remove()` is non-blocking and never leaks the thread/device.
        self.running.store(false, Ordering::Relaxed);
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

/// Fold one decoded frame into the shared accumulator: count it good/bad by CRC,
/// remember the decoder, and absorb whatever signal-quality fields the DSP layer
/// measured.
fn record(metrics: &SharedMetrics, f: &Frame) {
    let mut m = metrics.lock().unwrap();
    m.record_frame(f.meta.crc_ok, f.meta.decoder.as_deref());
    if let Some(s) = f.meta.snr_db {
        m.snr_db = s;
    }
    if let Some(off) = f.meta.freq_offset_hz {
        m.afc_offset_hz = off;
    }
}

/// Publish the latest accumulator state on the lossy telemetry channel. Called
/// once per audio chunk/window that produced at least one frame, so a busy
/// channel doesn't flood the (lossy) broadcast with per-frame churn.
fn emit_metrics(
    telemetry: &broadcast::Sender<TelemetryEvent>,
    channel: ChannelId,
    metrics: &SharedMetrics,
) {
    let m = metrics.lock().unwrap();
    let _ = telemetry.send(TelemetryEvent::ChannelMetrics {
        channel,
        good_frames: m.good_frames,
        bad_frames: m.bad_frames,
        snr_db: m.snr_db,
        dbfs: m.dbfs,
        afc_offset_hz: m.afc_offset_hz,
        dcd: m.dcd,
        last_decoder: m.last_decoder.clone(),
    });
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

    fn test_metrics() -> SharedMetrics {
        Arc::new(Mutex::new(ChannelMetrics::default()))
    }

    /// A streaming demod that records the peak |sample| it is fed, so a test can
    /// observe the samples *after* RX gain is applied but before any decode.
    struct PeakRecordingDemod {
        rate: u32,
        peak: Arc<Mutex<f32>>,
    }

    impl omnimodem_dsp::mode::Demodulator for PeakRecordingDemod {
        fn caps(&self) -> omnimodem_dsp::mode::ModeCaps {
            omnimodem_dsp::mode::ModeCaps {
                native_rate: self.rate,
                bandwidth_hz: 1000.0,
                tx: false,
                duplex: omnimodem_dsp::mode::Duplex::Half,
                shape: omnimodem_dsp::mode::DemodShape::Streaming,
            }
        }
        fn feed(&mut self, samples: &[omnimodem_dsp::types::Sample]) -> Vec<Frame> {
            let mut p = self.peak.lock().unwrap();
            for &s in samples {
                *p = p.max(s.abs());
            }
            Vec::new()
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn rx_gain_scales_samples_before_the_demod() {
        // Constant 0.5-full-scale capture (i16 16384 ≈ 0.5 after to_f32). With the
        // demod's native rate == capture rate there is no resampling, so the demod
        // sees exactly the gained samples. rx_gain = 3.0 must triple the peak.
        let backend = FileBackend::from_samples(vec![16_384i16; 4_800], 48_000);
        let capture = backend.open_capture(48_000).unwrap();

        let peak = Arc::new(Mutex::new(0.0f32));
        let gain = crate::core::AudioGain::default();
        gain.set(3.0, 1.0);
        let worker = RxWorker::spawn_streaming(
            ChannelId(0),
            DeviceId::placeholder(),
            capture,
            Box::new(PeakRecordingDemod { rate: 48_000, peak: peak.clone() }),
            RxTxInterlock::new(),
            broadcast::channel(8).0,
            broadcast::channel(8).0,
            test_metrics(),
            gain,
        );
        worker.join();

        let observed = *peak.lock().unwrap();
        // Input peak ≈ 0.5; gained ≈ 1.5. Allow generous tolerance for i16 rounding.
        assert!(
            (observed - 1.5).abs() < 0.05,
            "rx_gain not applied: observed peak {observed}, expected ≈1.5 (0.5 × 3.0)"
        );
    }

    #[test]
    fn rx_unity_gain_leaves_samples_unscaled() {
        // The default-unity path must not alter samples: peak stays ≈ 0.5.
        let backend = FileBackend::from_samples(vec![16_384i16; 4_800], 48_000);
        let capture = backend.open_capture(48_000).unwrap();
        let peak = Arc::new(Mutex::new(0.0f32));
        let worker = RxWorker::spawn_streaming(
            ChannelId(0),
            DeviceId::placeholder(),
            capture,
            Box::new(PeakRecordingDemod { rate: 48_000, peak: peak.clone() }),
            RxTxInterlock::new(),
            broadcast::channel(8).0,
            broadcast::channel(8).0,
            test_metrics(),
            crate::core::AudioGain::default(),
        );
        worker.join();
        let observed = *peak.lock().unwrap();
        assert!((observed - 0.5).abs() < 0.05, "unity gain altered samples: peak {observed}");
    }

    #[test]
    fn stop_halts_a_worker_on_an_infinite_capture() {
        // Simulate the real cpal capture, which never EOFs: keep the producer
        // sender alive and never disconnect. `stop()` must still return (the old
        // recv-forever loop would hang here, leaking the thread).
        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(4);
        let capture = CaptureHandle::new(rx, 48_000, || {});
        let worker = RxWorker::spawn_streaming(
            ChannelId(0),
            DeviceId::placeholder(),
            capture,
            Box::new(Afsk1200Demod::ensemble(9)),
            RxTxInterlock::new(),
            broadcast::channel(8).0,
            broadcast::channel(8).0,
            test_metrics(),
            crate::core::AudioGain::default(),
        );
        // Worker is blocked in recv_timeout with no data. stop() joins it.
        let start = std::time::Instant::now();
        worker.stop();
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "stop() hung");
        drop(tx); // producer outlived the worker, as cpal's would
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
            broadcast::channel(8).0,
            test_metrics(),
            crate::core::AudioGain::default(),
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
            broadcast::channel(8).0,
            test_metrics(),
            crate::core::AudioGain::default(),
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
            broadcast::channel(8).0,
            test_metrics(),
            FT8_WINDOW_S,
            crate::core::AudioGain::default(),
        );
        worker.join();

        let mut texts = Vec::new();
        while let Ok(FrameEvent::RxFrame { data, .. }) = rx_b.try_recv() {
            texts.push(String::from_utf8_lossy(&data).to_string());
        }
        assert!(texts.iter().any(|t| t == "CQ K1ABC FN42"), "got {texts:?}");
    }

    #[test]
    fn rx_worker_emits_channel_metrics_after_decode() {
        // Replay a valid AFSK frame and assert the worker both updates the shared
        // accumulator (good_frames >= 1) and publishes a ChannelMetrics telemetry
        // event reflecting it.
        use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
        let ax = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("K1ABC", 2),
            digipeaters: vec![],
            info: b"metrics".to_vec(),
        };
        let f32s = Afsk1200Mod::new().modulate(&Frame::packet(ax.encode())).unwrap();
        let backend = FileBackend::from_samples(to_i16(&f32s), 48_000);
        let capture = backend.open_capture(48_000).unwrap();

        let (tele_tx, mut tele_rx) = broadcast::channel(64);
        let metrics = test_metrics();
        let worker = RxWorker::spawn_streaming(
            ChannelId(3),
            DeviceId::placeholder(),
            capture,
            Box::new(Afsk1200Demod::ensemble(9)),
            RxTxInterlock::new(),
            broadcast::channel(64).0,
            tele_tx,
            metrics.clone(),
            crate::core::AudioGain::default(),
        );
        worker.join();

        assert!(metrics.lock().unwrap().good_frames >= 1, "accumulator not updated");
        let mut saw = false;
        while let Ok(ev) = tele_rx.try_recv() {
            if let TelemetryEvent::ChannelMetrics { channel, good_frames, .. } = ev {
                if channel == ChannelId(3) && good_frames >= 1 {
                    saw = true;
                }
            }
        }
        assert!(saw, "no ChannelMetrics telemetry emitted");
    }
}
