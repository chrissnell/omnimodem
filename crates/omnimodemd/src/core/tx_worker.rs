//! Per-channel TX worker. A cooperative queue serializes frames from any client
//! onto one channel's on-air timeline; the worker modulates each frame to
//! samples and runs the no-sleep `drive_tx_cycle`. Windowed modes wait for the
//! next time-slot boundary before keying (FT8's 15 s grid). Per-rig
//! serialization is enforced by the shared PTT registry/interlock at the core
//! (two channels on one rig still serialize). This replaces Graywolf's single
//! global TX worker, which needlessly serialized TX across independent radios.

use crate::audio::backend::PlaybackHandle;
use crate::core::clock::SlotClock;
use crate::core::event::TelemetryEvent;
use crate::ids::{ChannelId, DeviceId, TransmitId};
use crate::ptt::interlock::RxTxInterlock;
use crate::ptt::lease::TxLeaseRegistry;
use crate::ptt::sequence::{drive_tx_cycle, TxCycleOutcome};
use crate::ptt::PttDriver;
use omnimodem_dsp::frontend::resample::Resampler;
use omnimodem_dsp::mode::Modulator;
use omnimodem_dsp::types::{Frame, FramePayload};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::broadcast;

/// Cooperative queue depth per channel. Frames beyond this are rejected with a
/// "queue full" error (the caller can retry), applying natural backpressure.
pub const TX_QUEUE_DEPTH: usize = 32;

/// Drain-loop poll interval for the no-sleep TX cycle in production.
const TX_POLL: Duration = Duration::from_millis(5);

/// A queued TX job: the frame to send and its transmit id (for events).
#[derive(Debug)]
pub struct TxJob {
    pub frame: Frame,
    pub transmit_id: TransmitId,
}

/// Everything the worker thread owns for one channel.
pub struct TxWorkerCfg {
    pub channel: ChannelId,
    pub rig: DeviceId,
    pub rate: u32,
    pub modulator: Box<dyn Modulator>,
    pub sink: PlaybackHandle,
    pub driver: Box<dyn PttDriver>,
    pub interlock: RxTxInterlock,
    /// Per-rig exclusive TX lease. While another channel holds the rig's lease,
    /// this worker's jobs complete without keying.
    pub lease: TxLeaseRegistry,
    pub telemetry: broadcast::Sender<TelemetryEvent>,
    /// `Some(slot_s)` for windowed modes (align to the slot boundary).
    pub slot_s: Option<f32>,
    /// Runtime TX output gain (linear multiplier, 1.0 == unity).
    pub gain: crate::core::AudioGain,
}

/// Handle to a running TX worker.
pub struct TxWorker {
    queue: SyncSender<TxJob>,
    /// Set to drop any not-yet-started jobs and stop the worker promptly.
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl TxWorker {
    /// Enqueue a frame for transmission. Returns the job back on a full or
    /// closed queue so the caller can surface an error.
    pub fn enqueue(&self, job: TxJob) -> Result<(), TxJob> {
        self.queue.try_send(job).map_err(|e| match e {
            std::sync::mpsc::TrySendError::Full(j)
            | std::sync::mpsc::TrySendError::Disconnected(j) => j,
        })
    }

    /// Stop the worker and wait for it (graceful shutdown / tests). May block up
    /// to the in-flight cycle's airtime; the core uses `Drop` instead, which
    /// does not block.
    pub fn shutdown(mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.close_queue();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }

    /// Drop our queue sender so the worker's `recv` ends, without UB.
    fn close_queue(&mut self) {
        let (dead, _) = std::sync::mpsc::sync_channel(1);
        let _ = std::mem::replace(&mut self.queue, dead);
    }
}

impl Drop for TxWorker {
    fn drop(&mut self) {
        // Signal cancel and close the queue, but DETACH (no join): the core
        // thread calls `remove()` to evict a worker and must never block on the
        // worker's in-flight transmit (which can be a full FT8 slot of airtime).
        // The worker finishes at most its current cycle, then exits on its own.
        self.cancel.store(true, Ordering::Relaxed);
        self.close_queue();
    }
}

/// Spawn a TX worker thread for one channel.
pub fn spawn(cfg: TxWorkerCfg) -> TxWorker {
    let (tx, rx) = std::sync::mpsc::sync_channel(TX_QUEUE_DEPTH);
    let cancel = Arc::new(AtomicBool::new(false));
    let join = std::thread::Builder::new()
        .name(format!("omnimodem-tx-{}", cfg.channel.0))
        .spawn({
            let cancel = cancel.clone();
            move || run(cfg, rx, cancel)
        })
        .expect("spawn tx worker");
    TxWorker { queue: tx, cancel, join: Some(join) }
}

fn run(mut cfg: TxWorkerCfg, rx: Receiver<TxJob>, cancel: Arc<AtomicBool>) {
    while let Ok(job) = rx.recv() {
        // Drop pending jobs promptly once cancelled (e.g. the rig departed).
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let samples = match cfg.modulator.modulate(&job.frame) {
            Ok(s) => s,
            Err(_) => {
                // Payload not valid for this mode: surface start+complete so a
                // client awaiting this transmit id isn't left hanging.
                let _ = cfg.telemetry.send(TelemetryEvent::TransmitStarted {
                    channel: cfg.channel,
                    transmit_id: job.transmit_id,
                });
                let _ = cfg.telemetry.send(TelemetryEvent::TransmitComplete {
                    channel: cfg.channel,
                    transmit_id: job.transmit_id,
                });
                continue;
            }
        };
        // Bridge the modulator's native rate to the playback stream rate. A
        // modulator emits baseband at caps().native_rate (CW/RTTY/PSK31 at 8 kHz,
        // FT8 at 12 kHz); feeding those straight to a 48 kHz sink ran the burst
        // several times too fast and high-pitched. afsk1200 is already 48 kHz, so
        // its resampler is a passthrough — which is why only it sounded right.
        let native = cfg.modulator.caps().native_rate;
        let samples = if cfg.rate > 0 && native != cfg.rate {
            Resampler::new(native, cfg.rate, 16).process(&samples)
        } else {
            samples
        };
        // Apply runtime TX gain before the i16 conversion; the clamp stays after
        // the multiply so boosting can hit the rails but never overflow i16.
        let g = cfg.gain.tx();
        let pcm: Vec<i16> =
            samples.iter().map(|&s| ((s * g).clamp(-1.0, 1.0) * 32767.0) as i16).collect();

        // Exclusive TX lease: if another channel holds this rig, drop the job
        // without keying. Checked up front to avoid a pointless slot-wait, then
        // RE-checked after the wait (a windowed slot can be ~minutes, during
        // which another channel could acquire the lease — closes the TOCTOU).
        let lease_blocked = |worker: &TxWorkerCfg| !worker.lease.may_transmit(&worker.rig, worker.channel);
        if lease_blocked(&cfg) {
            let _ = cfg.telemetry.send(TelemetryEvent::TransmitStarted {
                channel: cfg.channel,
                transmit_id: job.transmit_id,
            });
            let _ = cfg.telemetry.send(TelemetryEvent::TransmitComplete {
                channel: cfg.channel,
                transmit_id: job.transmit_id,
            });
            continue;
        }

        // Windowed modes wait for the next slot boundary before keying.
        if let Some(slot) = cfg.slot_s {
            let delay = SlotClock::new(slot).delay_until_next();
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
        }

        // Re-check the lease after the slot wait, immediately before keying.
        if lease_blocked(&cfg) {
            let _ = cfg.telemetry.send(TelemetryEvent::TransmitStarted {
                channel: cfg.channel,
                transmit_id: job.transmit_id,
            });
            let _ = cfg.telemetry.send(TelemetryEvent::TransmitComplete {
                channel: cfg.channel,
                transmit_id: job.transmit_id,
            });
            continue;
        }

        let _ = cfg.telemetry.send(TelemetryEvent::TransmitStarted {
            channel: cfg.channel,
            transmit_id: job.transmit_id,
        });
        cfg.interlock.begin_tx(&cfg.rig);
        let _ = cfg.telemetry.send(TelemetryEvent::PttKeyed { channel: cfg.channel, keyed: true });
        let outcome = drive_tx_cycle(cfg.driver.as_mut(), &cfg.sink, pcm, cfg.rate, TX_POLL);
        let _ = cfg.telemetry.send(TelemetryEvent::PttKeyed { channel: cfg.channel, keyed: false });
        cfg.interlock.end_tx(&cfg.rig);
        let _ = cfg.telemetry.send(TelemetryEvent::TransmitComplete {
            channel: cfg.channel,
            transmit_id: job.transmit_id,
        });

        if !matches!(outcome, TxCycleOutcome::Done) {
            // PTT error: stop the worker; the core evicts on the next command.
            break;
        }
    }
}

/// Interpret opaque transmit-payload bytes into a `Frame` for `mode`. Text modes
/// (FT8/CW/RTTY/PSK31) take UTF-8 text; AFSK takes raw AX.25 frame bytes.
pub fn payload_to_frame(mode: &crate::mode::ModeConfig, payload: Vec<u8>) -> Frame {
    use crate::mode::ModeConfig;
    match mode {
        ModeConfig::Afsk1200 { .. } => {
            Frame { payload: FramePayload::Packet(payload), meta: Default::default() }
        }
        _ => Frame {
            payload: FramePayload::Text(String::from_utf8_lossy(&payload).to_string()),
            meta: Default::default(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::backend::AudioBackend;
    use crate::audio::file::FileBackend;
    use crate::mode::ModeConfig;
    use crate::ptt::none::MockPtt;
    use omnimodem_dsp::types::Frame as DspFrame;

    #[test]
    fn worker_modulates_and_plays_a_queued_text_frame() {
        let backend = FileBackend::from_samples(vec![], 8_000);
        let sink = backend.open_playback(8_000).unwrap();
        let (tele, mut tele_rx) = broadcast::channel(64);
        let worker = spawn(TxWorkerCfg {
            channel: ChannelId(0),
            rig: DeviceId::placeholder(),
            rate: 8_000,
            modulator: crate::mode::registry::build_modulator(&ModeConfig::Psk31 {
                center_hz: 1000.0,
            })
            .unwrap(),
            sink,
            driver: Box::new(MockPtt::new()),
            interlock: RxTxInterlock::new(),
            lease: TxLeaseRegistry::new(),
            telemetry: tele,
            slot_s: None,
            gain: crate::core::AudioGain::default(),
        });
        worker.enqueue(TxJob { frame: DspFrame::text("CQ"), transmit_id: TransmitId(1) }).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            // PSK31 "CQ" with its idle preamble is ~2 s of airtime; the no-sleep
            // cycle waits the full duration before completing, so poll well past
            // that.
            let mut completed = false;
            for _ in 0..400 {
                while let Ok(ev) = tele_rx.try_recv() {
                    if let TelemetryEvent::TransmitComplete { transmit_id, .. } = ev {
                        assert_eq!(transmit_id, TransmitId(1));
                        completed = true;
                    }
                }
                if completed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(completed, "no TransmitComplete");
        });
        assert!(!backend.played.lock().unwrap().is_empty(), "no audio played");
        worker.shutdown();
    }

    #[test]
    fn modulator_output_is_resampled_to_the_sink_rate() {
        // CW modulates at 8 kHz; through a 48 kHz sink it must be resampled up
        // ~6x. Playing it raw (1x) is what made CW/PSK31/RTTY sound several times
        // too fast and high-pitched.
        let mk = || {
            crate::mode::registry::build_modulator(&ModeConfig::Cw { wpm: 20, tone_hz: 700.0 })
                .unwrap()
        };
        assert_eq!(mk().caps().native_rate, 8_000);
        let raw_len = mk().modulate(&DspFrame::text("E")).unwrap().len();

        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let (tele, mut tele_rx) = broadcast::channel(64);
        let worker = spawn(TxWorkerCfg {
            channel: ChannelId(0),
            rig: DeviceId::placeholder(),
            rate: 48_000,
            modulator: mk(),
            sink,
            driver: Box::new(MockPtt::new()),
            interlock: RxTxInterlock::new(),
            lease: TxLeaseRegistry::new(),
            telemetry: tele,
            slot_s: None,
            gain: crate::core::AudioGain::default(),
        });
        worker.enqueue(TxJob { frame: DspFrame::text("E"), transmit_id: TransmitId(1) }).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            let mut completed = false;
            for _ in 0..400 {
                while let Ok(ev) = tele_rx.try_recv() {
                    if let TelemetryEvent::TransmitComplete { .. } = ev {
                        completed = true;
                    }
                }
                if completed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(completed, "no TransmitComplete");
        });

        // Played at 48 kHz from an 8 kHz native burst => ~6x the native samples.
        // Without the resample step it would be ~raw_len (1x).
        let played = backend.played.lock().unwrap().len();
        assert!(
            played >= raw_len * 5,
            "expected ~6x resample of {raw_len} native samples, played {played}",
        );
        worker.shutdown();
    }

    #[test]
    fn tx_gain_scales_played_amplitude() {
        use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};

        let ax = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("K1ABC", 1),
            digipeaters: vec![],
            info: b"g".to_vec(),
        };
        let bytes = ax.encode();
        // Reference unity-gain peak straight from the modulator (same samples the
        // worker will produce, before the 0.5x scaling).
        let raw = crate::mode::registry::build_modulator(&ModeConfig::Afsk1200 { tx: true })
            .unwrap()
            .modulate(&DspFrame::packet(bytes.clone()))
            .unwrap();
        let unity_peak = raw.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(unity_peak > 0.0);

        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let (tele, mut tele_rx) = broadcast::channel(64);
        let gain = crate::core::AudioGain::default();
        gain.set(1.0, 0.5); // halve TX output
        let worker = spawn(TxWorkerCfg {
            channel: ChannelId(0),
            rig: DeviceId::placeholder(),
            rate: 48_000,
            modulator: crate::mode::registry::build_modulator(&ModeConfig::Afsk1200 { tx: true })
                .unwrap(),
            sink,
            driver: Box::new(MockPtt::new()),
            interlock: RxTxInterlock::new(),
            lease: TxLeaseRegistry::new(),
            telemetry: tele,
            slot_s: None,
            gain: gain.clone(),
        });
        worker.enqueue(TxJob { frame: DspFrame::packet(bytes), transmit_id: TransmitId(1) }).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            let mut completed = false;
            for _ in 0..400 {
                while let Ok(ev) = tele_rx.try_recv() {
                    if let TelemetryEvent::TransmitComplete { .. } = ev {
                        completed = true;
                    }
                }
                if completed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(completed, "no TransmitComplete");
        });

        let played = backend.played.lock().unwrap();
        let played_peak = played.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0) as f32;
        let expected = 0.5 * unity_peak * 32767.0;
        // Within 10% + small abs slack for i16 rounding.
        assert!(
            (played_peak - expected).abs() <= expected * 0.10 + 64.0,
            "played_peak {played_peak} not ~= {expected} (half of unity {})",
            unity_peak * 32767.0
        );
        worker.shutdown();
    }

    #[test]
    fn worker_skips_tx_when_another_channel_holds_the_lease() {
        // A DIFFERENT channel holds the rig's exclusive lease, so this worker's
        // job must complete WITHOUT ever keying PTT.
        let backend = FileBackend::from_samples(vec![], 8_000);
        let sink = backend.open_playback(8_000).unwrap();
        let rig = DeviceId::placeholder();
        let lease = TxLeaseRegistry::new();
        lease.acquire(&rig, ChannelId(99)).unwrap(); // held by someone else

        let (tele, mut tele_rx) = broadcast::channel(64);
        let worker = spawn(TxWorkerCfg {
            channel: ChannelId(0),
            rig: rig.clone(),
            rate: 8_000,
            modulator: crate::mode::registry::build_modulator(&ModeConfig::Psk31 {
                center_hz: 1000.0,
            })
            .unwrap(),
            sink,
            driver: Box::new(MockPtt::new()),
            interlock: RxTxInterlock::new(),
            lease,
            telemetry: tele,
            slot_s: None,
            gain: crate::core::AudioGain::default(),
        });
        worker.enqueue(TxJob { frame: DspFrame::text("CQ"), transmit_id: TransmitId(1) }).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            let (mut completed, mut keyed) = (false, false);
            for _ in 0..50 {
                while let Ok(ev) = tele_rx.try_recv() {
                    match ev {
                        TelemetryEvent::TransmitComplete { transmit_id, .. } => {
                            assert_eq!(transmit_id, TransmitId(1));
                            completed = true;
                        }
                        TelemetryEvent::PttKeyed { keyed: true, .. } => keyed = true,
                        _ => {}
                    }
                }
                if completed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(completed, "lease-blocked job never completed");
            assert!(!keyed, "lease-blocked job keyed PTT anyway");
        });
        assert!(backend.played.lock().unwrap().is_empty(), "lease-blocked job played audio");
        worker.shutdown();
    }

    #[test]
    fn payload_to_frame_routes_by_mode() {
        let f = payload_to_frame(&ModeConfig::Afsk1200 { tx: true }, vec![1, 2, 3]);
        assert!(matches!(f.payload, FramePayload::Packet(b) if b == vec![1, 2, 3]));
        let f = payload_to_frame(&ModeConfig::Psk31 { center_hz: 1000.0 }, b"CQ".to_vec());
        assert!(matches!(f.payload, FramePayload::Text(t) if t == "CQ"));
    }
}
