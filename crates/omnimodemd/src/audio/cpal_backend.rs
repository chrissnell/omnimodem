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

/// A cpal device pair wrapped with its resolved durable identity. Capture and
/// playback are distinct cpal devices on CoreAudio/ALSA, so a duplex identity
/// carries both; a capture-only (mic) or playback-only (speaker) device carries
/// just the side it supports.
pub struct CpalBackend {
    input: Option<cpal::Device>,
    output: Option<cpal::Device>,
    id: DeviceId,
}

impl CpalBackend {
    /// True when this identity can capture (has an input device).
    pub fn has_capture(&self) -> bool {
        self.input.is_some()
    }

    /// True when this identity can play back (has an output device).
    pub fn has_playback(&self) -> bool {
        self.output.is_some()
    }
}

/// Collect the (format, min, max) tuples cpal advertises for input on `device`.
fn input_configs(device: &cpal::Device) -> Vec<(SampleFmt, u32, u32)> {
    let Ok(ranges) = device.supported_input_configs() else {
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

fn input_rate_ranges(device: &cpal::Device) -> Vec<(u32, u32)> {
    input_configs(device).iter().map(|&(_, lo, hi)| (lo, hi)).collect()
}

impl AudioBackend for CpalBackend {
    fn open_capture(&self, requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        let src = self.input.as_ref().ok_or_else(|| AudioError::NoUsableFormat {
            device: self.id.to_canonical_string(),
        })?;
        let configs = input_configs(src);
        if configs.is_empty() {
            return Err(AudioError::NoUsableFormat { device: self.id.to_canonical_string() });
        }
        let rate = choose_stream_rate(requested_rate, &input_rate_ranges(src))?;
        let fmt = pick_input_sample_format(&configs, rate)
            .ok_or_else(|| AudioError::NoUsableFormat { device: self.id.to_canonical_string() })?;

        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let device = src.clone();
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
        // No output side (an input-only mic, or TX defaulting to the capture
        // device) means this identity can't play back. Report that plainly
        // rather than letting choose_stream_rate's empty case surface as a
        // confusing "rate exceeds ceiling".
        let sink = self.output.as_ref().ok_or_else(|| AudioError::NoUsableFormat {
            device: self.id.to_canonical_string(),
        })?;
        let ranges = output_rate_ranges(sink);
        if ranges.is_empty() {
            return Err(AudioError::NoUsableFormat { device: self.id.to_canonical_string() });
        }
        let rate = choose_stream_rate(requested_rate, &ranges)?;
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

        let device = sink.clone();
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

        // Flush drops every sample still queued for the DAC callback, so an
        // aborted transmission (e.g. a mode change mid-burst) stops keying the
        // air within a callback period instead of playing the whole buffer out.
        // A whole burst is submitted at once and the feeder moves it into the
        // queue near-instantly, so by the time an abort fires (seconds into the
        // burst) it is long queued; the sub-millisecond window where a just-
        // submitted buffer sits in the channel un-queued is not reachable here.
        let qflush = queue.clone();
        let flush = Arc::new(move || qflush.lock().unwrap().clear());
        Ok(PlaybackHandle::new(tx, submitted, drained, rate, flush))
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

/// Enumerate the default host's devices as `(DeviceId, CpalBackend)`. Both the
/// capture (`input_devices`) and playback (`output_devices`) sides are listed
/// and merged by durable identity, so a duplex device becomes one backend that
/// can do both while a mic or a speaker carries only the side it supports —
/// without this, playback was never wired and every channel bound RX-only. The
/// cpal device name (an ALSA pcm id on Linux) yields an `AlsaCard` identity; the
/// USB attach step in `super::enumerate` upgrades it to a `Usb`/`Topology` id.
pub fn enumerate_default_host() -> Vec<(DeviceId, CpalBackend)> {
    let host = cpal::default_host();
    // First-seen order: capture devices, then any playback-only devices.
    let mut order: Vec<DeviceId> = Vec::new();
    let mut pairs: std::collections::HashMap<DeviceId, (Option<cpal::Device>, Option<cpal::Device>)> =
        std::collections::HashMap::new();
    if let Ok(devs) = host.input_devices() {
        for dev in devs {
            #[allow(deprecated)]
            let name = dev.name().unwrap_or_default();
            let id = id_for_name(&name);
            if !pairs.contains_key(&id) {
                order.push(id.clone());
            }
            pairs.entry(id).or_default().0 = Some(dev);
        }
    }
    if let Ok(devs) = host.output_devices() {
        for dev in devs {
            #[allow(deprecated)]
            let name = dev.name().unwrap_or_default();
            let id = id_for_name(&name);
            if !pairs.contains_key(&id) {
                order.push(id.clone());
            }
            pairs.entry(id).or_default().1 = Some(dev);
        }
    }
    order
        .into_iter()
        .map(|id| {
            let (input, output) = pairs.remove(&id).unwrap_or_default();
            (id.clone(), CpalBackend { input, output, id })
        })
        .collect()
}

/// Map a cpal device name to a durable identity. An ALSA pcm id with a
/// `CARD=<name>` token yields an `AlsaCard`; anything else falls back to a
/// `Placeholder` carrying the raw name.
fn id_for_name(name: &str) -> DeviceId {
    super::alsa::alsa_card_token(name)
        .map(|c| DeviceId::AlsaCard { card_name: c.to_string() })
        .unwrap_or_else(|| DeviceId::Placeholder { tag: name.to_string() })
}
