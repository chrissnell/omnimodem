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
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinSet;

/// A running listener: where it is bound and how to stop it.
struct ListenerHandle {
    bound_addr: SocketAddr,
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

        let accept_task = tokio::spawn(accept_loop(listener, core, channel));

        let mut map = self.inner.lock().await;
        map.insert(channel, ListenerHandle { bound_addr, accept_task });
        Ok(bound_addr)
    }

    /// Stop the listener for `channel` if any. No-op if none. Aborting the accept
    /// task drops its connection `JoinSet`, which aborts every live connection —
    /// closing their sockets and dropping their broadcast subscriptions, so
    /// nothing is leaked.
    pub async fn stop(&self, channel: ChannelId) {
        let handle = { self.inner.lock().await.remove(&channel) };
        if let Some(h) = handle {
            h.accept_task.abort();
            let _ = h.accept_task.await;
        }
    }

    /// The bound address of `channel`'s listener, if running (for state/tests).
    pub async fn bound_addr(&self, channel: ChannelId) -> Option<SocketAddr> {
        self.inner.lock().await.get(&channel).map(|h| h.bound_addr)
    }
}

/// Accept connections until the task is aborted (by `stop()`); each connection
/// is bridged independently. The `JoinSet` owns the connection tasks, so when
/// this task is aborted and unwinds, dropping the `JoinSet` aborts every live
/// connection — closing their sockets and broadcast subscriptions.
async fn accept_loop(listener: TcpListener, core: CoreHandle, channel: ChannelId) {
    let mut conns: JoinSet<()> = JoinSet::new();
    loop {
        match listener.accept().await {
            Ok((sock, _peer)) => {
                conns.spawn(bridge_connection(sock, core.clone(), channel));
                // Reap finished connection tasks so the JoinSet stays bounded.
                while conns.try_join_next().is_some() {}
            }
            Err(e) => {
                // Transient per-connection accept errors (ECONNABORTED, EMFILE/
                // ENFILE fd exhaustion, EINTR) must not kill the listener: log,
                // back off briefly, and keep serving.
                tracing::warn!(channel = channel.0, error = %e, "KISS accept error; continuing");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// Bridge one TCP connection: read KISS data frames → Transmit; forward this
/// channel's RxFrames → KISS to the socket. Reader and writer run under one
/// `select!` so whichever ends first — client disconnect, write error, or this
/// task being aborted on `stop()` — drops BOTH halves together, fully closing
/// the socket (no detached writer task or fd leak).
async fn bridge_connection(sock: tokio::net::TcpStream, core: CoreHandle, channel: ChannelId) {
    let (mut rd, mut wr) = sock.into_split();
    let mut rx_frames = core.frames.subscribe();

    // Air→host: forward RxFrames for this channel as KISS data frames.
    let writer = async {
        loop {
            match rx_frames.recv().await {
                Ok(FrameEvent::RxFrame { channel: ch, data, .. }) if ch == channel => {
                    if wr.write_all(&encode_data_frame(&data)).await.is_err() {
                        break; // client gone
                    }
                }
                Ok(_) => {} // other channels: ignore
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    // LOSSLESS policy: a KISS client that can't keep up is
                    // dropped rather than silently missing a frame.
                    tracing::warn!(channel = channel.0, dropped = n, "KISS client lagged; disconnecting");
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    // Host→air: decode KISS data frames and transmit them on the channel.
    let reader = async {
        let mut decoder = KissDecoder::new();
        let mut buf = [0u8; 1024];
        loop {
            match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break, // EOF or error
                Ok(n) => {
                    for frame in decoder.push(&buf[..n]) {
                        if frame.is_data() && !frame.data.is_empty() {
                            let (tx, rx) = oneshot::channel();
                            match core.commands.try_send(Command::Transmit {
                                channel,
                                payload: frame.data,
                                reply: tx,
                            }) {
                                Ok(()) => {
                                    // The core rejecting the transmit (no TX
                                    // binding / lease / mode mismatch) is invisible
                                    // to a KISS client (no NAK), so log it.
                                    if let Ok(Err(e)) = rx.await {
                                        tracing::warn!(channel = channel.0, error = %e, "KISS transmit rejected");
                                    }
                                }
                                Err(_) => {
                                    // Core queue full: drop the frame (host retries
                                    // — AX.25 is best-effort), but log it.
                                    tracing::warn!(channel = channel.0, "core command queue full; dropped KISS frame");
                                }
                            }
                        }
                        // Parameter/exit commands (TXDELAY/P/SlotTime/Return): ignored.
                    }
                }
            }
        }
    };

    tokio::select! {
        _ = writer => {}
        _ = reader => {}
    }
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
            .send(FrameEvent::RxFrame { channel: ChannelId(0), data: payload.clone(), image: None, timestamp_ns: 0 })
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
            .send(FrameEvent::RxFrame { channel: ChannelId(7), data: vec![1, 2, 3], image: None, timestamp_ns: 0 })
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

    #[tokio::test]
    async fn stop_closes_a_connected_client_socket() {
        // Regression guard: stopping a listener must tear down a live connection
        // (no leaked writer task / open socket). A connected client must see EOF.
        let (core, _cmd_rx) = fake_core();
        let reg = KissRegistry::default();
        let addr = reg.start(core.clone(), ChannelId(0), "127.0.0.1:0").await.unwrap();

        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await; // let the bridge spin up

        reg.stop(ChannelId(0)).await;

        let mut buf = [0u8; 8];
        let n = tokio::time::timeout(Duration::from_secs(2), client.read(&mut buf))
            .await
            .expect("client socket did not reach EOF after stop")
            .unwrap();
        assert_eq!(n, 0, "expected EOF after listener stop, got {n} bytes");
    }
}
