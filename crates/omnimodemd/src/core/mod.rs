//! The synchronous core. Owns the Supervisor; runs on a plain `std::thread`
//! with no tokio. Drains commands, mutates state, persists, and emits events.

pub mod command;
pub mod error;
pub mod event;

use crate::ids::TransmitId;
use crate::supervisor::Supervisor;
use command::Command;
use event::{FrameEvent, TelemetryEvent};
use std::sync::mpsc::{Receiver, SyncSender};
use tokio::sync::broadcast;

/// Bounded depth of the command queue (command-side backpressure).
pub const COMMAND_QUEUE_DEPTH: usize = 64;

/// Capacity of each broadcast ring. Frames get a deeper ring because lag means
/// disconnect (we want headroom before that); telemetry can be shallow.
pub const FRAME_RING: usize = 1024;
pub const TELEMETRY_RING: usize = 256;

/// Handles the async edge keeps to talk to the core.
#[derive(Clone)]
pub struct CoreHandle {
    pub commands: SyncSender<Command>,
    pub frames: broadcast::Sender<FrameEvent>,
    pub telemetry: broadcast::Sender<TelemetryEvent>,
}

/// Spawn the core thread. Returns a handle plus the thread's `JoinHandle`.
pub fn spawn(supervisor: Supervisor) -> (CoreHandle, std::thread::JoinHandle<()>) {
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
        .spawn(move || run(supervisor, cmd_rx, frame_tx, tele_tx))
        .expect("spawn core thread");

    (handle, join)
}

/// The core loop. Blocks on `recv()`; exits on `Shutdown` or a closed channel.
fn run(
    mut supervisor: Supervisor,
    commands: Receiver<Command>,
    _frames: broadcast::Sender<FrameEvent>,
    telemetry: broadcast::Sender<TelemetryEvent>,
) {
    let mut next_tx_id: u64 = 1;
    while let Ok(cmd) = commands.recv() {
        match cmd {
            Command::ConfigureChannel { id, name, mode, reply } => {
                let res = supervisor
                    .configure_channel(id, name, mode)
                    .map_err(Into::into);
                if res.is_ok() {
                    // Lossy: a missed "configured" event is harmless — the
                    // snapshot or a later GetState reflects the same state.
                    let _ = telemetry.send(TelemetryEvent::ChannelConfigured { channel: id });
                }
                let _ = reply.send(res);
            }
            Command::Transmit { channel, payload, reply } => {
                if !supervisor.has_channel(channel) {
                    let _ = reply.send(Err(error::CoreError::UnknownChannel(channel)));
                    continue;
                }
                let tx_id = TransmitId(next_tx_id);
                next_tx_id += 1;

                // Simulate the on-air cycle: key PTT, announce start, "send",
                // announce complete, unkey. No audio or DSP exists yet.
                supervisor.ptt_mut().key(channel);
                let _ = telemetry.send(TelemetryEvent::TransmitStarted { channel, transmit_id: tx_id });
                let _ = payload; // opaque; not interpreted in Phase 1
                let _ = telemetry.send(TelemetryEvent::TransmitComplete { channel, transmit_id: tx_id });
                supervisor.ptt_mut().unkey(channel);

                let _ = reply.send(Ok(tx_id));
            }
            Command::GetState { reply } => {
                let _ = reply.send(supervisor.snapshot());
            }
            Command::Shutdown => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::command::Command;
    use crate::ids::ChannelId;
    use crate::persist::Store;
    use tokio::sync::oneshot;

    fn fresh_core() -> (CoreHandle, std::thread::JoinHandle<()>) {
        let store = Store::open_in_memory().unwrap();
        let sup = Supervisor::new(store).unwrap();
        spawn(sup)
    }

    // The core thread is sync, but oneshot replies are awaited; drive them with
    // a small current-thread runtime in the test.
    #[test]
    fn configure_then_transmit_emits_events() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (core, join) = fresh_core();
        let mut tele_rx = core.telemetry.subscribe();

        rt.block_on(async {
            // Configure a channel.
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

            // First telemetry event is ChannelConfigured.
            match tele_rx.recv().await.unwrap() {
                TelemetryEvent::ChannelConfigured { channel } => assert_eq!(channel, ChannelId(0)),
                other => panic!("expected ChannelConfigured, got {other:?}"),
            }

            // Transmit and collect the ack.
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

            // Started then Complete on telemetry.
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
            assert!(matches!(err, error::CoreError::UnknownChannel(ChannelId(9))));
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }
}
