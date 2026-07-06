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
use crate::core::event::{FrameEvent, RxImage, TelemetryEvent};
use crate::core::spectrum::SpectrumControl;
use crate::ids::{ChannelId, DeviceId};
use crate::metrics::ChannelMetrics;
use crate::ptt::interlock::RxTxInterlock;
use omnimodem_dsp::frontend::resample::Resampler;
use omnimodem_dsp::frontend::rsid::{RsidDetection, RsidDetector};
use omnimodem_dsp::frontend::spectrum::{half_spectrum_dbfs, SpectrumPlan, SpectrumSetup};
use omnimodem_dsp::frontend::stft::Stft;
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
        spectrum: SpectrumControl,
        rsid_rx: bool,
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
                // RSID detector tap (built only when enabled); runs over the same
                // post-gain samples the demod sees, surfacing matches as telemetry.
                let mut rsid = rsid_rx.then(|| RsidDetector::new(native, 1));
                let mut tap: Option<SpectrumTap> = None;
                let mut tap_gen = u64::MAX; // force first reconcile
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
                            // Feed the waterfall tap (when enabled) the same
                            // post-gain samples the demod sees.
                            sync_spectrum_tap(&spectrum, &mut tap, &mut tap_gen, channel, native);
                            if let Some(t) = tap.as_mut() {
                                t.process(&samples, &telemetry);
                            }
                            if let Some(d) = rsid.as_mut() {
                                for det in d.feed(&samples) {
                                    emit_rsid(&telemetry, channel, det);
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
        spectrum: SpectrumControl,
        rsid_rx: bool,
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
                let mut rsid = rsid_rx.then(|| RsidDetector::new(native, 1));
                let mut buf: Vec<Sample> = Vec::with_capacity(win_samples);
                let mut muted_window = false;
                let mut tap: Option<SpectrumTap> = None;
                let mut tap_gen = u64::MAX; // force first reconcile
                let ended = loop {
                    if !run.load(Ordering::Relaxed) {
                        break false; // stopped: skip the trailing-window decode
                    }
                    match capture.rx.recv_timeout(STOP_POLL) {
                        Ok(chunk) => {
                            let muted = interlock.is_muted(&rig);
                            if muted {
                                muted_window = true; // a TX overlapped this window
                            }
                            let mut chunk_samples = resample(&mut resampler, to_f32(&chunk));
                            let g = gain.rx();
                            if g != 1.0 {
                                for s in chunk_samples.iter_mut() {
                                    *s *= g;
                                }
                            }
                            // Feed the waterfall continuously (per chunk), even
                            // though decode only fires once per multi-second
                            // window — but not while our own TX is keyed.
                            sync_spectrum_tap(&spectrum, &mut tap, &mut tap_gen, channel, native);
                            if let (Some(t), false) = (tap.as_mut(), muted) {
                                t.process(&chunk_samples, &telemetry);
                            }
                            if let (Some(d), false) = (rsid.as_mut(), muted) {
                                for det in d.feed(&chunk_samples) {
                                    emit_rsid(&telemetry, channel, det);
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

/// Worker-owned waterfall producer: a Hann STFT over the post-gain samples plus
/// the fixed rendering geometry. Built from a `SpectrumCfg` and the demod's
/// native rate (the rate of the samples it sees), rebuilt when the config or
/// rate changes. Emits one `SpectrumFrame` per STFT hop on the lossy telemetry
/// broadcast.
struct SpectrumTap {
    channel: ChannelId,
    stft: Stft,
    plan: SpectrumPlan,
}

impl SpectrumTap {
    fn build(channel: ChannelId, cfg: &crate::core::spectrum::SpectrumCfg, native_rate: u32) -> Self {
        let setup = SpectrumSetup::resolve(
            native_rate,
            cfg.bin_count,
            cfg.fft_size,
            cfg.rate_hz,
            cfg.freq_lo_hz,
            cfg.freq_hi_hz,
        );
        SpectrumTap { channel, stft: Stft::new(setup.nfft, setup.hop), plan: setup.plan }
    }

    fn process(&mut self, samples: &[Sample], telemetry: &broadcast::Sender<TelemetryEvent>) {
        for frame in self.stft.feed(samples) {
            let half = half_spectrum_dbfs(&frame, self.stft.window_sum());
            let bins = self.plan.render(&half);
            let timestamp_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let _ = telemetry.send(TelemetryEvent::SpectrumFrame {
                channel: self.channel,
                timestamp_ns,
                freq_start_hz: self.plan.freq_start_hz,
                freq_step_hz: self.plan.freq_step_hz,
                db_floor: self.plan.db_floor,
                db_ceiling: self.plan.db_ceiling,
                bins,
                transmit: false,
            });
        }
    }
}

/// Reconcile the worker's tap with the shared control when its generation has
/// changed: build/rebuild on enable, drop on disable. Cheap (one relaxed load)
/// when nothing changed.
fn sync_spectrum_tap(
    control: &SpectrumControl,
    tap: &mut Option<SpectrumTap>,
    seen_gen: &mut u64,
    channel: ChannelId,
    native_rate: u32,
) {
    let gen = control.generation();
    if gen == *seen_gen {
        return;
    }
    *seen_gen = gen;
    *tap = control.snapshot().map(|cfg| SpectrumTap::build(channel, &cfg, native_rate));
}

fn emit(frames: &broadcast::Sender<FrameEvent>, channel: ChannelId, payload: &FramePayload) {
    let ev = match payload {
        // Raster payloads travel in the typed image field; `data` stays empty.
        FramePayload::Image { width, gray } => FrameEvent::RxFrame {
            channel,
            data: Vec::new(),
            image: Some(RxImage { width: *width, gray: gray.clone() }),
            timestamp_ns: 0,
        },
        other => FrameEvent::RxFrame {
            channel,
            data: frame_bytes(other),
            image: None,
            timestamp_ns: 0,
        },
    };
    let _ = frames.send(ev);
}

/// Publish an identified RSID on the lossy telemetry broadcast.
fn emit_rsid(telemetry: &broadcast::Sender<TelemetryEvent>, channel: ChannelId, det: RsidDetection) {
    let _ = telemetry.send(TelemetryEvent::RsidDetected {
        channel,
        tag: det.tag.to_string(),
        mode: det.mode.unwrap_or("").to_string(),
        freq_hz: det.freq_hz,
        extended: det.extended,
    });
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

/// Flatten a byte-like decoded payload to the opaque bytes the proto
/// `RxFrame.data` carries. Text/message decode to UTF-8; packets/vocoder pass
/// through. Raster (`Image`) payloads do NOT come here — they travel in the
/// typed proto `Image` message (`emit` routes them), so a caller that reaches
/// this arm with an `Image` is a bug.
fn frame_bytes(p: &FramePayload) -> Vec<u8> {
    match p {
        FramePayload::Packet(b) | FramePayload::Vocoder(b) => b.clone(),
        FramePayload::Text(t) => t.clone().into_bytes(),
        FramePayload::Message77(m) => m.to_vec(),
        FramePayload::Image { .. } => {
            debug_assert!(false, "Image payloads are emitted via the typed proto Image message");
            Vec::new()
        }
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
            crate::core::spectrum::SpectrumControl::default(),
            false,
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
            crate::core::spectrum::SpectrumControl::default(),
            false,
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
            crate::core::spectrum::SpectrumControl::default(),
            false,
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
            crate::core::spectrum::SpectrumControl::default(),
            false,
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
            crate::core::spectrum::SpectrumControl::default(),
            false,
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
            crate::core::spectrum::SpectrumControl::default(),
            false,
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
            crate::core::spectrum::SpectrumControl::default(),
            false,
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

    #[test]
    fn rx_worker_detects_rsid_and_emits_event_when_enabled() {
        // Replay an RSID burst for MFSK16 at 8 kHz with the detector tap enabled;
        // the worker must surface a RsidDetected telemetry event with the tag.
        let burst = omnimodem_dsp::frontend::rsid::burst_for_mode("mfsk16", 1500.0, 8_000).unwrap();
        let mut audio = vec![0.0f32; 800];
        audio.extend_from_slice(&burst);
        audio.extend(std::iter::repeat_n(0.0, 800));
        let backend = FileBackend::from_samples(to_i16(&audio), 8_000);
        let capture = backend.open_capture(8_000).unwrap();

        let (tele_tx, mut tele_rx) = broadcast::channel(256);
        let worker = RxWorker::spawn_streaming(
            ChannelId(5),
            DeviceId::placeholder(),
            capture,
            // The demod itself is irrelevant to the RSID tap; use a passthrough.
            Box::new(PeakRecordingDemod { rate: 8_000, peak: Arc::new(Mutex::new(0.0)) }),
            RxTxInterlock::new(),
            broadcast::channel(8).0,
            tele_tx,
            test_metrics(),
            crate::core::AudioGain::default(),
            crate::core::spectrum::SpectrumControl::default(),
            true, // rsid_rx enabled
        );
        worker.join();

        let mut saw = None;
        while let Ok(ev) = tele_rx.try_recv() {
            if let TelemetryEvent::RsidDetected { channel, tag, mode, .. } = ev {
                if channel == ChannelId(5) {
                    saw = Some((tag, mode));
                }
            }
        }
        assert_eq!(saw, Some(("MFSK16".to_string(), "mfsk16".to_string())));
    }

    #[test]
    fn rx_worker_no_rsid_event_when_disabled() {
        // Same burst, detector tap OFF: no RsidDetected event may appear.
        let burst = omnimodem_dsp::frontend::rsid::burst_for_mode("mfsk16", 1500.0, 8_000).unwrap();
        let backend = FileBackend::from_samples(to_i16(&burst), 8_000);
        let capture = backend.open_capture(8_000).unwrap();
        let (tele_tx, mut tele_rx) = broadcast::channel(256);
        let worker = RxWorker::spawn_streaming(
            ChannelId(6),
            DeviceId::placeholder(),
            capture,
            Box::new(PeakRecordingDemod { rate: 8_000, peak: Arc::new(Mutex::new(0.0)) }),
            RxTxInterlock::new(),
            broadcast::channel(8).0,
            tele_tx,
            test_metrics(),
            crate::core::AudioGain::default(),
            crate::core::spectrum::SpectrumControl::default(),
            false, // rsid_rx disabled
        );
        worker.join();
        let any_rsid = std::iter::from_fn(|| tele_rx.try_recv().ok())
            .any(|ev| matches!(ev, TelemetryEvent::RsidDetected { .. }));
        assert!(!any_rsid, "RSID event emitted while detector disabled");
    }

    #[test]
    fn rx_worker_emits_spectrum_frames_when_enabled() {
        use crate::core::spectrum::{SpectrumCfg, SpectrumControl};
        use std::f32::consts::TAU;

        // ~1 s of a 1 kHz tone at half scale, 48 kHz; native == capture (no resample).
        let tone: Vec<i16> = (0..48_000)
            .map(|n| ((TAU * 1000.0 * n as f32 / 48_000.0).sin() * 16_384.0) as i16)
            .collect();
        let backend = FileBackend::from_samples(tone, 48_000);
        let capture = backend.open_capture(48_000).unwrap();

        // Enable before spawn: the worker reconciles its tap on the first chunk.
        let spectrum = SpectrumControl::default();
        spectrum.enable(SpectrumCfg {
            bin_count: 64,
            fft_size: 1024,
            rate_hz: 20,
            freq_lo_hz: 0.0,
            freq_hi_hz: 0.0,
        });

        let (tele_tx, mut tele_rx) = broadcast::channel(256);
        let peak = Arc::new(Mutex::new(0.0f32));
        let worker = RxWorker::spawn_streaming(
            ChannelId(7),
            DeviceId::placeholder(),
            capture,
            Box::new(PeakRecordingDemod { rate: 48_000, peak }),
            RxTxInterlock::new(),
            broadcast::channel(8).0,
            tele_tx,
            test_metrics(),
            crate::core::AudioGain::default(),
            spectrum,
            false,
        );
        worker.join();

        let mut lines = Vec::new();
        while let Ok(ev) = tele_rx.try_recv() {
            if let TelemetryEvent::SpectrumFrame { channel, bins, freq_step_hz, timestamp_ns, .. } = ev {
                assert_eq!(channel, ChannelId(7));
                assert_eq!(bins.len(), 64, "expected 64 output bins");
                assert!(freq_step_hz > 0.0);
                assert!(timestamp_ns > 0, "frame should carry a wall-clock timestamp");
                lines.push(bins);
            }
        }
        assert!(!lines.is_empty(), "no SpectrumFrame emitted with spectrum enabled");

        // The 1 kHz tone should dominate the bucket covering ~1 kHz (full passband:
        // step = 24000/64 = 375 Hz, so bucket 2 ≈ 937 Hz, bucket 3 ≈ 1312 Hz).
        let mid = &lines[lines.len() / 2];
        let (peak_idx, _) = mid.iter().enumerate().max_by_key(|(_, &v)| v).unwrap();
        let peak_hz = peak_idx as f32 * (24_000.0 / 64.0);
        assert!((peak_hz - 1000.0).abs() < 500.0, "tone bucket at {peak_hz} Hz, expected ~1 kHz");
    }
}
