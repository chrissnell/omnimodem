//! The synchronous core. Owns the Supervisor; runs on a plain `std::thread`
//! with no tokio. Drains commands, mutates state, persists, and emits events.
//!
//! Phase 2 adds per-channel audio (capture/playback handles) and PTT drivers,
//! a per-rig RX/TX interlock around each transmit, and a hotplug pump that runs
//! on the command-loop's idle tick (via `recv_timeout`) so `DeviceArrived` /
//! `DeviceDeparted` are emitted and a departed device's handles are evicted —
//! all without a second thread sharing the enumerator.

pub mod adsb;
pub mod clock;
pub mod command;
pub mod error;
pub mod event;
mod gain;
pub mod rx_worker;
pub mod spectrum;
pub mod tx_worker;

pub(crate) use gain::AudioGain;

use crate::audio::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use crate::audio::sdr::{
    plan_tune, snap_gain, supported_sample_rates, DemodMode, SdrControl, ADSB_CAPTURE_RATE,
    ADSB_FREQ_HZ, DIRECT_SAMPLING_MAX_HZ, DIRECT_SAMPLING_Q_BRANCH,
};
use crate::core::clock::ClockSource;
use crate::core::rx_worker::RxWorker;
use crate::core::tx_worker::{TxJob, TxWorker, TxWorkerCfg};
use crate::device::{DeviceEnumerator, HotplugEvent, HotplugWatcher};
use crate::core::rx_worker::SharedMetrics;
use crate::ids::{ChannelId, DeviceId, TransmitId};
use crate::metrics::{ChannelMetrics, ChannelMetricsSnapshot};
use crate::mode::registry::{self, DemodKind};
use crate::ptt::lease::TxLeaseRegistry;
use crate::ptt::sequence::{drive_tx_cycle, TxCycleOutcome};
use crate::ptt::{PttDriver, PttError};
use crate::supervisor::Supervisor;
use command::{
    Command, ConfigureAudioOk, ConfigureSdrOk, ConfigureSpectrumOk, SdrCapsOk, SdrGainOk,
    SdrTuneOk,
};
use error::CoreError;
use event::{FrameEvent, TelemetryEvent};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// How often the idle tick samples and publishes the host clock offset.
const CLOCK_METRIC_PERIOD: Duration = Duration::from_secs(1);

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
    spawn_with_devices(supervisor, enumerator, audio_factory, Vec::new())
}

/// Like [`spawn`], but seeds the config-file-registered devices merged into
/// `ListDevices`. Production loads these from the daemon config; tests and the
/// no-config path use the empty [`spawn`].
pub fn spawn_with_devices(
    supervisor: Supervisor,
    enumerator: Box<dyn DeviceEnumerator>,
    audio_factory: AudioBackendFactory,
    registered_devices: Vec<crate::device::DeviceDescriptor>,
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
        .spawn(move || {
            run(supervisor, enumerator, audio_factory, registered_devices, cmd_rx, frame_tx, tele_tx)
        })
        .expect("spawn core thread");

    (handle, join)
}

/// A channel's resolved audio binding: capture (RX) and playback (TX) devices,
/// which may be the same `DeviceId` (single rig) or differ (split rigs). The
/// interlock and TX lease gate on `tx_dev`; the RX worker reads on `rx_dev`.
/// `rx_req_rate`/`rx_rate`/`rx_mode` record what the running capture was opened
/// for so a re-bind to the same RX device can reuse it instead of re-claiming
/// an exclusively-held device (see `configure_audio`).
#[derive(Clone)]
struct AudioBinding {
    rx_dev: DeviceId,
    tx_dev: DeviceId,
    tx_rate: u32,
    /// Requested RX rate (as asked for by the caller; 0 = "backend's choice").
    rx_req_rate: u32,
    /// Actual RX rate the running capture opened at.
    rx_rate: u32,
    /// Mode the running RX worker was built for.
    rx_mode: crate::mode::ModeConfig,
}

/// Per-channel live audio/PTT bindings owned by the core loop. For a moded
/// channel the capture is consumed by an `RxWorker` and the sink+driver by a
/// `TxWorker`; for `ModeConfig::None` they stay here on the legacy path.
#[derive(Default)]
struct LiveBindings {
    sinks: HashMap<ChannelId, PlaybackHandle>,
    captures: HashMap<ChannelId, CaptureHandle>,
    /// Audio binding per channel (RX + TX device & rate).
    audio: HashMap<ChannelId, AudioBinding>,
    drivers: HashMap<ChannelId, Box<dyn PttDriver>>,
    /// PTT device id per channel (for eviction on hotplug).
    ptt_dev: HashMap<ChannelId, DeviceId>,
    /// Per-channel RX demod worker (moded channels only).
    rx_workers: HashMap<ChannelId, RxWorker>,
    /// Per-channel TX worker: cooperative queue → modulate → on-air.
    tx_workers: HashMap<ChannelId, TxWorker>,
    /// Per-channel live metrics accumulator, shared with the RX worker; the core
    /// reads its latest snapshot to answer `GetMetrics`.
    metrics: HashMap<ChannelId, SharedMetrics>,
    /// Per-channel runtime audio gain, shared with the RX/TX workers.
    gains: HashMap<ChannelId, AudioGain>,
    /// Per-channel spectrum (waterfall) control, shared with the RX worker.
    spectra: HashMap<ChannelId, spectrum::SpectrumControl>,
    /// Per-channel SDR runtime control, shared with an `rtl_tcp` capture thread.
    /// Present only for SDR-bound channels; gRPC (Plan 3) mutates it live.
    sdr_controls: HashMap<ChannelId, SdrControl>,
    /// Devices pre-registered in the daemon config file (currently `rtl_tcp`
    /// endpoints). Merged into `ListDevices` alongside the live enumeration so a
    /// frontend can present network SDRs that no hardware scan would surface.
    registered_devices: Vec<crate::device::DeviceDescriptor>,
}

/// The core loop. Blocks on `recv_timeout`; on a command, handles it; on the
/// idle tick, polls hotplug. Exits on `Shutdown` or a closed channel.
#[allow(clippy::too_many_arguments)]
fn run(
    mut supervisor: Supervisor,
    enumerator: Box<dyn DeviceEnumerator>,
    audio_factory: AudioBackendFactory,
    registered_devices: Vec<crate::device::DeviceDescriptor>,
    commands: Receiver<Command>,
    frames: broadcast::Sender<FrameEvent>,
    telemetry: broadcast::Sender<TelemetryEvent>,
) {
    let mut next_tx_id: u64 = 1;
    let mut live = LiveBindings { registered_devices, ..LiveBindings::default() };
    let interlock = supervisor.interlock();
    let lease = TxLeaseRegistry::new();
    // Persistence restores a channel's *config*, but not its live audio/PTT
    // bindings or workers — so after a restart the channel shows in the snapshot
    // yet can't RX (no spectrum/waterfall) or TX (AcquireTxLease can't find a
    // binding). Re-establish the live pipeline for restored channels here.
    restore_live_bindings(
        &mut supervisor, &*enumerator, &audio_factory, &interlock, &lease, &mut live, &frames,
        &telemetry,
    );
    let mut watcher = HotplugWatcher::new();
    let clock = ClockSource::new();
    // Initialize in the past so the first idle tick publishes immediately.
    let mut last_clock = Instant::now() - CLOCK_METRIC_PERIOD * 2;

    loop {
        match commands.recv_timeout(HOTPLUG_POLL) {
            Ok(Command::Shutdown) => break,
            Ok(cmd) => {
                // A panic inside one command must not unwind the whole core
                // thread — that would drop the reply (the caller sees "core
                // dropped reply") *and* silently kill the live feed. Catch it
                // here so a single bad command (e.g. a device enumeration that
                // panics deep in a platform audio backend) degrades to a failed
                // reply while the core keeps serving.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle_command(
                        cmd,
                        &mut supervisor,
                        &*enumerator,
                        &audio_factory,
                        &interlock,
                        &lease,
                        &mut live,
                        &mut next_tx_id,
                        &frames,
                        &telemetry,
                    )
                }));
                if outcome.is_err() {
                    tracing::error!("core command handler panicked; feed preserved, reply dropped");
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                poll_hotplug(
                    &mut watcher, &*enumerator, &mut supervisor, &mut live, &lease, &telemetry,
                );
                if last_clock.elapsed() >= CLOCK_METRIC_PERIOD {
                    let r = clock.read();
                    let _ = telemetry.send(TelemetryEvent::ClockOffset {
                        offset_s: r.offset_s,
                        est_error_s: r.est_error_s,
                        synchronized: r.synchronized,
                    });
                    last_clock = Instant::now();
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Rebuild the live audio/PTT pipeline and workers for channels restored from
/// persistence, so RX/TX (and the waterfall) work immediately after a daemon
/// restart rather than only after the operator reconfigures. Channels that were
/// never given a real device (still the placeholder), or whose devices are gone,
/// are left config-only and logged — the operator reconfigures when ready.
#[allow(clippy::too_many_arguments)]
fn restore_live_bindings(
    supervisor: &mut Supervisor,
    enumerator: &dyn DeviceEnumerator,
    audio_factory: &AudioBackendFactory,
    interlock: &crate::ptt::interlock::RxTxInterlock,
    lease: &TxLeaseRegistry,
    live: &mut LiveBindings,
    frames: &broadcast::Sender<FrameEvent>,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) {
    for cfg in supervisor.snapshot().channels {
        if cfg.device_id == DeviceId::placeholder() {
            continue; // configured but never bound to a real RX device
        }
        if let Err(e) = configure_audio(
            supervisor, enumerator, audio_factory, live, cfg.id, cfg.device_id.clone(),
            cfg.sample_rate, cfg.fanout, cfg.tx_device_id.clone(), cfg.tx_sample_rate, telemetry,
        ) {
            tracing::warn!(channel = cfg.id.0, error = %e, "skipping audio restore on startup");
            continue;
        }
        if let Some(ptt) = cfg.ptt.clone() {
            if let Err(e) = configure_ptt(supervisor, live, cfg.id, ptt) {
                tracing::warn!(channel = cfg.id.0, error = %e, "skipping ptt restore on startup");
            }
        }
        try_spawn_workers(cfg.id, supervisor, live, interlock, lease, frames, telemetry);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    cmd: Command,
    supervisor: &mut Supervisor,
    enumerator: &dyn DeviceEnumerator,
    audio_factory: &AudioBackendFactory,
    interlock: &crate::ptt::interlock::RxTxInterlock,
    lease: &TxLeaseRegistry,
    live: &mut LiveBindings,
    next_tx_id: &mut u64,
    frames: &broadcast::Sender<FrameEvent>,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) {
    match cmd {
        Command::ConfigureChannel { id, name, mode, rsid_tx, rsid_rx, reply } => {
            // Validate the mode string against the parametric registry before
            // persisting, so a typo can't silently configure nothing. The
            // string remains the persisted form (gRPC proto unchanged); it is
            // resolved to a `ModeConfig` at use.
            let res = match crate::mode::ModeConfig::parse(&mode) {
                Some(_) => {
                    tracing::info!(channel = id.0, name = %name, mode = %mode, "mode selected");
                    supervisor
                        .configure_channel(id, name, mode, rsid_tx, rsid_rx)
                        .map_err(Into::into)
                }
                None => {
                    tracing::warn!(channel = id.0, mode = %mode, "rejected unknown mode");
                    Err(CoreError::UnknownMode(mode))
                }
            };
            if res.is_ok() {
                let _ = telemetry.send(TelemetryEvent::ChannelConfigured { channel: id });
            }
            let _ = reply.send(res);
        }

        Command::ConfigureAudio {
            id, device_id, sample_rate, fanout, tx_device_id, tx_sample_rate, reply,
        } => {
            let res = configure_audio(
                supervisor, enumerator, audio_factory, live, id, device_id, sample_rate, fanout,
                tx_device_id, tx_sample_rate, telemetry,
            );
            if res.is_ok() {
                try_spawn_workers(id, supervisor, live, interlock, lease, frames, telemetry);
            }
            let _ = reply.send(res);
        }

        Command::ConfigurePtt { id, ptt, reply } => {
            let res = configure_ptt(supervisor, live, id, ptt);
            if res.is_ok() {
                try_spawn_workers(id, supervisor, live, interlock, lease, frames, telemetry);
            }
            let _ = reply.send(res);
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

        Command::TransmitImage { channel, send, reply } => {
            if !supervisor.has_channel(channel) {
                let _ = reply.send(Err(CoreError::UnknownChannel(channel)));
                return;
            }
            let tx_id = TransmitId(*next_tx_id);
            *next_tx_id += 1;
            let res = transmit_image(supervisor, live, channel, send, tx_id);
            let _ = reply.send(res);
        }

        Command::ListDevices { reply } => {
            // Enumeration reaches into platform backends (CoreAudio, libusb) that
            // can panic under the macOS App Sandbox. Catch it here, holding `reply`
            // outside the guard, so a scan panic degrades to a partial list rather
            // than dropping the reply — a dropped reply reads to the caller as
            // "daemon stopped responding" even though the core (and live feed)
            // survive via the loop's outer catch_unwind.
            let cache = supervisor.device_cache_mut();
            let mut devices = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cache.refresh(enumerator)
            }))
            .unwrap_or_else(|_| {
                tracing::error!("device scan panicked; returning registered devices only");
                Vec::new()
            });
            // Merge config-file-registered endpoints (e.g. remote `rtl_tcp` SDRs)
            // that no hardware scan produces. A registered id that the live
            // enumeration already surfaced is not duplicated.
            for reg in &live.registered_devices {
                if !devices.iter().any(|d| d.id == reg.id) {
                    devices.push(reg.clone());
                }
            }
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

        Command::GetMetrics { channel, reply } => {
            let snaps: Vec<ChannelMetricsSnapshot> = live
                .metrics
                .iter()
                .filter(|(c, _)| channel.is_none_or(|want| want == **c))
                .map(|(c, m)| m.lock().unwrap().snapshot(*c))
                .collect();
            let _ = reply.send(snaps);
        }

        Command::AcquireTxLease { channel, reply } => {
            let res = match live.audio.get(&channel).map(|b| b.tx_dev.clone()) {
                Some(rig) => match lease.acquire(&rig, channel) {
                    Ok(()) => Ok(command::LeaseGrant { granted: true, held_by: Some(channel) }),
                    Err(crate::ptt::lease::LeaseError::HeldBy(h)) => {
                        Ok(command::LeaseGrant { granted: false, held_by: Some(h) })
                    }
                },
                None => Err(CoreError::UnknownChannel(channel)),
            };
            let _ = reply.send(res);
        }

        Command::ReleaseTxLease { channel, reply } => {
            let res = match live.audio.get(&channel).map(|b| b.tx_dev.clone()) {
                Some(rig) => {
                    lease.release(&rig, channel);
                    Ok(())
                }
                None => Err(CoreError::UnknownChannel(channel)),
            };
            let _ = reply.send(res);
        }

        Command::SetAudioGain { channel, rx_gain, tx_gain, reply } => {
            // Create-or-update the gain cell so the call works whether or not
            // audio/workers exist yet. A running worker holds a clone of the same
            // Arc cells, so this update is seen on its next chunk — no respawn.
            let res = if supervisor.has_channel(channel) {
                live.gains.entry(channel).or_default().set(rx_gain, tx_gain);
                Ok(())
            } else {
                Err(CoreError::UnknownChannel(channel))
            };
            let _ = reply.send(res);
        }

        Command::ConfigureSpectrum {
            channel,
            enable,
            bin_count,
            fft_size,
            rate_hz,
            freq_lo_hz,
            freq_hi_hz,
            reply,
        } => {
            let res = configure_spectrum(
                supervisor, live, channel, enable, bin_count, fft_size, rate_hz, freq_lo_hz,
                freq_hi_hz,
            );
            let _ = reply.send(res);
        }

        Command::SetSdrTune { channel, freq_hz, reply } => {
            let res = set_sdr_tune(live, channel, freq_hz, telemetry);
            let _ = reply.send(res);
        }

        Command::SetSdrGain { channel, auto, gain_db, reply } => {
            let res = set_sdr_gain(live, channel, auto, gain_db, telemetry);
            let _ = reply.send(res);
        }

        Command::ConfigureSdr {
            channel,
            capture_rate,
            demod_mode,
            squelch_db,
            ppm,
            bias_tee,
            direct_sampling,
            reply,
        } => {
            let res = configure_sdr(
                live, channel, capture_rate, demod_mode, squelch_db, ppm, bias_tee,
                direct_sampling, telemetry,
            );
            let _ = reply.send(res);
        }

        Command::GetSdrCaps { channel, reply } => {
            let res = get_sdr_caps(live, channel);
            let _ = reply.send(res);
        }

        Command::Shutdown => {} // handled in run()
    }
}

/// Enable/disable a channel's spectrum stream. The shared `SpectrumControl` is
/// created-or-updated so the call works whether or not a worker exists yet; a
/// running RX worker holds a clone of the same handle and reconciles its tap on
/// the next chunk — no respawn. Echoes the actual clamped params, resolved
/// against the channel's demod native rate.
#[allow(clippy::too_many_arguments)]
fn configure_spectrum(
    supervisor: &Supervisor,
    live: &mut LiveBindings,
    channel: ChannelId,
    enable: bool,
    bin_count: u32,
    fft_size: u32,
    rate_hz: u32,
    freq_lo_hz: f32,
    freq_hi_hz: f32,
) -> Result<ConfigureSpectrumOk, CoreError> {
    if !supervisor.has_channel(channel) {
        return Err(CoreError::UnknownChannel(channel));
    }
    let control = live.spectra.entry(channel).or_default();
    if !enable {
        control.disable();
        return Ok(ConfigureSpectrumOk::default());
    }
    // Resolve against the demod native rate (the rate the spectrum FFT sees).
    let mode = supervisor.channel_mode(channel);
    let native_rate = registry::native_rate(&mode)
        .ok_or_else(|| CoreError::UnknownMode("channel has no RX mode to tap a spectrum from".into()))?;
    let setup = omnimodem_dsp::frontend::spectrum::SpectrumSetup::resolve(
        native_rate, bin_count, fft_size, rate_hz, freq_lo_hz, freq_hi_hz,
    );
    control.enable(spectrum::SpectrumCfg { bin_count, fft_size, rate_hz, freq_lo_hz, freq_hi_hz });
    Ok(ConfigureSpectrumOk {
        bin_count: setup.plan.bin_count as u32,
        fft_size: setup.nfft as u32,
        rate_hz: setup.rate_hz,
        freq_start_hz: setup.plan.freq_start_hz,
        freq_step_hz: setup.plan.freq_step_hz,
    })
}

// ---------------------------------------------------------------------------
// SDR (rtl_tcp) runtime control
// ---------------------------------------------------------------------------

/// The channel's SDR control cell, or `SdrRequired` when the channel is not bound
/// to an `rtl_tcp` source (the cell exists only for SDR-bound channels).
fn sdr_control(
    live: &LiveBindings,
    channel: ChannelId,
) -> Result<&SdrControl, CoreError> {
    live.sdr_controls.get(&channel).ok_or(CoreError::SdrRequired(channel))
}

/// Broadcast the channel's current SDR state so multiple/late-joining clients stay
/// in sync. Called after every mutating SDR RPC.
fn emit_sdr_state(
    telemetry: &broadcast::Sender<TelemetryEvent>,
    channel: ChannelId,
    control: &SdrControl,
) {
    let center_hz = control.center_hz();
    let offset_hz = control.offset_hz() as f64;
    let _ = telemetry.send(TelemetryEvent::SdrState {
        channel,
        center_hz,
        offset_hz,
        freq_hz: center_hz + offset_hz,
        gain_auto: control.gain_auto(),
        gain_db: control.gain_db(),
        demod_mode: control.demod_mode() as u8,
        squelch_db: control.squelch_db(),
    });
}

/// Tune an SDR channel to an absolute frequency, splitting into hardware center +
/// NCO offset (the gqrx model). Mutates the shared control (the capture thread
/// picks it up on its next block) and broadcasts the new state.
fn set_sdr_tune(
    live: &mut LiveBindings,
    channel: ChannelId,
    freq_hz: f64,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) -> Result<SdrTuneOk, CoreError> {
    let control = sdr_control(live, channel)?;
    let (center, offset) = plan_tune(control.center_hz(), control.capture_rate(), freq_hz);
    control.set_center_hz(center);
    control.set_offset_hz(offset);
    emit_sdr_state(telemetry, channel, control);
    let offset = offset as f64;
    Ok(SdrTuneOk { actual_freq_hz: center + offset, center_hz: center, offset_hz: offset })
}

/// Set an SDR channel's gain: auto AGC, or a manual value snapped to the tuner's
/// discrete gain table (when caps are known). Echoes the applied gain.
fn set_sdr_gain(
    live: &mut LiveBindings,
    channel: ChannelId,
    auto: bool,
    gain_db: f32,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) -> Result<SdrGainOk, CoreError> {
    let control = sdr_control(live, channel)?;
    let applied = if auto {
        control.set_gain(true, 0.0);
        0.0
    } else {
        // Snap to the nearest table entry so the echoed gain is what the dongle
        // will actually use; an unknown tuner (no table) passes the value through.
        let snapped = match control.caps() {
            Some(caps) => snap_gain(&caps.gains_db, gain_db),
            None => gain_db,
        };
        control.set_gain(false, snapped);
        snapped
    };
    emit_sdr_state(telemetry, channel, control);
    Ok(SdrGainOk { actual_gain_db: applied })
}

/// Configure an SDR channel's source parameters: capture rate, demod mode,
/// squelch, ppm, bias-tee, and direct-sampling. All demod modes (NBFM/AM/WFM/SSB)
/// are supported; the dongle-extras controls are applied to the shared
/// `SdrControl`, where the capture thread sends them to the dongle on its next
/// block. Direct sampling (`bool`) maps to the Q-branch (mode 2) — the branch
/// consumer HF-capable dongles wire their HF input to. Echoes the effective rate.
#[allow(clippy::too_many_arguments)]
fn configure_sdr(
    live: &mut LiveBindings,
    channel: ChannelId,
    capture_rate: u32,
    demod_mode: u8,
    squelch_db: f32,
    ppm: i32,
    bias_tee: bool,
    direct_sampling: bool,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) -> Result<ConfigureSdrOk, CoreError> {
    let control = sdr_control(live, channel)?;

    // Every demod mode (NBFM/AM/WFM/SSB) is implemented; the capture thread
    // dispatches on the mode via `SdrDemod`. Bias-tee and direct-sampling are
    // applied below (Phase C) — no control is rejected here anymore.
    let mode = DemodMode::from_u8(demod_mode);

    // Capture rate: 0 = unchanged; otherwise it must be a rate the dongle accepts.
    let actual_capture_rate = if capture_rate == 0 {
        control.capture_rate()
    } else {
        if !supported_sample_rates().contains(&capture_rate) {
            return Err(CoreError::InvalidArgument(format!(
                "unsupported capture rate {capture_rate} Hz"
            )));
        }
        control.set_capture_rate(capture_rate);
        capture_rate
    };

    control.set_demod_mode(mode);
    control.set_squelch_db(squelch_db);
    control.set_ppm(ppm);
    control.set_bias_tee(bias_tee);
    control.set_direct_sampling(if direct_sampling { DIRECT_SAMPLING_Q_BRANCH } else { 0 });
    emit_sdr_state(telemetry, channel, control);
    Ok(ConfigureSdrOk { actual_capture_rate })
}

/// Read an SDR channel's tuner capabilities (published by the capture thread at
/// connect). `SdrRequired` if the channel is not SDR-bound; a bound channel whose
/// capture has not yet parsed the header reports an empty/unknown tuner.
fn get_sdr_caps(live: &LiveBindings, channel: ChannelId) -> Result<SdrCapsOk, CoreError> {
    let control = sdr_control(live, channel)?;
    let caps = control.caps().unwrap_or_else(|| crate::audio::sdr::TunerCaps {
        tuner: "unknown".into(),
        freq_min_hz: 0.0,
        freq_max_hz: 0.0,
        sample_rates: supported_sample_rates(),
        gains_db: Vec::new(),
        bias_tee_supported: false,
        direct_sampling_supported: false,
    });
    // Direct sampling bypasses the tuner to reach HF, so when it is active the
    // usable range widens down to DC up to the ADC Nyquist ceiling.
    let (freq_min_hz, freq_max_hz) = if control.direct_sampling() != 0 {
        (0.0, caps.freq_max_hz.max(DIRECT_SAMPLING_MAX_HZ))
    } else {
        (caps.freq_min_hz, caps.freq_max_hz)
    };
    Ok(SdrCapsOk {
        tuner: caps.tuner,
        freq_min_hz,
        freq_max_hz,
        sample_rates: caps.sample_rates,
        gains_db: caps.gains_db,
        bias_tee_supported: caps.bias_tee_supported,
        direct_sampling_supported: caps.direct_sampling_supported,
    })
}

/// Couple an SDR control cell to a channel's mode before the capture opens, so
/// the capture thread sends the correct demod mode and tune to `rtl_tcp` on
/// connect. Runs on every (re)bind.
///
/// ADS-B is the special case: it needs the wideband magnitude path (`RawMag`,
/// full-rate `|I+jQ|` at 2.4 Msps instead of channelized audio) **and** a tune to
/// its fixed 1090 MHz. Unlike the audio modes, the operator never tunes ADS-B, so
/// nothing else sets the frequency — without this the control keeps its 0 Hz
/// default and the daemon commands `rtl_tcp` to `set freq 0` (PLL unlocked, no
/// signal, zero aircraft). The tune is planned from a cold center so 1090 MHz
/// lands a quarter-band off hardware center, clear of the R820T DC spike; the
/// magnitude envelope is offset-invariant, so that placement does not hurt the
/// decode. Every other mode uses the channelizing demod and leaves tuning to the
/// operator — but a channel that was previously ADS-B must not stay stuck in
/// `RawMag`.
fn couple_sdr_mode_to_channel(control: &SdrControl, mode: &crate::mode::ModeConfig) {
    match mode {
        crate::mode::ModeConfig::Adsb => {
            control.set_demod_mode(DemodMode::RawMag);
            let (center, offset) = plan_tune(0.0, ADSB_CAPTURE_RATE, ADSB_FREQ_HZ);
            control.set_center_hz(center);
            control.set_offset_hz(offset);
        }
        _ if control.demod_mode() == DemodMode::RawMag => {
            control.set_demod_mode(DemodMode::Nbfm);
        }
        _ => {}
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
    tx_device_id: DeviceId,
    tx_sample_rate: u32,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) -> Result<ConfigureAudioOk, CoreError> {
    if !supervisor.has_channel(id) {
        return Err(CoreError::UnknownChannel(id));
    }
    let tx_rate_req = if tx_sample_rate == 0 { sample_rate } else { tx_sample_rate };
    supervisor.configure_audio(
        id, device_id.clone(), sample_rate, fanout, tx_device_id.clone(), tx_sample_rate,
    )?;

    // Resolve durable ids to live devices (refresh first so a never-listed
    // device still binds). Capture (RX) and playback (TX) may differ. An
    // `rtl_tcp` SDR endpoint needs no physical enumeration — synthesize a
    // capture-only descriptor so an ad-hoc `rtltcp:host:port` binds directly.
    supervisor.device_cache_mut().refresh(enumerator);
    let resolve = |sup: &mut Supervisor, dev: &DeviceId| -> Result<_, CoreError> {
        if let DeviceId::RtlTcp { host, port } = dev {
            return Ok(crate::device::DeviceDescriptor {
                id: dev.clone(),
                label: format!("rtl_tcp {host}:{port}"),
                has_capture: true,
                has_playback: false,
                needs_setup: false,
            });
        }
        sup.device_cache_mut()
            .resolve(dev)
            .cloned()
            .ok_or_else(|| {
                CoreError::Audio(crate::audio::AudioError::DeviceNotFound(
                    dev.to_canonical_string(),
                ))
            })
    };

    // Re-binding a channel to the RX device it is already capturing on, in the
    // same mode and at the same requested rate, must NOT re-open the device: a
    // local USB SDR claims interface 0 exclusively, so re-`open_capture` while
    // the running capture still holds it fails with "exclusive access" and the
    // channel loses its feed for good. The app's feed auto-reconnect re-sends
    // ConfigureAudio on every stream restart, so without this one stream blip
    // permanently kills reception. Keep the live capture; only the TX/bookkeeping
    // below refreshes. Reuse requires an actually-running RX (a finished worker
    // has already released the device, so re-open is both safe and needed).
    let mode = supervisor.channel_mode(id);
    let rx_running = live.rx_workers.get(&id).is_some_and(RxWorker::is_running)
        || live.captures.contains_key(&id);
    let reuse_rx = rx_running
        && live.audio.get(&id).is_some_and(|b| {
            b.rx_dev == device_id && b.rx_req_rate == sample_rate && b.rx_mode == mode
        });

    let rx_rate = if reuse_rx {
        let rate = live.audio.get(&id).map_or(sample_rate, |b| b.rx_rate);
        tracing::debug!(
            channel = id.0, device = %device_id.to_canonical_string(),
            "reusing running capture on re-bind (same RX device + mode)",
        );
        rate
    } else {
        let rx_desc = resolve(supervisor, &device_id)?;
        let mut rx_backend = (audio_factory)(&rx_desc);
        // For an SDR device, inject the per-channel control cell + telemetry sink so
        // the capture thread can honor runtime tune/gain/squelch and emit the RF
        // waterfall. The control persists across rebinds (settings survive a mode
        // switch); a departing device clears it in poll_hotplug.
        if device_id.is_sdr() {
            let control = live.sdr_controls.entry(id).or_default().clone();
            // Couple the channel mode to the SDR front-end before opening the capture,
            // so the capture thread sends the right demod mode + tune on connect.
            couple_sdr_mode_to_channel(&control, &mode);
            rx_backend.attach_sdr_context(id, telemetry.clone(), control);
        } else {
            // Rebinding to a non-SDR device: drop any stale control cell so a later
            // rebind to an SDR starts fresh rather than resurrecting old tune/gain.
            live.sdr_controls.remove(&id);
        }
        // Known limitation: on a *same-device* re-open (a real mode/rate switch
        // on one exclusive USB SDR, so `reuse_rx` is false) the old RxWorker is
        // still dropped below, after this claim — and its drop only signals stop,
        // releasing interface 0 asynchronously. So this `open_capture` can lose
        // the race and fail exclusive-access on real hardware. That is a
        // pre-existing hazard (not the reconnect symptom GRA-372 fixes, which is
        // the same device *and* mode → handled by the reuse path above); closing
        // it means releasing the old worker first and waiting, which risks
        // leaving the channel with zero RX if the re-open then fails. Tracked
        // separately.
        let capture = rx_backend.open_capture(sample_rate)?;
        let rate = capture.sample_rate;
        live.captures.insert(id, capture);
        rate
    };

    // Playback is best-effort: a TX device with no usable playback support — an
    // input-only device, or TX defaulting to the capture device — binds the
    // channel RX-only (receive works; transmit stays unavailable, signalled by
    // tx_rate == 0, until a real TX device is set). The TX worker already only
    // spawns when a sink exists, so an absent sink is safe. Other audio errors
    // (device gone, I/O) are genuine failures and propagate.
    let tx_desc = resolve(supervisor, &tx_device_id)?;
    let tx_rate = match (audio_factory)(&tx_desc).open_playback(tx_rate_req) {
        Ok(playback) => {
            let rate = playback.sample_rate;
            live.sinks.insert(id, playback);
            rate
        }
        Err(crate::audio::AudioError::NoUsableFormat { device }) => {
            tracing::warn!(channel = id.0, %device, "TX device has no usable playback; channel is RX-only");
            live.sinks.remove(&id); // drop any stale sink from a prior bind
            0
        }
        // An SDR (rtl_tcp) TX device is receive-only — bind the channel RX-only
        // rather than failing the whole configure, same as a mic-only device.
        Err(crate::audio::AudioError::Unsupported) => {
            tracing::warn!(channel = id.0, "TX device is receive-only (SDR); channel is RX-only");
            live.sinks.remove(&id);
            0
        }
        Err(e) => return Err(e.into()),
    };

    tracing::info!(
        channel = id.0,
        rx_device = %device_id.to_canonical_string(),
        tx_device = %tx_device_id.to_canonical_string(),
        rx_rate,
        tx_rate,
        "audio bound{}",
        if tx_rate == 0 { " (RX-only)" } else { "" },
    );
    live.audio.insert(
        id,
        AudioBinding {
            rx_dev: device_id,
            tx_dev: tx_device_id,
            tx_rate,
            rx_req_rate: sample_rate,
            rx_rate,
            rx_mode: mode,
        },
    );
    live.gains.entry(id).or_default(); // default unity until SetAudioGain

    // A re-bind supersedes any workers already running for this channel: drop them
    // so the try_spawn_workers calls (after this, and after the ptt step) rebuild
    // them against the fresh capture/sink AND the channel's current mode. Without
    // this, reconfiguring — e.g. switching modes — left the old RX/TX workers (and
    // the TX worker's old modulator) running, so RX decoding and the transmitted
    // audio never changed. try_spawn_workers consumed the prior capture/sink into
    // those workers, so dropping them also releases the stale streams.
    //
    // The RX worker is kept when we reused its capture (`reuse_rx`) — dropping it
    // would tear down the very stream we chose not to re-open, and its device
    // release is what a re-claim on an exclusive SDR would then deadlock against.
    if !reuse_rx {
        live.rx_workers.remove(&id);
    }
    live.tx_workers.remove(&id);

    Ok(ConfigureAudioOk { rx_rate, tx_rate })
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
    tracing::info!(
        channel = id.0,
        device = %ptt.device_id.to_canonical_string(),
        method = ?ptt.method,
        "PTT configured",
    );
    live.drivers.insert(id, driver);
    live.ptt_dev.insert(id, ptt.device_id);
    Ok(())
}

/// Spawn the per-channel RX/TX workers once their prerequisites exist. Called
/// after audio and after PTT config, since either may arrive first. Idempotent:
/// a worker is spawned at most once per channel. For `ModeConfig::None` no
/// worker is spawned — that channel stays on the legacy capture-idle / raw-PCM
/// transmit path.
#[allow(clippy::too_many_arguments)]
fn try_spawn_workers(
    channel: ChannelId,
    supervisor: &Supervisor,
    live: &mut LiveBindings,
    interlock: &crate::ptt::interlock::RxTxInterlock,
    lease: &TxLeaseRegistry,
    frames: &broadcast::Sender<FrameEvent>,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) {
    let mode = supervisor.channel_mode(channel);
    // The RX worker reads on the capture (RX) device.
    let rig = live.audio.get(&channel).map(|b| b.rx_dev.clone());
    // Shared runtime gain for this channel (cloned into the workers).
    let gain = live.gains.entry(channel).or_default().clone();
    // Shared spectrum control (cloned into the RX worker; default OFF). For an
    // SDR channel the RF waterfall is produced by the capture thread, so hand the
    // RX worker a throwaway control that can never enable its audio-passband tap —
    // keeping exactly one spectrum producer per channel.
    let is_sdr = rig.as_ref().is_some_and(DeviceId::is_sdr);
    let spectrum = if is_sdr {
        spectrum::SpectrumControl::default()
    } else {
        live.spectra.entry(channel).or_default().clone()
    };
    // Per-channel RSID enables: (tx = prepend our burst, rx = detect inbound).
    let (rsid_tx, rsid_rx) = supervisor.channel_rsid(channel);

    // RX worker: needs a capture and a real demod. Consume the held capture.
    if !live.rx_workers.contains_key(&channel) {
        if let (Some(rig), Some(capture)) = (rig.clone(), live.captures.remove(&channel)) {
            let metrics = live
                .metrics
                .entry(channel)
                .or_insert_with(|| Arc::new(Mutex::new(ChannelMetrics::default())))
                .clone();
            match registry::demod_kind(&mode) {
                DemodKind::Streaming(demod) => {
                    tracing::info!(
                        channel = channel.0, mode = mode.label(),
                        device = %rig.to_canonical_string(), "RX started (streaming)",
                    );
                    // ADS-B folds decoded packets into aircraft state and emits
                    // `AircraftReport` telemetry (the TUI flights table); other modes
                    // don't. One reporter lives for the channel's RX lifetime.
                    let reporter = matches!(mode, crate::mode::ModeConfig::Adsb).then(|| {
                        crate::core::adsb::AdsbReporter::new(channel, crate::core::adsb::DEFAULT_TTL_MS)
                    });
                    let w = RxWorker::spawn_streaming(
                        channel, rig, capture, demod, interlock.clone(), frames.clone(),
                        telemetry.clone(), metrics, gain.clone(), spectrum.clone(), rsid_rx,
                        reporter,
                    );
                    live.rx_workers.insert(channel, w);
                }
                DemodKind::Windowed(bd, window_s) => {
                    tracing::info!(
                        channel = channel.0, mode = mode.label(),
                        device = %rig.to_canonical_string(), window_s,
                        "RX started (windowed)",
                    );
                    let w = RxWorker::spawn_windowed(
                        channel, rig, capture, bd, interlock.clone(), frames.clone(),
                        telemetry.clone(), metrics, window_s, gain.clone(), spectrum.clone(),
                        rsid_rx,
                    );
                    live.rx_workers.insert(channel, w);
                }
                DemodKind::None => {
                    live.captures.insert(channel, capture); // hold idle
                }
            }
        }
    }

    // TX worker: needs a sink, a driver, and a modulating mode (not None).
    if !live.tx_workers.contains_key(&channel)
        && !matches!(mode, crate::mode::ModeConfig::None)
        && live.sinks.contains_key(&channel)
        && live.drivers.contains_key(&channel)
    {
        // The TX worker plays/keys on the playback (TX) device.
        if let (Some((rig, rate)), Some(modulator)) = (
            live.audio.get(&channel).map(|b| (b.tx_dev.clone(), b.tx_rate)),
            registry::build_modulator(&mode),
        ) {
            tracing::info!(
                channel = channel.0, mode = mode.label(),
                device = %rig.to_canonical_string(), "TX ready",
            );
            let sink = live.sinks.remove(&channel).unwrap();
            let driver = live.drivers.remove(&channel).unwrap();
            let slot_s = registry::tx_slot_s(&mode);
            // Resolve the mode's RSID burst once at spawn (key + audio offset).
            // `rsid_tx_key` (not `rsid_key`) gates TX: CW never prepends a burst,
            // even when the channel's sticky rsid_tx carried over from a prior
            // digital mode (GRA-318).
            let rsid = (rsid_tx)
                .then(|| mode.rsid_tx_key().map(|k| (k, mode.rsid_center_hz())))
                .flatten();
            let (tx_delay_ms, tx_tail_ms) = supervisor.channel_ptt_timing(channel);
            let w = tx_worker::spawn(TxWorkerCfg {
                channel,
                rig,
                rate,
                modulator,
                sink,
                driver,
                interlock: interlock.clone(),
                lease: lease.clone(),
                telemetry: telemetry.clone(),
                slot_s,
                gain: gain.clone(),
                spectrum: spectrum.clone(),
                rsid,
                tx_delay: Duration::from_millis(tx_delay_ms as u64),
                tx_tail: Duration::from_millis(tx_tail_ms as u64),
            });
            live.tx_workers.insert(channel, w);
        }
    }
}

fn key_ptt(
    supervisor: &mut Supervisor,
    interlock: &crate::ptt::interlock::RxTxInterlock,
    live: &mut LiveBindings,
    telemetry: &broadcast::Sender<TelemetryEvent>,
    channel: ChannelId,
    keyed: bool,
) -> Result<(), CoreError> {
    // Manual keying is a TX act → gate the interlock on the playback (TX) rig.
    let rig = live
        .audio
        .get(&channel)
        .map(|b| b.tx_dev.clone())
        .or_else(|| live.ptt_dev.get(&channel).cloned());
    // On a moded channel the PTT driver is owned by the TX worker, which keys
    // the rig as part of transmitting. Manual keying isn't available there.
    if live.tx_workers.contains_key(&channel) {
        return Err(CoreError::Ptt(PttError::Config(
            "channel is in a mode; the TX worker keys PTT during transmit — use Transmit, not manual key".into(),
        )));
    }
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
            tracing::info!(channel = channel.0, keyed, "PTT keyed (manual)");
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
    // Moded channel with a live TX worker: interpret the payload per-mode,
    // enqueue, and return immediately ("accepted onto the queue, not when it
    // leaves the air"). The worker emits TransmitStarted/Complete itself.
    if live.tx_workers.contains_key(&channel) {
        let mode = supervisor.channel_mode(channel);
        let frame = tx_worker::payload_to_frame(&mode, payload);
        let worker = live.tx_workers.get(&channel).unwrap();
        return match worker.enqueue(TxJob::frame(frame, tx_id)) {
            Ok(()) => Ok(tx_id),
            Err(_) => Err(CoreError::Ptt(PttError::Config("tx queue full".into()))),
        };
    }

    let _ = telemetry.send(TelemetryEvent::TransmitStarted { channel, transmit_id: tx_id });

    // Legacy path (ModeConfig::None / unmoded): a real cycle only when both
    // audio and PTT are bound; otherwise fall back to the Phase-1 simulation
    // (announce start/complete, ignore payload).
    let have_audio = live.sinks.contains_key(&channel) && live.audio.contains_key(&channel);
    let have_ptt = live.drivers.contains_key(&channel);

    let outcome = if have_audio && have_ptt {
        // Legacy raw-PCM cycle plays/keys on the playback (TX) rig.
        let b = live.audio.get(&channel).cloned().unwrap();
        let (rig, rate) = (b.tx_dev, b.tx_rate);
        let samples: Vec<i16> = payload
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]))
            .collect();
        let sink = live.sinks.get(&channel).unwrap();
        let driver = live.drivers.get_mut(&channel).unwrap();

        let (tx_delay_ms, tx_tail_ms) = supervisor.channel_ptt_timing(channel);
        interlock.begin_tx(&rig);
        let _ = telemetry.send(TelemetryEvent::PttKeyed { channel, keyed: true });
        // The legacy raw-PCM cycle runs inline on the core thread (no worker),
        // so there is no async cancel to honor — pass a never-set flag.
        let outcome = drive_tx_cycle(
            driver.as_mut(), sink, samples, rate, TX_POLL, &AtomicBool::new(false),
            Duration::from_millis(tx_delay_ms as u64), Duration::from_millis(tx_tail_ms as u64),
        );
        let _ = telemetry.send(TelemetryEvent::PttKeyed { channel, keyed: false });
        interlock.end_tx(&rig);
        Some(outcome)
    } else {
        None // simulation
    };

    let _ = telemetry.send(TelemetryEvent::TransmitComplete { channel, transmit_id: tx_id });

    match outcome {
        // Aborted is unreachable here (the legacy cancel flag is never set) but
        // is a clean stop, so it maps to Ok alongside Done.
        None | Some(TxCycleOutcome::Done) | Some(TxCycleOutcome::Aborted) => Ok(tx_id),
        Some(TxCycleOutcome::KeyFailed(e))
        | Some(TxCycleOutcome::SubmitFailed(e))
        | Some(TxCycleOutcome::UnkeyFailed(e)) => {
            evict_on_gone(supervisor, live, channel, &e);
            Err(CoreError::Ptt(e))
        }
    }
}

/// Transmit an image on a moded channel: build the header + pixel-FSK audio for
/// the channel's configured picture mode and enqueue it on the channel worker
/// (accepted onto the queue, not when it leaves the air — the worker emits
/// TransmitStarted/Complete + keys the rig). Errors if the channel has no live TX
/// worker or the mode/size can't carry the image.
fn transmit_image(
    supervisor: &Supervisor,
    live: &LiveBindings,
    channel: ChannelId,
    send: crate::mode::picture_tx::PictureSend,
    tx_id: TransmitId,
) -> Result<TransmitId, CoreError> {
    let worker = live
        .tx_workers
        .get(&channel)
        .ok_or_else(|| CoreError::Picture("channel has no active transmit worker".into()))?;
    let mode = supervisor.channel_mode(channel);
    let (audio, native_rate) =
        crate::mode::picture_tx::build(&mode, &send).map_err(|e| CoreError::Picture(e.to_string()))?;
    match worker.enqueue(TxJob::prebuilt(audio, native_rate, tx_id)) {
        Ok(()) => Ok(tx_id),
        Err(_) => Err(CoreError::Ptt(PttError::Config("tx queue full".into()))),
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
        live.tx_workers.remove(&channel);
        if let Some(id) = live.ptt_dev.remove(&channel) {
            supervisor.ptt_registry_mut().evict(&id);
        }
    }
}

/// Poll for hotplug changes and react: emit telemetry, and on departure evict
/// the PTT identity and drop any channel handles bound to that device.
#[allow(clippy::too_many_arguments)]
fn poll_hotplug(
    watcher: &mut HotplugWatcher,
    enumerator: &dyn DeviceEnumerator,
    supervisor: &mut Supervisor,
    live: &mut LiveBindings,
    lease: &TxLeaseRegistry,
    telemetry: &broadcast::Sender<TelemetryEvent>,
) {
    for ev in watcher.poll(enumerator) {
        match ev {
            HotplugEvent::Arrived(desc) => {
                tracing::info!(
                    device = %desc.id.to_canonical_string(), label = %desc.label,
                    "device arrived",
                );
                let _ = telemetry.send(TelemetryEvent::DeviceArrived {
                    device_id: desc.id,
                    label: desc.label,
                });
            }
            HotplugEvent::Departed(id) => {
                tracing::info!(device = %id.to_canonical_string(), "device departed");
                let _ = telemetry.send(TelemetryEvent::DeviceDeparted { device_id: id.clone() });
                supervisor.ptt_registry_mut().evict(&id);
                // Drop audio handles for channels bound to this device on
                // either the capture (RX) or playback (TX) side.
                let audio_chans: Vec<ChannelId> = live
                    .audio
                    .iter()
                    .filter(|(_, b)| b.rx_dev == id || b.tx_dev == id)
                    .map(|(c, _)| *c)
                    .collect();
                for c in audio_chans {
                    live.sinks.remove(&c);
                    live.captures.remove(&c);
                    live.audio.remove(&c);
                    live.rx_workers.remove(&c); // stop RX on the departed rig
                    live.tx_workers.remove(&c);
                    live.metrics.remove(&c);
                    // Drop the spectrum control so a replugged device starts with
                    // the waterfall OFF rather than silently resuming the FFT.
                    live.spectra.remove(&c);
                    live.sdr_controls.remove(&c); // gone SDR endpoint loses its control cell
                    lease.release_all(c); // free any lease held on the gone rig
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
                    live.tx_workers.remove(&c);
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
                    rsid_tx: false,
                    rsid_rx: false,
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

    async fn channel_info(core: &CoreHandle, id: ChannelId) -> crate::proto::ChannelInfo {
        let (t, r) = oneshot::channel();
        core.commands.send(Command::GetState { reply: t }).unwrap();
        let snap = r.await.unwrap();
        crate::grpc::convert::snapshot_to_proto(&snap)
            .channels
            .into_iter()
            .find(|c| c.channel == id.0)
            .expect("channel present in snapshot")
    }

    // End-to-end reproduction of the operator report: pick RX, a distinct TX, and
    // a PTT device (leaving the method at the TUI default VOX), then read back the
    // snapshot the client preloads on reopen. All three must survive — including
    // across a later mode change (the TUI sends ConfigureChannel alone for that).
    #[test]
    fn snapshot_reports_all_devices_after_config_and_mode_change() {
        let rx = named_device("Rig-RX");
        let tx = named_device("Rig-TX");
        let ptt = named_device("Rig-PTT");
        let (rx_id, tx_id, ptt_id) = (rx.id.clone(), tx.id.clone(), ptt.id.clone());
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![rx, tx, ptt])),
            Box::new(|_| Box::new(NullBackend::new(48_000))),
        );
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "psk31").await;
            configure_audio_split(&core, ChannelId(0), rx_id.clone(), tx_id.clone()).await;
            let (t, r) = oneshot::channel();
            core.commands
                .send(Command::ConfigurePtt {
                    id: ChannelId(0),
                    ptt: PttConfig { device_id: ptt_id.clone(), method: PttMethod::Vox, invert: false, tx_delay_ms: 0, tx_tail_ms: 0 },
                    reply: t,
                })
                .unwrap();
            r.await.unwrap().unwrap();

            let ci = channel_info(&core, ChannelId(0)).await;
            assert_eq!(ci.device_id, rx_id.to_canonical_string(), "RX after config");
            assert_eq!(ci.tx_device_id, tx_id.to_canonical_string(), "TX after config");
            assert_eq!(ci.ptt_device_id, ptt_id.to_canonical_string(), "PTT after config");

            configure_channel(&core, ChannelId(0), "rtty").await;
            let ci = channel_info(&core, ChannelId(0)).await;
            assert_eq!(ci.mode, "rtty");
            assert_eq!(ci.device_id, rx_id.to_canonical_string(), "RX after mode change");
            assert_eq!(ci.tx_device_id, tx_id.to_canonical_string(), "TX after mode change");
            assert_eq!(ci.ptt_device_id, ptt_id.to_canonical_string(), "PTT after mode change");
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    // The operator's exact macOS setup: ONE BlackHole device (a Placeholder id,
    // since CoreAudio names have no ALSA CARD= token) used for RX, TX, and PTT,
    // with the default VOX method. After configuring, the snapshot the client
    // preloads must report the PTT device — "PTT still not persisted" was the
    // remaining report after the display fixes landed.
    #[test]
    fn snapshot_reports_placeholder_ptt_device_single_blackhole() {
        let bh = DeviceDescriptor {
            id: DeviceId::Placeholder { tag: "BlackHole 2ch".into() },
            label: "BlackHole 2ch".into(),
            has_capture: true,
            has_playback: true,
            needs_setup: false,
        };
        let bh_id = bh.id.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![bh])),
            Box::new(|_| Box::new(NullBackend::new(48_000))),
        );
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "psk31").await;
            // Same device for RX and TX (one BlackHole for both).
            configure_audio_split(&core, ChannelId(0), bh_id.clone(), bh_id.clone()).await;
            let (t, r) = oneshot::channel();
            core.commands
                .send(Command::ConfigurePtt {
                    id: ChannelId(0),
                    ptt: PttConfig { device_id: bh_id.clone(), method: PttMethod::Vox, invert: false, tx_delay_ms: 0, tx_tail_ms: 0 },
                    reply: t,
                })
                .unwrap();
            r.await.unwrap().unwrap();

            let ci = channel_info(&core, ChannelId(0)).await;
            assert_eq!(ci.device_id, "virtual:BlackHole 2ch", "RX");
            assert_eq!(ci.ptt_device_id, "virtual:BlackHole 2ch", "PTT device must be reported");
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    // A device-based PTT method whose driver can't open (a serial method with no
    // usable node — exactly what the TUI sends, since it omits node/pin) returns
    // an error to the client, but the config is still persisted (configure_ptt
    // commits before opening the driver). The client must therefore refresh from
    // the snapshot on error, or the saved device looks lost — the "PTT device
    // still not persisted" report.
    #[test]
    fn ptt_config_persists_even_when_driver_open_fails() {
        let bh = DeviceDescriptor {
            id: DeviceId::Placeholder { tag: "BlackHole 2ch".into() },
            label: "BlackHole 2ch".into(),
            has_capture: true,
            has_playback: true,
            needs_setup: false,
        };
        let bh_id = bh.id.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![bh])),
            Box::new(|_| Box::new(NullBackend::new(48_000))),
        );
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "psk31").await;
            configure_audio_split(&core, ChannelId(0), bh_id.clone(), bh_id.clone()).await;
            let (t, r) = oneshot::channel();
            core.commands
                .send(Command::ConfigurePtt {
                    id: ChannelId(0),
                    ptt: PttConfig {
                        device_id: bh_id.clone(),
                        method: PttMethod::SerialRts { node: String::new() },
                        invert: false, tx_delay_ms: 0, tx_tail_ms: 0,
                    },
                    reply: t,
                })
                .unwrap();
            assert!(r.await.unwrap().is_err(), "opening a serial PTT with no node must error");

            // ...yet the device choice is persisted and surfaced by the snapshot.
            let ci = channel_info(&core, ChannelId(0)).await;
            assert_eq!(ci.ptt_device_id, "virtual:BlackHole 2ch");
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
    fn transmit_image_on_unknown_channel_errors() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (core, join) = fresh_core();
        rt.block_on(async {
            let (tx, rx) = oneshot::channel();
            let send = crate::mode::picture_tx::PictureSend {
                rgb: vec![0; 12],
                width: 2,
                height: 2,
                color: false,
                txspp: 8,
            };
            core.commands
                .send(Command::TransmitImage { channel: ChannelId(9), send, reply: tx })
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
            needs_setup: false,
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
                    rsid_tx: false,
                    rsid_rx: false,
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
                    tx_device_id: dev_id.clone(),
                    tx_sample_rate: 0,
                    reply: tx,
                })
                .unwrap();
            assert_eq!(rx.await.unwrap().unwrap().rx_rate, 48_000);

            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::ConfigurePtt {
                    id: ChannelId(0),
                    ptt: PttConfig {
                        device_id: dev_id.clone(),
                        method: PttMethod::None,
                        invert: false, tx_delay_ms: 0, tx_tail_ms: 0,
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

    async fn configure_channel(core: &CoreHandle, id: ChannelId, mode: &str) {
        let (tx, rx) = oneshot::channel();
        core.commands
            .send(Command::ConfigureChannel {
                id,
                name: "ch".into(),
                mode: mode.into(),
                rsid_tx: false,
                rsid_rx: false,
                reply: tx,
            })
            .unwrap();
        rx.await.unwrap().unwrap();
    }

    async fn configure_audio_ch(core: &CoreHandle, id: ChannelId, dev: DeviceId) {
        let (tx, rx) = oneshot::channel();
        core.commands
            .send(Command::ConfigureAudio {
                id,
                device_id: dev.clone(),
                sample_rate: 48_000,
                fanout: 1,
                tx_device_id: dev,
                tx_sample_rate: 0,
                reply: tx,
            })
            .unwrap();
        rx.await.unwrap().unwrap();
    }

    /// Configure split RX/TX devices on a channel, returning the opened rates.
    async fn configure_audio_split(
        core: &CoreHandle,
        id: ChannelId,
        rx_dev: DeviceId,
        tx_dev: DeviceId,
    ) -> crate::core::command::ConfigureAudioOk {
        let (tx, rx) = oneshot::channel();
        core.commands
            .send(Command::ConfigureAudio {
                id,
                device_id: rx_dev,
                sample_rate: 48_000,
                fanout: 1,
                tx_device_id: tx_dev,
                tx_sample_rate: 0,
                reply: tx,
            })
            .unwrap();
        rx.await.unwrap().unwrap()
    }

    fn loop_device() -> DeviceDescriptor {
        DeviceDescriptor {
            id: DeviceId::AlsaCard { card_name: "loop".into() },
            label: "loop".into(),
            has_capture: true,
            has_playback: true,
            needs_setup: false,
        }
    }

    fn named_device(name: &str) -> DeviceDescriptor {
        DeviceDescriptor {
            id: DeviceId::AlsaCard { card_name: name.into() },
            label: name.into(),
            has_capture: true,
            has_playback: true,
            needs_setup: false,
        }
    }

    #[test]
    fn set_audio_gain_updates_known_channel_and_rejects_unknown() {
        let dev = loop_device();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(|_| Box::new(FileBackend::from_samples(vec![], 48_000))),
        );
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "none").await;

            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::SetAudioGain {
                    channel: ChannelId(0),
                    rx_gain: 3.0,
                    tx_gain: 0.25,
                    reply: tx,
                })
                .unwrap();
            assert!(rx.await.unwrap().is_ok());

            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::SetAudioGain {
                    channel: ChannelId(9),
                    rx_gain: 1.0,
                    tx_gain: 1.0,
                    reply: tx,
                })
                .unwrap();
            assert!(matches!(rx.await.unwrap(), Err(CoreError::UnknownChannel(_))));
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    #[test]
    fn sdr_rpcs_require_an_sdr_bound_channel() {
        // A channel with no SDR binding must reject every SDR control RPC with
        // SdrRequired (mapped to FAILED_PRECONDITION at the gRPC edge).
        let (core, join) = fresh_core();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "none").await;

            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::SetSdrTune { channel: ChannelId(0), freq_hz: 144_390_000.0, reply: tx })
                .unwrap();
            assert!(matches!(rx.await.unwrap(), Err(CoreError::SdrRequired(_))));

            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::GetSdrCaps { channel: ChannelId(0), reply: tx })
                .unwrap();
            assert!(matches!(rx.await.unwrap(), Err(CoreError::SdrRequired(_))));
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    #[test]
    fn configure_sdr_applies_bias_tee_and_direct_sampling_and_widens_caps() {
        // Phase C: bias-tee and direct-sampling flow through to the shared control,
        // and direct-sampling widens the reported tunable range down to HF.
        use crate::audio::sdr::{caps_from_header, RtlHeader};
        let mut live = LiveBindings::default();
        let ctrl = SdrControl::default();
        ctrl.set_caps(caps_from_header(&RtlHeader { tuner_type: 5, tuner_gain_count: 29 }));
        live.sdr_controls.insert(ChannelId(0), ctrl.clone());
        let (tele, _keep) = broadcast::channel(16);

        // Baseline: without direct-sampling the range is the tuner's normal span.
        let base = get_sdr_caps(&live, ChannelId(0)).unwrap();
        assert!(base.freq_min_hz > 0.0);
        assert!(base.bias_tee_supported && base.direct_sampling_supported);

        let ok = configure_sdr(
            &mut live, ChannelId(0), 0, 0, -200.0, 0, true, true, &tele,
        )
        .unwrap();
        assert_eq!(ok.actual_capture_rate, ctrl.capture_rate());
        assert!(ctrl.bias_tee());
        assert_eq!(ctrl.direct_sampling(), 2); // bool true → Q-branch

        // Caps now advertise HF: floor at DC, ceiling at least the ADC Nyquist.
        let caps = get_sdr_caps(&live, ChannelId(0)).unwrap();
        assert_eq!(caps.freq_min_hz, 0.0);
        assert!(caps.freq_max_hz >= DIRECT_SAMPLING_MAX_HZ);

        // Turning direct-sampling back off restores the tuner's normal range.
        configure_sdr(&mut live, ChannelId(0), 0, 0, -200.0, 0, false, false, &tele).unwrap();
        assert!(!ctrl.bias_tee());
        assert_eq!(ctrl.direct_sampling(), 0);
        let restored = get_sdr_caps(&live, ChannelId(0)).unwrap();
        assert_eq!(restored.freq_min_hz, base.freq_min_hz);
    }

    #[test]
    fn couple_sdr_mode_to_channel_tunes_adsb_to_1090_clear_of_dc_spike() {
        use crate::audio::sdr::{DemodMode, ADSB_FREQ_HZ};
        use crate::mode::ModeConfig;
        // Regression: an ADS-B bind used to only select RawMag and leave the
        // frequency at its 0 Hz default, so the daemon told rtl_tcp `set freq 0`
        // and decoded nothing. A fresh control is untuned...
        let ctrl = SdrControl::default();
        assert_eq!(ctrl.center_hz(), 0.0);

        couple_sdr_mode_to_channel(&ctrl, &ModeConfig::Adsb);

        // ...after coupling it runs the wideband magnitude path AND is tuned to
        // exactly 1090 MHz (center + NCO offset).
        assert_eq!(ctrl.demod_mode(), DemodMode::RawMag);
        assert_eq!(ctrl.center_hz() + ctrl.offset_hz() as f64, ADSB_FREQ_HZ);
        // The hardware is actually tuned (not `set freq 0`), and 1090 sits off
        // center so it clears the R820T DC spike.
        assert_ne!(ctrl.center_hz(), 0.0);
        assert!((ctrl.center_hz() - ADSB_FREQ_HZ).abs() > 100_000.0);
    }

    #[test]
    fn couple_sdr_mode_to_channel_resets_raw_mag_when_leaving_adsb() {
        use crate::audio::sdr::DemodMode;
        use crate::mode::ModeConfig;
        let ctrl = SdrControl::default();
        couple_sdr_mode_to_channel(&ctrl, &ModeConfig::Adsb);
        assert_eq!(ctrl.demod_mode(), DemodMode::RawMag);
        // Rebinding the same channel to a non-ADS-B mode must not leave it stuck
        // in the wideband path.
        couple_sdr_mode_to_channel(&ctrl, &ModeConfig::None);
        assert_eq!(ctrl.demod_mode(), DemodMode::Nbfm);
    }

    #[test]
    fn list_devices_surfaces_registered_rtl_tcp_endpoints() {
        // A config-registered rtl_tcp endpoint must appear in ListDevices even
        // though no hardware enumeration produces it, alongside live devices.
        let reg = DeviceDescriptor {
            id: DeviceId::RtlTcp { host: "192.168.1.50".into(), port: 1234 },
            label: "Rooftop".into(),
            has_capture: true,
            has_playback: false,
            needs_setup: false,
        };
        let store = Store::open_in_memory().unwrap();
        let sup = Supervisor::new(store, Box::new(RealOpener)).unwrap();
        let (core, join) = spawn_with_devices(
            sup,
            Box::new(FakeEnumerator::new(vec![named_device("Soundcard")])),
            Box::new(|_| Box::new(NullBackend::new(48_000))),
            vec![reg.clone()],
        );
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            let (tx, rx) = oneshot::channel();
            core.commands.send(Command::ListDevices { reply: tx }).unwrap();
            let devices = rx.await.unwrap();
            assert!(
                devices.iter().any(|d| d.id == reg.id && d.label == "Rooftop"),
                "registered rtl_tcp endpoint missing from ListDevices"
            );
            // The enumerated soundcard is still present (registered devices are
            // additive, not a replacement).
            assert!(devices.iter().any(|d| matches!(&d.id, DeviceId::AlsaCard { .. })));
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    /// An enumerator that panics on every scan, standing in for the CoreAudio
    /// backend blowing up under the App Sandbox during a USB device scan.
    struct PanickingEnumerator;
    impl DeviceEnumerator for PanickingEnumerator {
        fn enumerate(&self) -> Vec<DeviceDescriptor> {
            panic!("device enumeration exploded");
        }
    }

    // A command that panics deep in the handler (here, device enumeration) must
    // not unwind the whole core thread: the live feed survives, and the scan
    // itself degrades to an (empty) reply rather than a dropped one. A dropped
    // reply reads to the gRPC caller as "daemon stopped responding"; catching the
    // panic at the ListDevices boundary keeps the RPC answering.
    #[test]
    fn panicking_command_preserves_the_core_thread() {
        let (core, join) = spawn_core(
            Box::new(PanickingEnumerator),
            Box::new(|_| Box::new(NullBackend::new(48_000))),
        );
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            // The panicking ListDevices still answers: the caller gets an empty
            // list, not a dropped reply that would surface as an "unavailable" RPC.
            let (tx, rx) = oneshot::channel();
            core.commands.send(Command::ListDevices { reply: tx }).unwrap();
            let devices = rx.await.expect("scan panic must degrade to a reply, not a drop");
            assert!(devices.is_empty(), "panicking scan should yield no devices");

            // The core is still alive: a subsequent command answers normally.
            let (tx, rx) = oneshot::channel();
            core.commands.send(Command::GetState { reply: tx }).unwrap();
            rx.await.expect("core thread survived the panic and answered GetState");
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    #[test]
    fn configure_audio_binds_distinct_rx_and_tx_devices() {
        let rx = named_device("RX");
        let tx = named_device("TX");
        let rx_id = rx.id.clone();
        let tx_id = tx.id.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![rx, tx])),
            Box::new(|_| Box::new(FileBackend::from_samples(vec![], 48_000))),
        );
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "none").await;
            // Capture on RX, playback on a DIFFERENT device (TX). Both must open.
            let ok = configure_audio_split(&core, ChannelId(0), rx_id.clone(), tx_id.clone()).await;
            assert_eq!(ok.rx_rate, 48_000);
            assert_eq!(ok.tx_rate, 48_000);
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    /// A backend that captures fine but reports no usable playback — mimics an
    /// input-only device (a microphone) chosen as, or defaulted to, the TX device.
    struct CaptureOnlyBackend {
        rate: u32,
        id: DeviceId,
    }
    impl crate::audio::backend::AudioBackend for CaptureOnlyBackend {
        fn open_capture(
            &self,
            r: u32,
        ) -> Result<crate::audio::backend::CaptureHandle, crate::audio::AudioError> {
            NullBackend::new(self.rate).open_capture(r)
        }
        fn open_playback(
            &self,
            _r: u32,
        ) -> Result<crate::audio::backend::PlaybackHandle, crate::audio::AudioError> {
            Err(crate::audio::AudioError::NoUsableFormat { device: self.id.to_canonical_string() })
        }
        fn device_id(&self) -> DeviceId {
            self.id.clone()
        }
    }

    #[test]
    fn configure_audio_binds_rx_only_when_tx_device_has_no_playback() {
        let dev = named_device("MIC");
        let dev_id = dev.id.clone();
        let backend_id = dev.id.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(move |_| Box::new(CaptureOnlyBackend { rate: 48_000, id: backend_id.clone() })),
        );
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "none").await;
            // RX = TX = the input-only device. Saving must succeed RX-only, not
            // fail the whole bind.
            let ok = configure_audio_split(&core, ChannelId(0), dev_id.clone(), dev_id.clone()).await;
            assert_eq!(ok.rx_rate, 48_000);
            assert_eq!(ok.tx_rate, 0, "no-playback TX must bind RX-only (tx_rate 0)");
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    #[test]
    fn configuring_an_afsk_channel_spawns_rx_and_emits_frames() {
        use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
        use omnimodem_dsp::mode::Modulator;
        use omnimodem_dsp::modes::afsk1200::Afsk1200Mod;
        use omnimodem_dsp::types::Frame as DspFrame;

        let ax = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("K1ABC", 1),
            digipeaters: vec![],
            info: b"core rx".to_vec(),
        };
        let f32s = Afsk1200Mod::new().modulate(&DspFrame::packet(ax.encode())).unwrap();
        let i16s: Vec<i16> = f32s.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();

        let dev = loop_device();
        let dev_id = dev.id.clone();
        let samples = i16s.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(move |_| Box::new(FileBackend::from_samples(samples.clone(), 48_000))),
        );
        let mut frames = core.frames.subscribe();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "afsk1200").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            let got = tokio::time::timeout(std::time::Duration::from_secs(10), frames.recv())
                .await
                .expect("frame within timeout")
                .unwrap();
            let FrameEvent::RxFrame { data, .. } = got;
            assert_eq!(data, ax.encode());
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    #[test]
    fn transmit_on_moded_channel_enqueues_and_completes() {
        use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};

        let dev = loop_device();
        let dev_id = dev.id.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(|_| Box::new(FileBackend::from_samples(vec![], 48_000))),
        );
        let mut tele_rx = core.telemetry.subscribe();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "afsk1200").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;

            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::ConfigurePtt {
                    id: ChannelId(0),
                    ptt: PttConfig {
                        device_id: dev_id.clone(),
                        method: PttMethod::None,
                        invert: false, tx_delay_ms: 0, tx_tail_ms: 0,
                    },
                    reply: tx,
                })
                .unwrap();
            rx.await.unwrap().unwrap();

            let ax = Ax25Frame {
                dest: Address::new("APRS", 0),
                source: Address::new("K1ABC", 1),
                digipeaters: vec![],
                info: b"moded tx".to_vec(),
            };
            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::Transmit { channel: ChannelId(0), payload: ax.encode(), reply: tx })
                .unwrap();
            // Accepted onto the queue immediately with a transmit id.
            assert_eq!(rx.await.unwrap().unwrap(), TransmitId(1));

            // The TX worker modulates and runs the cycle, ending in
            // TransmitComplete for this id.
            let done = tokio::time::timeout(std::time::Duration::from_secs(20), async {
                loop {
                    if let Ok(TelemetryEvent::TransmitComplete { transmit_id, .. }) =
                        tele_rx.recv().await
                    {
                        return transmit_id;
                    }
                }
            })
            .await
            .expect("TransmitComplete within timeout");
            assert_eq!(done, TransmitId(1));
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    async fn configure_ptt_ch(core: &CoreHandle, id: ChannelId, dev: DeviceId) {
        let (tx, rx) = oneshot::channel();
        core.commands
            .send(Command::ConfigurePtt {
                id,
                ptt: PttConfig { device_id: dev, method: PttMethod::None, invert: false, tx_delay_ms: 0, tx_tail_ms: 0 },
                reply: tx,
            })
            .unwrap();
        rx.await.unwrap().unwrap();
    }

    async fn key_ptt_call(core: &CoreHandle, channel: ChannelId) -> Result<(), CoreError> {
        let (tx, rx) = oneshot::channel();
        core.commands
            .send(Command::KeyPtt { channel, keyed: true, reply: tx })
            .unwrap();
        rx.await.unwrap()
    }

    // Reconfiguring a channel must rebuild its workers. A moded channel's TX
    // worker owns PTT, so manual KeyPtt is rejected; after re-binding the channel
    // to None the stale worker must be gone and KeyPtt allowed again. Without the
    // re-bind teardown the old worker — and its modulator — survive, which is why
    // switching modes produced no audible change.
    #[test]
    fn reconfiguring_a_channel_rebuilds_its_workers() {
        let dev = loop_device();
        let dev_id = dev.id.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(|_| Box::new(FileBackend::from_samples(vec![], 48_000))),
        );
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "afsk1200").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            configure_ptt_ch(&core, ChannelId(0), dev_id.clone()).await;
            // The moded TX worker owns PTT, so a manual key is rejected.
            assert!(key_ptt_call(&core, ChannelId(0)).await.is_err());

            // Re-bind to None: the re-bind must tear the old TX worker down.
            configure_channel(&core, ChannelId(0), "none").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            configure_ptt_ch(&core, ChannelId(0), dev_id.clone()).await;
            // Worker gone -> manual key is allowed again.
            assert!(key_ptt_call(&core, ChannelId(0)).await.is_ok());
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    /// A backend that counts `open_capture` calls and hands back a capture that
    /// stays live: the sender is retained in `keep` (not the handle's stop hook,
    /// which edition-2021 disjoint captures drop early), so the RX worker's
    /// recv_timeout just times out and loops instead of hitting EOF. Lets a test
    /// assert whether a re-bind re-opens the device or reuses the running capture.
    /// It does NOT model real USB interface exclusivity, so a re-open here always
    /// succeeds — the tests assert the reuse/re-open *decision*, not hardware
    /// claim success. Drop the retained sender to drive a capture to EOF.
    struct CountingLiveBackend {
        id: DeviceId,
        rate: u32,
        opens: Arc<std::sync::atomic::AtomicUsize>,
        keep: Arc<Mutex<Vec<std::sync::mpsc::SyncSender<crate::audio::AudioChunk>>>>,
    }
    impl crate::audio::backend::AudioBackend for CountingLiveBackend {
        fn open_capture(&self, _r: u32) -> Result<CaptureHandle, crate::audio::AudioError> {
            self.opens.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let (tx, rx) = std::sync::mpsc::sync_channel(4);
            self.keep.lock().unwrap().push(tx);
            Ok(CaptureHandle::new(rx, self.rate, || {}))
        }
        fn open_playback(&self, r: u32) -> Result<PlaybackHandle, crate::audio::AudioError> {
            NullBackend::new(self.rate).open_playback(r)
        }
        fn device_id(&self) -> DeviceId {
            self.id.clone()
        }
    }

    // GRA-372: the app's feed auto-reconnect re-sends ConfigureChannel +
    // ConfigureAudio for the *same* SDR on every stream restart. Re-opening an
    // exclusively-claimed USB SDR while the running capture still holds interface
    // 0 fails with "exclusive access" and the channel loses its feed for good, so
    // a re-bind to the same RX device + mode must reuse the live capture. A real
    // mode switch still re-opens (the worker is rebuilt against the new mode).
    #[test]
    fn rebinding_same_device_reuses_the_running_capture() {
        let dev = loop_device();
        let dev_id = dev.id.clone();
        let backend_id = dev.id.clone();
        let opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opens_factory = opens.clone();
        let keep = Arc::new(Mutex::new(Vec::new()));
        let keep_factory = keep.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(move |_| {
                Box::new(CountingLiveBackend {
                    id: backend_id.clone(),
                    rate: 48_000,
                    opens: opens_factory.clone(),
                    keep: keep_factory.clone(),
                })
            }),
        );
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            use std::sync::atomic::Ordering::Relaxed;
            configure_channel(&core, ChannelId(0), "afsk1200").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            assert_eq!(opens.load(Relaxed), 1, "first bind opens the capture");

            // The reconnect path: same device, same mode. Must NOT re-open —
            // on a real USB SDR that would fail exclusive-access.
            configure_channel(&core, ChannelId(0), "afsk1200").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            assert_eq!(opens.load(Relaxed), 1, "re-bind to same device+mode must reuse, not re-open");

            // Switching mode on the same device does re-open (worker rebuilt).
            configure_channel(&core, ChannelId(0), "psk31").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            assert_eq!(opens.load(Relaxed), 2, "mode switch re-opens the capture");
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    // The reuse gate must NOT reuse a *dead* RX worker (device removed, replay
    // drained): a finished worker has already released its device, so a re-bind
    // must re-open. This guards the `RxWorker::is_running()` liveness check.
    #[test]
    fn rebinding_after_capture_ends_reopens_the_device() {
        let dev = loop_device();
        let dev_id = dev.id.clone();
        let backend_id = dev.id.clone();
        let opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opens_factory = opens.clone();
        let keep = Arc::new(Mutex::new(Vec::new()));
        let keep_factory = keep.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(move |_| {
                Box::new(CountingLiveBackend {
                    id: backend_id.clone(),
                    rate: 48_000,
                    opens: opens_factory.clone(),
                    keep: keep_factory.clone(),
                })
            }),
        );
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            use std::sync::atomic::Ordering::Relaxed;
            configure_channel(&core, ChannelId(0), "afsk1200").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            assert_eq!(opens.load(Relaxed), 1, "first bind opens the capture");

            // Drop the retained sender: the capture disconnects, the RX worker
            // hits EOF and exits, and its JoinHandle finishes. Wait for it so the
            // liveness check sees a finished worker (not a still-parked one).
            keep.lock().unwrap().clear();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            // Same device + mode, but the worker is dead → must re-open.
            configure_channel(&core, ChannelId(0), "afsk1200").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            assert_eq!(opens.load(Relaxed), 2, "a finished worker must not be reused");
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    // GRA-279: reconfiguring a channel while a burst is on the air must stop that
    // burst promptly (the mode-switch collision) — not let it drain to the end —
    // and TX must still work afterward. A mode change from the client is a
    // ConfigureChannel+ConfigureAudio+ConfigurePtt sequence; ConfigureAudio drops
    // the running worker, which now aborts the in-flight cycle instead of playing
    // it out.
    #[test]
    fn reconfigure_mid_tx_aborts_burst_and_tx_survives() {
        let dev = loop_device();
        let dev_id = dev.id.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(|_| Box::new(FileBackend::from_samples(vec![], 48_000))),
        );
        let mut tele_rx = core.telemetry.subscribe();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "psk31").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            configure_ptt_ch(&core, ChannelId(0), dev_id.clone()).await;

            // A long PSK31 message is tens of seconds of airtime; draining it
            // would blow past the assertion below, so only a real abort passes.
            let long = "CQ CQ CQ DE NW5W NW5W NW5W ".repeat(4).into_bytes();
            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::Transmit { channel: ChannelId(0), payload: long, reply: tx })
                .unwrap();
            assert_eq!(rx.await.unwrap().unwrap(), TransmitId(1));

            // Wait until the burst is actually on the air.
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if let Ok(TelemetryEvent::TransmitStarted { transmit_id, .. }) =
                        tele_rx.recv().await
                    {
                        assert_eq!(transmit_id, TransmitId(1));
                        return;
                    }
                }
            })
            .await
            .expect("TransmitStarted within timeout");

            // Reconfigure audio mid-burst — the client's mode-switch step that
            // drops and rebuilds the worker. The in-flight burst must abort now.
            let t0 = std::time::Instant::now();
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if let Ok(TelemetryEvent::TransmitComplete { transmit_id, .. }) =
                        tele_rx.recv().await
                    {
                        assert_eq!(transmit_id, TransmitId(1));
                        return;
                    }
                }
            })
            .await
            .expect("in-flight burst never completed after reconfigure");
            assert!(
                t0.elapsed() < std::time::Duration::from_secs(3),
                "burst drained instead of aborting: {:?}",
                t0.elapsed(),
            );

            // Rebuild PTT (its driver was consumed by the aborted worker) and
            // confirm TX is not left dead — a fresh transmit completes.
            configure_ptt_ch(&core, ChannelId(0), dev_id.clone()).await;
            let (tx, rx) = oneshot::channel();
            core.commands
                .send(Command::Transmit { channel: ChannelId(0), payload: b"K".to_vec(), reply: tx })
                .unwrap();
            let id2 = rx.await.unwrap().unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                loop {
                    if let Ok(TelemetryEvent::TransmitComplete { transmit_id, .. }) =
                        tele_rx.recv().await
                    {
                        if transmit_id == id2 {
                            return;
                        }
                    }
                }
            })
            .await
            .expect("post-reconfigure transmit never completed");
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }

    // A channel restored from persistence must come up fully live (audio bound,
    // workers spawned) so TX works right after a restart — not only after the
    // operator reconfigures. Regression for "unknown channel" on TX post-restart.
    #[test]
    fn restores_live_pipeline_for_persisted_channels_on_startup() {
        let dev = loop_device();
        let dev_id = dev.id.clone();
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_channel(&crate::supervisor::channel::ChannelConfig {
                id: ChannelId(0),
                name: "restored".into(),
                mode: "psk31".into(),
                device_id: dev_id.clone(),
                sample_rate: 48_000,
                fanout: 1,
                tx_device_id: dev_id.clone(),
                tx_sample_rate: 0,
                ptt: Some(PttConfig {
                    device_id: dev_id.clone(),
                    method: PttMethod::None,
                    invert: false, tx_delay_ms: 0, tx_tail_ms: 0,
                }),
                rsid_tx: false,
                rsid_rx: false,
            })
            .unwrap();
        let sup = Supervisor::new(store, Box::new(RealOpener)).unwrap();
        let (core, join) = spawn(
            sup,
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(|_| Box::new(FileBackend::from_samples(vec![], 48_000))),
        );
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            // Startup restore must have rebuilt the live audio binding, so a
            // TX-lease acquire succeeds instead of failing with UnknownChannel.
            let (tx, rx) = oneshot::channel();
            core.commands.send(Command::AcquireTxLease { channel: ChannelId(0), reply: tx }).unwrap();
            assert!(
                rx.await.unwrap().is_ok(),
                "restored channel must have a live audio binding after startup"
            );
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }
}
