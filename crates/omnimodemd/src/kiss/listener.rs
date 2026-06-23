//! The KISS-over-TCP bridge: one listener per channel, started/stopped over
//! gRPC. Host→air decodes KISS data frames and issues `Command::Transmit`;
//! air→host KISS-encodes `FrameEvent::RxFrame` for the bound channel. Runs
//! entirely on the async edge; touches only the public core spine.

use crate::core::command::Command;
use crate::core::event::FrameEvent;
use crate::core::CoreHandle;
use crate::ids::ChannelId;
use crate::kiss::codec::{encode_data_frame, KissDecoder};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex, Notify};
use tokio::task::JoinSet;

/// A running listener: where it is bound and how to stop it.
struct ListenerHandle {
    bound_addr: SocketAddr,
    shutdown: Arc<Notify>,
    accept_task: tokio::task::JoinHandle<()>,
}

/// Registry of active KISS listeners, one per channel. Cloneable (Arc inside).
#[derive(Clone, Default)]
pub struct KissRegistry {
    inner: Arc<Mutex<HashMap<ChannelId, ListenerHandle>>>,
}

/// Why a `start` failed, mapped to a gRPC status by the caller.
#[derive(Debug)]
pub enum KissError {
    Bind(std::io::Error),
}

impl KissRegistry {
    /// Start (or replace) the listener for `channel`, bound to `bind_addr`
    /// (e.g. "127.0.0.1:8001"; ":0" picks an ephemeral port). Returns the
    /// actual bound address. Replacing stops the previous listener first.
    pub async fn start(
        &self,
        core: CoreHandle,
        channel: ChannelId,
        bind_addr: &str,
    ) -> Result<SocketAddr, KissError> {
        self.stop(channel).await; // idempotent replace

        let listener = TcpListener::bind(bind_addr).await.map_err(KissError::Bind)?;
        let bound_addr = listener.local_addr().map_err(KissError::Bind)?;
        let shutdown = Arc::new(Notify::new());

        let accept_task = tokio::spawn(accept_loop(listener, core, channel, shutdown.clone()));

        let mut map = self.inner.lock().await;
        map.insert(channel, ListenerHandle { bound_addr, shutdown, accept_task });
        Ok(bound_addr)
    }

    /// Stop the listener for `channel` if any. No-op if none. Aborts the accept
    /// task (which cancels its connection tasks via the shared JoinSet).
    pub async fn stop(&self, channel: ChannelId) {
        let handle = { self.inner.lock().await.remove(&channel) };
        if let Some(h) = handle {
            h.shutdown.notify_waiters();
            h.accept_task.abort();
            let _ = h.accept_task.await;
        }
    }

    /// The bound address of `channel`'s listener, if running (for state/tests).
    pub async fn bound_addr(&self, channel: ChannelId) -> Option<SocketAddr> {
        self.inner.lock().await.get(&channel).map(|h| h.bound_addr)
    }
}

/// Accept connections until shutdown; each connection is bridged independently.
/// A shared `JoinSet` means aborting this task tears down all live connections.
async fn accept_loop(
    listener: TcpListener,
    core: CoreHandle,
    channel: ChannelId,
    shutdown: Arc<Notify>,
) {
    let mut conns: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((sock, _peer)) => {
                        conns.spawn(bridge_connection(sock, core.clone(), channel));
                    }
                    Err(_) => break, // listener broken: stop the loop
                }
            }
        }
    }
    conns.shutdown().await; // abort all live connection bridges
}

/// Bridge one TCP connection: read KISS data frames → Transmit; forward this
/// channel's RxFrames → KISS to the socket. Ends when the socket closes.
async fn bridge_connection(sock: tokio::net::TcpStream, core: CoreHandle, channel: ChannelId) {
    let (mut rd, mut wr) = sock.into_split();
    let mut rx_frames = core.frames.subscribe();

    // Air→host: forward RxFrames for this channel as KISS data frames.
    let writer = tokio::spawn(async move {
        loop {
            match rx_frames.recv().await {
                Ok(FrameEvent::RxFrame { channel: ch, data, .. }) if ch == channel => {
                    if wr.write_all(&encode_data_frame(&data)).await.is_err() {
                        break; // client gone
                    }
                }
                Ok(_) => {} // other channels: ignore
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // LOSSLESS policy: a KISS client that can't keep up is
                    // dropped rather than silently missing a frame.
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Host→air: decode KISS data frames and transmit them on the channel.
    let mut decoder = KissDecoder::new();
    let mut buf = [0u8; 1024];
    loop {
        match rd.read(&mut buf).await {
            Ok(0) | Err(_) => break, // EOF or error
            Ok(n) => {
                for frame in decoder.push(&buf[..n]) {
                    if frame.is_data() && !frame.data.is_empty() {
                        let (tx, rx) = oneshot::channel();
                        // try_send is non-blocking; if the core queue is full we
                        // drop this frame (host retries — AX.25 is best-effort).
                        if core
                            .commands
                            .try_send(Command::Transmit { channel, payload: frame.data, reply: tx })
                            .is_ok()
                        {
                            let _ = rx.await; // await acceptance onto the TX queue
                        }
                    }
                    // Parameter/exit commands (TXDELAY/P/SlotTime/Return): ignored.
                }
            }
        }
    }
    writer.abort();
    let _ = writer.await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A hand-built `CoreHandle` plus the command `Receiver`, so a test can both
    /// publish RxFrames (air→host) and observe Transmit commands (host→air)
    /// without spinning the whole DSP core — the bridge only uses this spine.
    fn fake_core() -> (CoreHandle, std::sync::mpsc::Receiver<Command>) {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::sync_channel(16);
        let (frame_tx, _) = tokio::sync::broadcast::channel(16);
        let (tele_tx, _) = tokio::sync::broadcast::channel(16);
        (CoreHandle { commands: cmd_tx, frames: frame_tx, telemetry: tele_tx }, cmd_rx)
    }

    #[tokio::test]
    async fn rxframe_reaches_a_connected_kiss_client() {
        let (core, _cmd_rx) = fake_core();
        let reg = KissRegistry::default();
        let addr = reg.start(core.clone(), ChannelId(0), "127.0.0.1:0").await.unwrap();

        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        // Let the connection's writer task subscribe before we publish (broadcast
        // only delivers to subscribers that exist at send time).
        tokio::time::sleep(Duration::from_millis(100)).await;

        let payload = vec![0x82, 0xA0, 0xC0]; // arbitrary bytes incl. a FEND to exercise escaping
        core.frames
            .send(FrameEvent::RxFrame { channel: ChannelId(0), data: payload.clone(), timestamp_ns: 0 })
            .unwrap();

        let mut got = vec![0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(2), client.read(&mut got))
            .await
            .expect("no KISS frame arrived")
            .unwrap();
        assert_eq!(&got[..n], &encode_data_frame(&payload)[..]);

        reg.stop(ChannelId(0)).await;
    }

    #[tokio::test]
    async fn other_channels_rxframes_are_not_forwarded() {
        let (core, _cmd_rx) = fake_core();
        let reg = KissRegistry::default();
        let addr = reg.start(core.clone(), ChannelId(0), "127.0.0.1:0").await.unwrap();
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // A frame on a different channel must NOT reach this listener's client.
        core.frames
            .send(FrameEvent::RxFrame { channel: ChannelId(7), data: vec![1, 2, 3], timestamp_ns: 0 })
            .unwrap();
        let mut got = vec![0u8; 16];
        let r = tokio::time::timeout(Duration::from_millis(300), client.read(&mut got)).await;
        assert!(r.is_err(), "frame from another channel leaked to the client");

        reg.stop(ChannelId(0)).await;
    }

    #[tokio::test]
    async fn kiss_data_frame_from_client_triggers_a_transmit() {
        let (core, cmd_rx) = fake_core();
        let reg = KissRegistry::default();
        let addr = reg.start(core.clone(), ChannelId(3), "127.0.0.1:0").await.unwrap();

        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        // An AX.25-ish payload incl. a 0xC0 to confirm round-trip through escaping.
        let ax25 = vec![0x96, 0x70, 0x9A, 0xC0, 0x9E, 0x40, 0x60];
        client.write_all(&encode_data_frame(&ax25)).await.unwrap();

        // The bridge issues Command::Transmit on the bound channel with the
        // decoded payload. Receive it off the held command channel.
        let got = tokio::task::spawn_blocking(move || cmd_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap();
        match got {
            Ok(Command::Transmit { channel, payload, .. }) => {
                assert_eq!(channel, ChannelId(3));
                assert_eq!(payload, ax25);
            }
            other => panic!("expected a Transmit command, got {:?}", other.map(|_| "cmd")),
        }

        reg.stop(ChannelId(3)).await;
    }

    #[tokio::test]
    async fn parameter_frames_do_not_transmit() {
        let (core, cmd_rx) = fake_core();
        let reg = KissRegistry::default();
        let addr = reg.start(core.clone(), ChannelId(0), "127.0.0.1:0").await.unwrap();
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();

        // A TXDELAY parameter frame: FEND, cmd=0x01, value, FEND. Must be ignored.
        client.write_all(&[0xC0, 0x01, 0x32, 0xC0]).await.unwrap();

        let got = tokio::task::spawn_blocking(move || cmd_rx.recv_timeout(Duration::from_millis(400)))
            .await
            .unwrap();
        assert!(got.is_err(), "a non-data KISS frame must not cause a Transmit");

        reg.stop(ChannelId(0)).await;
    }

    #[tokio::test]
    async fn stop_is_idempotent_and_unblocks_the_port() {
        let (core, _cmd_rx) = fake_core();
        let reg = KissRegistry::default();
        let addr = reg.start(core.clone(), ChannelId(0), "127.0.0.1:0").await.unwrap();
        assert_eq!(reg.bound_addr(ChannelId(0)).await, Some(addr));
        reg.stop(ChannelId(0)).await;
        assert_eq!(reg.bound_addr(ChannelId(0)).await, None);
        reg.stop(ChannelId(0)).await; // second stop is a no-op
        // Re-binding the same explicit port now succeeds (old listener released).
        let addr2 = reg.start(core.clone(), ChannelId(0), &addr.to_string()).await.unwrap();
        assert_eq!(addr2, addr);
        reg.stop(ChannelId(0)).await;
    }
}
