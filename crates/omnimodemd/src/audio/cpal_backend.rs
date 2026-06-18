//! cpal audio backend. The only audio code that calls cpal. Decision logic
//! (rate/format) lives in `super::alsa`; identity in `super::enumerate`.

use super::alsa::{choose_stream_rate, pick_input_sample_format, SampleFmt};
use super::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use super::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH};
use crate::ids::DeviceId;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Exponential rebuild backoff after a stream error (Graywolf `REBUILD_BACKOFF`).
const REBUILD_BACKOFF: &[Duration] = &[
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];
/// Clear the backoff after a stream that ran stable this long.
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(60);

/// A cpal device wrapped with its resolved durable identity.
pub struct CpalBackend {
    device: cpal::Device,
    id: DeviceId,
}

impl CpalBackend {
    pub fn new(device: cpal::Device, id: DeviceId) -> Self {
        CpalBackend { device, id }
    }

    /// Collect the (format, min, max) tuples cpal advertises for input.
    fn input_configs(&self) -> Vec<(SampleFmt, u32, u32)> {
        let Ok(ranges) = self.device.supported_input_configs() else {
            return Vec::new();
        };
        ranges
            .filter_map(|r| {
                let fmt = match r.sample_format() {
                    cpal::SampleFormat::I16 => SampleFmt::I16,
                    cpal::SampleFormat::F32 => SampleFmt::F32,
                    cpal::SampleFormat::U16 => SampleFmt::U16,
                    _ => return None,
                };
                Some((fmt, r.min_sample_rate(), r.max_sample_rate()))
            })
            .collect()
    }

    fn input_rate_ranges(&self) -> Vec<(u32, u32)> {
        self.input_configs().iter().map(|&(_, lo, hi)| (lo, hi)).collect()
    }
}

impl AudioBackend for CpalBackend {
    fn open_capture(&self, requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        let configs = self.input_configs();
        if configs.is_empty() {
            return Err(AudioError::NoUsableFormat { device: self.id.to_canonical_string() });
        }
        let rate = choose_stream_rate(requested_rate, &self.input_rate_ranges())?;
        let fmt = pick_input_sample_format(&configs, rate)
            .ok_or_else(|| AudioError::NoUsableFormat { device: self.id.to_canonical_string() })?;

        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let device = self.device.clone();
        let cfg = cpal::StreamConfig {
            channels: 1,
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };

        // Held thread owns the stream and rebuilds it on error with backoff.
        std::thread::Builder::new()
            .name("omni-capture".into())
            .spawn(move || {
                let mut backoff = 0usize;
                while !stop_thread.load(Ordering::Relaxed) {
                    let failed = Arc::new(AtomicBool::new(false));
                    let f2 = failed.clone();
                    let tx2 = tx.clone();
                    let err_fn = move |_e| f2.store(true, Ordering::Relaxed);
                    let built = build_input(&device, &cfg, fmt, tx2, err_fn);
                    let stream = match built {
                        Ok(s) => s,
                        Err(_) => {
                            backoff_wait(&mut backoff, &stop_thread);
                            continue;
                        }
                    };
                    if stream.play().is_err() {
                        backoff_wait(&mut backoff, &stop_thread);
                        continue;
                    }
                    let started = Instant::now();
                    while !stop_thread.load(Ordering::Relaxed)
                        && !failed.load(Ordering::Relaxed)
                    {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    drop(stream);
                    if started.elapsed() >= BACKOFF_RESET_AFTER {
                        backoff = 0;
                    }
                    if !stop_thread.load(Ordering::Relaxed) {
                        backoff_wait(&mut backoff, &stop_thread);
                    }
                }
            })
            .map_err(|e| AudioError::Io(e.to_string()))?;

        let stop_on_drop = stop;
        Ok(CaptureHandle::new(rx, rate, move || {
            stop_on_drop.store(true, Ordering::Relaxed)
        }))
    }

    fn open_playback(&self, requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        let rate = choose_stream_rate(requested_rate, &output_rate_ranges(&self.device))?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let submitted = Arc::new(AtomicUsize::new(0));
        let drained = Arc::new(AtomicUsize::new(0));
        let queue: Arc<Mutex<std::collections::VecDeque<i16>>> =
            Arc::new(Mutex::new(std::collections::VecDeque::new()));

        // Feeder: move submitted buffers into the shared queue.
        let qf = queue.clone();
        std::thread::spawn(move || {
            while let Ok(buf) = rx.recv() {
                qf.lock().unwrap().extend(buf);
            }
        });

        let device = self.device.clone();
        let cfg = cpal::StreamConfig {
            channels: 1,
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let qcb = queue.clone();
        let drained_cb = drained.clone();
        std::thread::Builder::new()
            .name("omni-playback".into())
            .spawn(move || {
                // Build once; on error rebuild with backoff (same pattern).
                let mut backoff = 0usize;
                let never_stop = Arc::new(AtomicBool::new(false));
                loop {
                    let failed = Arc::new(AtomicBool::new(false));
                    let f2 = failed.clone();
                    let q = qcb.clone();
                    let d = drained_cb.clone();
                    let stream = device.build_output_stream(
                        &cfg,
                        move |out: &mut [i16], _| {
                            let mut ql = q.lock().unwrap();
                            for s in out.iter_mut() {
                                // Count every sample actually pulled from the
                                // queue (not what's left): this is the drain
                                // watermark the no-sleep TX cycle waits on.
                                match ql.pop_front() {
                                    Some(v) => {
                                        *s = v;
                                        d.fetch_add(1, Ordering::Relaxed);
                                    }
                                    None => *s = 0, // underrun: emit silence
                                }
                            }
                        },
                        move |_e| f2.store(true, Ordering::Relaxed),
                        None,
                    );
                    let Ok(stream) = stream else {
                        backoff_wait(&mut backoff, &never_stop);
                        continue;
                    };
                    if stream.play().is_err() {
                        backoff_wait(&mut backoff, &never_stop);
                        continue;
                    }
                    while !failed.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    drop(stream);
                    backoff_wait(&mut backoff, &never_stop);
                }
            })
            .map_err(|e| AudioError::Io(e.to_string()))?;

        Ok(PlaybackHandle::new(tx, submitted, drained, rate))
    }

    fn device_id(&self) -> DeviceId {
        self.id.clone()
    }
}

/// Sleep one backoff step, waking every 100 ms to honor `stop`.
fn backoff_wait(idx: &mut usize, stop: &AtomicBool) {
    let dur = REBUILD_BACKOFF[(*idx).min(REBUILD_BACKOFF.len() - 1)];
    let mut waited = Duration::ZERO;
    while waited < dur && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
        waited += Duration::from_millis(100);
    }
    *idx = (*idx + 1).min(REBUILD_BACKOFF.len() - 1);
}

/// Build a mono input stream in `fmt`, converting each callback buffer to i16.
fn build_input(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    fmt: SampleFmt,
    tx: std::sync::mpsc::SyncSender<AudioChunk>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    match fmt {
        SampleFmt::I16 => device.build_input_stream(
            cfg,
            move |data: &[i16], _| {
                let _ = tx.try_send(data.to_vec());
            },
            err_fn,
            None,
        ),
        SampleFmt::F32 => device.build_input_stream(
            cfg,
            move |data: &[f32], _| {
                let chunk = data
                    .iter()
                    .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();
                let _ = tx.try_send(chunk);
            },
            err_fn,
            None,
        ),
        SampleFmt::U16 => device.build_input_stream(
            cfg,
            move |data: &[u16], _| {
                let chunk = data.iter().map(|&s| (s as i32 - 32768) as i16).collect();
                let _ = tx.try_send(chunk);
            },
            err_fn,
            None,
        ),
    }
}

fn output_rate_ranges(device: &cpal::Device) -> Vec<(u32, u32)> {
    device
        .supported_output_configs()
        .map(|it| it.map(|r| (r.min_sample_rate(), r.max_sample_rate())).collect())
        .unwrap_or_default()
}

/// Enumerate the default host's devices as `(DeviceId, CpalBackend)`. The cpal
/// device name (an ALSA pcm id on Linux) yields an `AlsaCard` identity; the USB
/// attach step in `super::enumerate` upgrades it to a `Usb`/`Topology` id when
/// it can match the card to a USB device.
pub fn enumerate_default_host() -> Vec<(DeviceId, CpalBackend)> {
    let host = cpal::default_host();
    let mut out = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for dev in devs {
            #[allow(deprecated)]
            let name = dev.name().unwrap_or_default();
            let id = id_for_name(&name);
            out.push((id.clone(), CpalBackend::new(dev, id)));
        }
    }
    out
}

/// Map a cpal device name to a durable identity. An ALSA pcm id with a
/// `CARD=<name>` token yields an `AlsaCard`; anything else falls back to a
/// `Placeholder` carrying the raw name.
fn id_for_name(name: &str) -> DeviceId {
    super::alsa::alsa_card_token(name)
        .map(|c| DeviceId::AlsaCard { card_name: c.to_string() })
        .unwrap_or_else(|| DeviceId::Placeholder { tag: name.to_string() })
}
