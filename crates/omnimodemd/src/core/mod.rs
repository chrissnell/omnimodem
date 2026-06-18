//! The synchronous core. Owns the Supervisor; runs on a plain `std::thread`
//! with no tokio. Drains commands, mutates state, persists, and emits events.
//!
//! Phase 2 adds per-channel audio (capture/playback handles) and PTT drivers,
//! a per-rig RX/TX interlock around each transmit, and a hotplug pump that runs
//! on the command-loop's idle tick (via `recv_timeout`) so `DeviceArrived` /
//! `DeviceDeparted` are emitted and a departed device's handles are evicted —
//! all without a second thread sharing the enumerator.

pub mod command;
pub mod error;
pub mod event;

use crate::audio::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use crate::device::{DeviceEnumerator, HotplugEvent, HotplugWatcher};
use crate::ids::{ChannelId, DeviceId, TransmitId};
use crate::ptt::sequence::{drive_tx_cycle, TxCycleOutcome};
use crate::ptt::{PttDriver, PttError};
use crate::supervisor::Supervisor;
use command::Command;
use error::CoreError;
use event::{FrameEvent, TelemetryEvent};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;
use tokio::sync::broadcast;

/// Bounded depth of the command queue (command-side backpressure).
pub const COMMAND_QUEUE_DEPTH: usize = 64;

/// Capacity of each broadcast ring. Frames get a deeper ring because lag means
/// disconnect (we want headroom before that); telemetry can be shallow.
pub const FRAME_RING: usize = 1024;
pub const TELEMETRY_RING: usize = 256;

/// Idle-tick period: how often the command loop polls for device hotplug when
/// no command is pending.
const HOTPLUG_POLL: Duration = Duration::from_millis(500);

/// Drain-loop poll interval for the no-sleep TX cycle in production.
const TX_POLL: Duration = Duration::from_millis(5);

/// Builds an `AudioBackend` for a resolved device. Production passes a cpal
/// factory; tests pass a file/null factory. Owned by the core thread.
pub type AudioBackendFactory =
    Box<dyn Fn(&crate::device::DeviceDescriptor) -> Box<dyn AudioBackend> + Send>;

/// Handles the async edge keeps to talk to the core.
#[derive(Clone)]
pub struct CoreHandle {
    pub commands: SyncSender<Command>,
    pub frames: broadcast::Sender<FrameEvent>,
    pub telemetry: broadcast::Sender<TelemetryEvent>,
}

/// Spawn the core thread. Returns a handle plus the thread's `JoinHandle`. The
/// `enumerator` and `audio_factory` are injected so the core is testable with
/// fakes; production passes `RealEnumerator` + a cpal-backend factory.
pub fn spawn(
    supervisor: Supervisor,
    enumerator: Box<dyn DeviceEnumerator>,
    audio_factory: AudioBackendFactory,
) -> (CoreHandle, std::thread::JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::sync_channel(COMMAND_QUEUE_DEPTH);
    let (frame_tx, _) = broadcast::channel(FRAME_RING);
    let (tele_tx, _) = broadcast::channel(TELEMETRY_RING);

    let handle = CoreHandle {
        commands: cmd_tx,
        frames: frame_tx.clone(),
        telemetry: tele_tx.clone(),
    };

    let join = std::thread::Builder::new()
        .name("omnimodem-core".into())
        .spawn(move || run(supervisor, enumerator, audio_factory, cmd_rx, frame_tx, tele_tx))
        .expect("spawn core thread");

    (handle, join)
}

/// Per-channel live audio/PTT bindings owned by the core loop. Audio capture
/// handles are held to keep their streams alive even though no DSP consumes
/// them yet (Phase 3).
#[derive(Default)]
struct LiveBindings {
    sinks: HashMap<ChannelId, PlaybackHandle>,
    #[allow(dead_code)]
    captures: HashMap<ChannelId, CaptureHandle>,
    /// Audio device + working rate per channel (the rig the interlock gates).
    audio: HashMap<ChannelId, (DeviceId, u32)>,
    drivers: HashMap<ChannelId, Box<dyn PttDriver>>,
    /// PTT device id per channel (for eviction on hotplug).
    ptt_dev: HashMap<ChannelId, DeviceId>,
}

/// The core loop. Blocks on `recv_timeout`; on a command, handles it; on the
/// idle tick, polls hotplug. Exits on `Shutdown` or a closed channel.
fn run(
    mut supervisor: Supervisor,
    enumerator: Box<dyn DeviceEnumerator>,
    audio_factory: AudioBackendFactory,
    commands: Receiver<Command>,
    _frames: broadcast::Sender<FrameEvent>,
    telemetry: broadcast::Sender<TelemetryEvent>,
) {
    let mut next_tx_id: u64 = 1;
    let mut live = LiveBindings::default();
    let interlock = supervisor.interlock();
    let mut watcher = HotplugWatcher::new();

    loop {
        match commands.recv_timeout(HOTPLUG_POLL) {
            Ok(Command::Shutdown) => break,
            Ok(cmd) => handle_command(
                cmd,
                &mut supervisor,
                &*enumerator,
                &audio_factory,
                &interlock,
                &mut live,
                &mut next_tx_id,
                &telemetry,
            ),
            Err(RecvTimeoutError::Timeout) => {
                poll_hotplug(&mut watcher, &*enumerator, &mut supervisor, &mut live, &telemetry);
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    cmd: Command,
    supervisor: &mut Supervisor,
    enumerator: &dyn DeviceEnumerator,
    audio_factory: &AudioBackendFactory,
    interlock: &crate::ptt::interlock::RxTxInterlock,
    live: &mut LiveBindings,
    next_tx_id: &mut u64,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) {
    match cmd {
        Command::ConfigureChannel { id, name, mode, reply } => {
            let res = supervisor.configure_channel(id, name, mode).map_err(Into::into);
            if res.is_ok() {
                let _ = telemetry.send(TelemetryEvent::ChannelConfigured { channel: id });
            }
            let _ = reply.send(res);
        }

        Command::ConfigureAudio { id, device_id, sample_rate, fanout, reply } => {
            let _ = reply.send(configure_audio(
                supervisor, enumerator, audio_factory, live, id, device_id, sample_rate, fanout,
            ));
        }

        Command::ConfigurePtt { id, ptt, reply } => {
            let _ = reply.send(configure_ptt(supervisor, live, id, ptt));
        }

        Command::KeyPtt { channel, keyed, reply } => {
            let _ = reply.send(key_ptt(supervisor, interlock, live, telemetry, channel, keyed));
        }

        Command::Transmit { channel, payload, reply } => {
            if !supervisor.has_channel(channel) {
                let _ = reply.send(Err(CoreError::UnknownChannel(channel)));
                return;
            }
            let tx_id = TransmitId(*next_tx_id);
            *next_tx_id += 1;
            let res = transmit(supervisor, interlock, live, telemetry, channel, payload, tx_id);
            let _ = reply.send(res);
        }

        Command::ListDevices { reply } => {
            let devices = supervisor.device_cache_mut().refresh(enumerator);
            let _ = reply.send(devices);
        }

        Command::SuggestUdevRule { device_id, reply } => {
            let res = crate::ptt::udev::suggest(&device_id).ok_or_else(|| {
                CoreError::Ptt(PttError::Config(format!(
                    "no udev rule applies to {}",
                    device_id.to_canonical_string()
                )))
            });
            let _ = reply.send(res);
        }

        Command::GetState { reply } => {
            let _ = reply.send(supervisor.snapshot());
        }

        Command::Shutdown => {} // handled in run()
    }
}

#[allow(clippy::too_many_arguments)]
fn configure_audio(
    supervisor: &mut Supervisor,
    enumerator: &dyn DeviceEnumerator,
    audio_factory: &AudioBackendFactory,
    live: &mut LiveBindings,
    id: ChannelId,
    device_id: DeviceId,
    sample_rate: u32,
    fanout: u32,
) -> Result<u32, CoreError> {
    if !supervisor.has_channel(id) {
        return Err(CoreError::UnknownChannel(id));
    }
    supervisor.configure_audio(id, device_id.clone(), sample_rate, fanout)?;

    // Resolve the durable id to a live device (refresh first so a never-listed
    // device still binds).
    supervisor.device_cache_mut().refresh(enumerator);
    let desc = supervisor
        .device_cache_mut()
        .resolve(&device_id)
        .cloned()
        .ok_or_else(|| {
            CoreError::Audio(crate::audio::AudioError::DeviceNotFound(
                device_id.to_canonical_string(),
            ))
        })?;

    let backend = (audio_factory)(&desc);
    let capture = backend.open_capture(sample_rate)?;
    let playback = backend.open_playback(sample_rate)?;
    let actual = playback.sample_rate;

    live.captures.insert(id, capture);
    live.sinks.insert(id, playback);
    live.audio.insert(id, (device_id, actual));
    Ok(actual)
}

fn configure_ptt(
    supervisor: &mut Supervisor,
    live: &mut LiveBindings,
    id: ChannelId,
    ptt: crate::ptt::registry::PttConfig,
) -> Result<(), CoreError> {
    if !supervisor.has_channel(id) {
        return Err(CoreError::UnknownChannel(id));
    }
    supervisor.configure_ptt(id, ptt.clone())?;
    let driver = supervisor.ptt_registry_mut().build_driver(&ptt)?;
    live.drivers.insert(id, driver);
    live.ptt_dev.insert(id, ptt.device_id);
    Ok(())
}

fn key_ptt(
    supervisor: &mut Supervisor,
    interlock: &crate::ptt::interlock::RxTxInterlock,
    live: &mut LiveBindings,
    telemetry: &broadcast::Sender<TelemetryEvent>,
    channel: ChannelId,
    keyed: bool,
) -> Result<(), CoreError> {
    let rig = live
        .audio
        .get(&channel)
        .map(|(d, _)| d.clone())
        .or_else(|| live.ptt_dev.get(&channel).cloned());
    let driver = live
        .drivers
        .get_mut(&channel)
        .ok_or_else(|| CoreError::Ptt(PttError::Config("channel has no PTT configured".into())))?;

    let result = if keyed {
        if let Some(r) = &rig {
            interlock.begin_tx(r);
        }
        driver.key()
    } else {
        let r = driver.unkey();
        if let Some(rig) = &rig {
            interlock.end_tx(rig);
        }
        r
    };

    match result {
        Ok(()) => {
            let _ = telemetry.send(TelemetryEvent::PttKeyed { channel, keyed });
            Ok(())
        }
        Err(e) => {
            if keyed {
                // key() failed: undo the interlock we optimistically took.
                if let Some(r) = &rig {
                    interlock.end_tx(r);
                }
            }
            evict_on_gone(supervisor, live, channel, &e);
            Err(CoreError::Ptt(e))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn transmit(
    supervisor: &mut Supervisor,
    interlock: &crate::ptt::interlock::RxTxInterlock,
    live: &mut LiveBindings,
    telemetry: &broadcast::Sender<TelemetryEvent>,
    channel: ChannelId,
    payload: Vec<u8>,
    tx_id: TransmitId,
) -> Result<TransmitId, CoreError> {
    let _ = telemetry.send(TelemetryEvent::TransmitStarted { channel, transmit_id: tx_id });

    // Real cycle only when both audio and PTT are bound; otherwise fall back to
    // the Phase-1 simulation (announce start/complete, ignore payload).
    let have_audio = live.sinks.contains_key(&channel) && live.audio.contains_key(&channel);
    let have_ptt = live.drivers.contains_key(&channel);

    let outcome = if have_audio && have_ptt {
        let (rig, rate) = live.audio.get(&channel).cloned().unwrap();
        let samples: Vec<i16> = payload
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]))
            .collect();
        let sink = live.sinks.get(&channel).unwrap();
        let driver = live.drivers.get_mut(&channel).unwrap();

        interlock.begin_tx(&rig);
        let _ = telemetry.send(TelemetryEvent::PttKeyed { channel, keyed: true });
        let outcome = drive_tx_cycle(driver.as_mut(), sink, samples, rate, TX_POLL);
        let _ = telemetry.send(TelemetryEvent::PttKeyed { channel, keyed: false });
        interlock.end_tx(&rig);
        Some(outcome)
    } else {
        None // simulation
    };

    let _ = telemetry.send(TelemetryEvent::TransmitComplete { channel, transmit_id: tx_id });

    match outcome {
        None | Some(TxCycleOutcome::Done) => Ok(tx_id),
        Some(TxCycleOutcome::KeyFailed(e))
        | Some(TxCycleOutcome::SubmitFailed(e))
        | Some(TxCycleOutcome::UnkeyFailed(e)) => {
            evict_on_gone(supervisor, live, channel, &e);
            Err(CoreError::Ptt(e))
        }
    }
}

/// On `DeviceGone`, drop the channel's PTT driver and evict its identity from
/// the registry so the next configure re-opens from scratch.
fn evict_on_gone(
    supervisor: &mut Supervisor,
    live: &mut LiveBindings,
    channel: ChannelId,
    e: &PttError,
) {
    if matches!(e, PttError::DeviceGone { .. }) {
        live.drivers.remove(&channel);
        if let Some(id) = live.ptt_dev.remove(&channel) {
            supervisor.ptt_registry_mut().evict(&id);
        }
    }
}

/// Poll for hotplug changes and react: emit telemetry, and on departure evict
/// the PTT identity and drop any channel handles bound to that device.
fn poll_hotplug(
    watcher: &mut HotplugWatcher,
    enumerator: &dyn DeviceEnumerator,
    supervisor: &mut Supervisor,
    live: &mut LiveBindings,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) {
    for ev in watcher.poll(enumerator) {
        match ev {
            HotplugEvent::Arrived(desc) => {
                let _ = telemetry.send(TelemetryEvent::DeviceArrived {
                    device_id: desc.id,
                    label: desc.label,
                });
            }
            HotplugEvent::Departed(id) => {
                let _ = telemetry.send(TelemetryEvent::DeviceDeparted { device_id: id.clone() });
                supervisor.ptt_registry_mut().evict(&id);
                // Drop audio handles for channels bound to this device.
                let audio_chans: Vec<ChannelId> = live
                    .audio
                    .iter()
                    .filter(|(_, (d, _))| d == &id)
                    .map(|(c, _)| *c)
                    .collect();
                for c in audio_chans {
                    live.sinks.remove(&c);
                    live.captures.remove(&c);
                    live.audio.remove(&c);
                }
                let ptt_chans: Vec<ChannelId> = live
                    .ptt_dev
                    .iter()
                    .filter(|(_, d)| *d == &id)
                    .map(|(c, _)| *c)
                    .collect();
                for c in ptt_chans {
                    live.drivers.remove(&c);
                    live.ptt_dev.remove(&c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::backend::NullBackend;
    use crate::audio::file::FileBackend;
    use crate::core::command::Command;
    use crate::device::enumerate::FakeEnumerator;
    use crate::device::DeviceDescriptor;
    use crate::ids::ChannelId;
    use crate::persist::Store;
    use crate::ptt::registry::{PttConfig, PttMethod, RealOpener};
    use tokio::sync::oneshot;

    fn spawn_core(
        enumerator: Box<dyn DeviceEnumerator>,
        factory: AudioBackendFactory,
    ) -> (CoreHandle, std::thread::JoinHandle<()>) {
        let store = Store::open_in_memory().unwrap();
        let sup = Supervisor::new(store, Box::new(RealOpener)).unwrap();
        spawn(sup, enumerator, factory)
    }

    fn fresh_core() -> (CoreHandle, std::thread::JoinHandle<()>) {
        spawn_core(
            Box::new(FakeEnumerator::new(vec![])),
            Box::new(|_| Box::new(NullBackend::new(48_000))),
        )
    }

    #[test]
    fn configure_then_transmit_emits_events() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (core, join) = fresh_core();
        let mut tele_rx = core.telemetry.subscribe();

        rt.block_on(async {
            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::ConfigureChannel {
                    id: ChannelId(0),
                    name: "vfo-a".into(),
                    mode: "none".into(),
                    reply: tx,
                })
                .unwrap();
            rx.await.unwrap().unwrap();

            match tele_rx.recv().await.unwrap() {
                TelemetryEvent::ChannelConfigured { channel } => assert_eq!(channel, ChannelId(0)),
                other => panic!("expected ChannelConfigured, got {other:?}"),
            }

            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::Transmit {
                    channel: ChannelId(0),
                    payload: vec![1, 2, 3],
                    reply: tx,
                })
                .unwrap();
            let tx_id = rx.await.unwrap().unwrap();
            assert_eq!(tx_id, TransmitId(1));

            match tele_rx.recv().await.unwrap() {
                TelemetryEvent::TransmitStarted { channel, transmit_id } => {
                    assert_eq!(channel, ChannelId(0));
                    assert_eq!(transmit_id, TransmitId(1));
                }
                other => panic!("expected TransmitStarted, got {other:?}"),
            }
            match tele_rx.recv().await.unwrap() {
                TelemetryEvent::TransmitComplete { transmit_id, .. } => {
                    assert_eq!(transmit_id, TransmitId(1));
                }
                other => panic!("expected TransmitComplete, got {other:?}"),
            }
        });

        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    #[test]
    fn transmit_on_unknown_channel_errors() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (core, join) = fresh_core();
        rt.block_on(async {
            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::Transmit { channel: ChannelId(9), payload: vec![], reply: tx })
                .unwrap();
            let err = rx.await.unwrap().unwrap_err();
            assert!(matches!(err, CoreError::UnknownChannel(ChannelId(9))));
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    #[test]
    fn configured_audio_ptt_transmit_runs_real_cycle() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let dev = DeviceDescriptor {
            id: DeviceId::AlsaCard { card_name: "loop".into() },
            label: "loop".into(),
            has_capture: true,
            has_playback: true,
        };
        let dev_id = dev.id.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(|_| Box::new(FileBackend::from_samples(vec![], 48_000))),
        );
        let mut tele_rx = core.telemetry.subscribe();

        rt.block_on(async {
            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::ConfigureChannel {
                    id: ChannelId(0),
                    name: "vfo-a".into(),
                    mode: "none".into(),
                    reply: tx,
                })
                .unwrap();
            rx.await.unwrap().unwrap();

            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::ConfigureAudio {
                    id: ChannelId(0),
                    device_id: dev_id.clone(),
                    sample_rate: 48_000,
                    fanout: 1,
                    reply: tx,
                })
                .unwrap();
            assert_eq!(rx.await.unwrap().unwrap(), 48_000);

            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::ConfigurePtt {
                    id: ChannelId(0),
                    ptt: PttConfig {
                        device_id: dev_id.clone(),
                        method: PttMethod::None,
                        invert: false,
                    },
                    reply: tx,
                })
                .unwrap();
            rx.await.unwrap().unwrap();

            // 480 i16 samples => 960 LE bytes.
            let pcm: Vec<u8> = (0..480i16).flat_map(|i| i.to_le_bytes()).collect();
            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::Transmit { channel: ChannelId(0), payload: pcm, reply: tx })
                .unwrap();
            rx.await.unwrap().unwrap();

            // Observe keyed -> unkeyed and started/complete around the cycle.
            let (mut keyed, mut unkeyed, mut started, mut completed) = (false, false, false, false);
            while !(keyed && unkeyed && started && completed) {
                match tele_rx.recv().await.unwrap() {
                    TelemetryEvent::PttKeyed { keyed: true, .. } => keyed = true,
                    TelemetryEvent::PttKeyed { keyed: false, .. } => unkeyed = true,
                    TelemetryEvent::TransmitStarted { .. } => started = true,
                    TelemetryEvent::TransmitComplete { .. } => completed = true,
                    _ => {}
                }
            }
        });

        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }
}
