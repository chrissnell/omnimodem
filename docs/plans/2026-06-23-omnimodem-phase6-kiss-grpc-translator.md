# Phase 6 — KISS↔gRPC Translator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a gRPC client turn a packet channel (AFSK 1200 AX.25) into a KISS TNC by calling `ConfigureKissListener(channel, bind_addr)`, which spins up an in-daemon KISS-over-TCP listener that bridges legacy TNC apps (Direwolf, APRX, pat, Xastir, YAAC) to omnimodem with no separate process.

**Architecture:** The KISS bridge lives on the **async control edge** (a tokio task next to the gRPC server), **not** in the synchronous DSP core. It is a first-class internal consumer of the existing public spine — `CoreHandle.commands` (`Command::Transmit`) for host→air and `CoreHandle.frames.subscribe()` (`FrameEvent::RxFrame`) for air→host — so it adds **zero** code to the sample path and keeps the DSP core clean, which is exactly what the design doc meant by "KISS stays out of the core." A `ConfigureKissListener` RPC starts/stops one listener per channel; `ControlService` owns a small registry of running listeners keyed by channel.

**Tech Stack:** Rust, tonic/prost (gRPC), tokio (`net` + `sync` already enabled in the workspace), the existing `omnimodemd` core command/event bus.

**Design-doc reconciliation:** The design (§"Phase 5", resolved decision #5) put KISS "out of the core" as "a separate external KISS↔gRPC translator process." This plan keeps the *spirit* (no DSP-core coupling; pure client of the gRPC spine) but co-locates the translator inside the daemon and configures it over gRPC — answering the question "could a gRPC call configure/invoke a KISS listener?" with **yes**. The bridge only ever calls `Transmit` and reads `RxFrame`, the same surface a separate process would use, so it can be lifted into its own binary later with no core changes if desired.

---

## Background: the KISS protocol (what the codec must implement)

KISS (Keep It Simple, Stupid) is the de-facto host↔TNC framing. One special byte delimits frames and two-byte escapes keep it binary-clean:

| Name | Byte | Meaning |
|---|---|---|
| FEND | `0xC0` | Frame end (delimiter, both sides) |
| FESC | `0xDB` | Frame escape |
| TFEND | `0xDC` | Transposed FEND (follows FESC) |
| TFESC | `0xDD` | Transposed FESC (follows FESC) |

A frame on the wire is: `FEND, <command byte>, <escaped data...>, FEND`. The **command byte** is `(port << 4) | command`. Command `0x0` = **data frame** (the data is a complete AX.25 frame to transmit, or one just received). Commands `0x1..0x6` are TNC parameters (TXDELAY, P-persistence, SlotTime, TXtail, FullDuplex, SetHardware); command `0xF` (byte `0xFF`) is "exit KISS mode." Escaping inside the data: `0xC0 → 0xDB 0xDC`, `0xDB → 0xDB 0xDD`.

For v1 we **transmit** every received data frame (command low-nibble `0x0`) on the bound channel, **encode** every off-air `RxFrame` back to the host as a data frame (command byte `0x00`), and **accept-and-ignore** the parameter/exit commands (logged, not applied — AFSK timing is handled by the TX worker / PTT sequence already). The port nibble is ignored (one listener serves one channel).

---

## File Structure

- `crates/omnimodemd/src/kiss/mod.rs` — module root; re-exports `codec` + `listener`.
- `crates/omnimodemd/src/kiss/codec.rs` — pure KISS framing: `encode_data_frame`, `KissDecoder` (streaming), `KissFrame`. No I/O, fully unit-tested with known-answer vectors.
- `crates/omnimodemd/src/kiss/listener.rs` — the tokio bridge: bind a `TcpListener`, accept clients, host→air (`KissFrame` data → `Command::Transmit`), air→host (`FrameEvent::RxFrame` → KISS bytes), plus the start/stop `KissRegistry`.
- `proto/omnimodem.proto` — add `ConfigureKissListener` RPC + request/response (additive, per `proto/VERSIONING.md`).
- `crates/omnimodemd/src/grpc/service.rs` — `ControlService` gains a `KissRegistry` field and the `configure_kiss_listener` handler.
- `crates/omnimodemd/src/lib.rs` — add `pub mod kiss;` (no behavioral change; `ControlService::new` already the single construction site).

Convention note: existing phase plans live in `docs/plans/`; this plan follows that location.

---

### Task 1: KISS codec — encode

**Files:**
- Create: `crates/omnimodemd/src/kiss/mod.rs`
- Create: `crates/omnimodemd/src/kiss/codec.rs`
- Modify: `crates/omnimodemd/src/lib.rs` (add `pub mod kiss;`)
- Test: `crates/omnimodemd/src/kiss/codec.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Create the module root**

`crates/omnimodemd/src/kiss/mod.rs`:

```rust
//! KISS↔gRPC bridge. `codec` is pure framing (no I/O); `listener` is the tokio
//! TCP bridge that turns a packet channel into a KISS TNC. Lives on the async
//! control edge — it only uses the public Command/FrameEvent spine, never the
//! synchronous DSP core.

pub mod codec;
pub mod listener;
```

And in `crates/omnimodemd/src/lib.rs`, add alongside the other `pub mod` lines:

```rust
pub mod kiss;
```

(Add `pub mod listener;` now even though `listener.rs` is created in Task 3 — to keep this step compiling, create `listener.rs` as an empty file `// implemented in Task 3` for now, or defer the `pub mod listener;` line to Task 3. Choose deferring: in this task `mod.rs` should contain only `pub mod codec;`, and Task 3 adds `pub mod listener;`.)

So for **this** task, `mod.rs` is just:

```rust
//! KISS↔gRPC bridge. `codec` is pure framing (no I/O).
pub mod codec;
```

- [ ] **Step 2: Write the failing encode test**

Create `crates/omnimodemd/src/kiss/codec.rs` with only the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_plain_data_frame() {
        // FEND, cmd=0x00 (data, port 0), payload, FEND
        assert_eq!(encode_data_frame(&[0x11, 0x22, 0x33]), vec![0xC0, 0x00, 0x11, 0x22, 0x33, 0xC0]);
    }

    #[test]
    fn escapes_fend_and_fesc_in_payload() {
        assert_eq!(encode_data_frame(&[0xC0]), vec![0xC0, 0x00, 0xDB, 0xDC, 0xC0]);
        assert_eq!(encode_data_frame(&[0xDB]), vec![0xC0, 0x00, 0xDB, 0xDD, 0xC0]);
        assert_eq!(
            encode_data_frame(&[0xDB, 0xC0]),
            vec![0xC0, 0x00, 0xDB, 0xDD, 0xDB, 0xDC, 0xC0]
        );
    }

    #[test]
    fn encodes_an_empty_payload() {
        assert_eq!(encode_data_frame(&[]), vec![0xC0, 0x00, 0xC0]);
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `source /tmp/buildenv.sh 2>/dev/null; cargo test -p omnimodemd kiss::codec 2>&1 | tail -15`
Expected: FAIL — `cannot find function encode_data_frame`.
(Note: this repo needs ALSA + `protoc` to build. If a build env script exists from a prior session source it; otherwise the toolchain must provide `libasound2-dev` / `pkg-config` / `protobuf-compiler`. See "Build prerequisites" at the end of this plan.)

- [ ] **Step 4: Implement `encode_data_frame`**

At the top of `codec.rs` (above the tests):

```rust
//! Pure KISS framing — no I/O. See the protocol table in the Phase 6 plan.

pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;

/// KISS command for a data frame on port 0: `(port << 4) | cmd` = `0x00`.
const CMD_DATA: u8 = 0x00;

/// Encode `payload` (a complete AX.25 frame) as a KISS data frame, ready to
/// write to a host socket: `FEND, 0x00, <escaped payload>, FEND`.
pub fn encode_data_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.push(FEND);
    out.push(CMD_DATA);
    for &b in payload {
        match b {
            FEND => out.extend_from_slice(&[FESC, TFEND]),
            FESC => out.extend_from_slice(&[FESC, TFESC]),
            other => out.push(other),
        }
    }
    out.push(FEND);
    out
}
```

- [ ] **Step 5: Run the encode tests**

Run: `source /tmp/buildenv.sh 2>/dev/null; cargo test -p omnimodemd kiss::codec 2>&1 | tail -15`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/omnimodemd/src/kiss/mod.rs crates/omnimodemd/src/kiss/codec.rs crates/omnimodemd/src/lib.rs
git commit -m "kiss: pure KISS data-frame encoder with escape handling"
```

---

### Task 2: KISS codec — streaming decode

**Files:**
- Modify: `crates/omnimodemd/src/kiss/codec.rs`
- Test: `crates/omnimodemd/src/kiss/codec.rs` (tests module)

- [ ] **Step 1: Write the failing decode tests**

Add to the tests module:

```rust
    #[test]
    fn decodes_a_single_frame_across_chunked_input() {
        let mut d = KissDecoder::new();
        // FEND, cmd=0x00, [0x11,0x22], FEND — fed in two chunks.
        let mut frames = d.push(&[0xC0, 0x00, 0x11]);
        assert!(frames.is_empty());
        frames = d.push(&[0x22, 0xC0]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].port, 0);
        assert_eq!(frames[0].cmd, 0x0);
        assert_eq!(frames[0].data, vec![0x11, 0x22]);
    }

    #[test]
    fn unescapes_transposed_bytes() {
        let mut d = KissDecoder::new();
        let frames = d.push(&[0xC0, 0x00, 0xDB, 0xDC, 0xDB, 0xDD, 0xC0]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, vec![0xC0, 0xDB]);
    }

    #[test]
    fn ignores_empty_frames_and_back_to_back_fends() {
        let mut d = KissDecoder::new();
        // Leading/duplicate FENDs are delimiters, not zero-length frames.
        let frames = d.push(&[0xC0, 0xC0, 0x00, 0x41, 0xC0]);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, vec![0x41]);
    }

    #[test]
    fn round_trips_encode_then_decode() {
        let payload = vec![0x00, 0xC0, 0xDB, 0xFF, 0x7E];
        let wire = encode_data_frame(&payload);
        let mut d = KissDecoder::new();
        let frames = d.push(&wire);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, payload);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `source /tmp/buildenv.sh 2>/dev/null; cargo test -p omnimodemd kiss::codec 2>&1 | tail -15`
Expected: FAIL — `cannot find type KissDecoder`.

- [ ] **Step 3: Implement `KissFrame` + `KissDecoder`**

Add to `codec.rs` (above the tests):

```rust
/// A decoded KISS frame: the split command byte and the unescaped data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KissFrame {
    pub port: u8,
    pub cmd: u8,
    pub data: Vec<u8>,
}

impl KissFrame {
    /// True if this is a data frame (command low-nibble 0) — the only kind we
    /// transmit. Parameter/exit commands return false and are ignored.
    pub fn is_data(&self) -> bool {
        self.cmd == 0x0
    }
}

/// Streaming KISS decoder. Feed arbitrary byte chunks; get back any frames that
/// completed. Holds partial-frame and escape state across calls.
#[derive(Default)]
pub struct KissDecoder {
    buf: Vec<u8>,
    in_frame: bool,
    escaped: bool,
}

impl KissDecoder {
    pub fn new() -> Self {
        KissDecoder::default()
    }

    /// Feed bytes; return every frame that completed in this chunk.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<KissFrame> {
        let mut out = Vec::new();
        for &b in bytes {
            match b {
                FEND => {
                    if self.in_frame && !self.buf.is_empty() {
                        if let Some(f) = self.take_frame() {
                            out.push(f);
                        }
                    }
                    // Either way a FEND resets to "between frames".
                    self.buf.clear();
                    self.escaped = false;
                    self.in_frame = true;
                }
                FESC if self.in_frame => self.escaped = true,
                _ if self.in_frame => {
                    let byte = if self.escaped {
                        self.escaped = false;
                        match b {
                            TFEND => FEND,
                            TFESC => FESC,
                            other => other, // tolerate malformed escape
                        }
                    } else {
                        b
                    };
                    self.buf.push(byte);
                }
                _ => {} // bytes before the first FEND are noise
            }
        }
        out
    }

    /// Split the accumulated bytes into command + data. Caller guarantees
    /// `self.buf` is non-empty.
    fn take_frame(&mut self) -> Option<KissFrame> {
        let cmd_byte = *self.buf.first()?;
        let data = self.buf[1..].to_vec();
        Some(KissFrame {
            port: cmd_byte >> 4,
            cmd: cmd_byte & 0x0F,
            data,
        })
    }
}
```

- [ ] **Step 4: Run the decode tests**

Run: `source /tmp/buildenv.sh 2>/dev/null; cargo test -p omnimodemd kiss::codec 2>&1 | tail -15`
Expected: PASS (7 tests total).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/kiss/codec.rs
git commit -m "kiss: streaming KISS decoder with chunk + escape handling"
```

---

### Task 3: The KISS listener task and registry

**Files:**
- Create: `crates/omnimodemd/src/kiss/listener.rs`
- Modify: `crates/omnimodemd/src/kiss/mod.rs` (add `pub mod listener;`)
- Test: `crates/omnimodemd/src/kiss/listener.rs` (inline integration-style test)

- [ ] **Step 1: Add the module line**

In `crates/omnimodemd/src/kiss/mod.rs`:

```rust
pub mod listener;
```

- [ ] **Step 2: Implement the bridge and registry**

Create `crates/omnimodemd/src/kiss/listener.rs`. This is the core of the feature — read it whole before editing.

```rust
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
                        // drop this frame (host will retry — AX.25 is best-effort).
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
```

- [ ] **Step 3: Write the failing end-to-end test**

Add an inline test that starts a registry against a real in-process core with a configured AFSK1200 channel on a file backend, connects a TCP client, and checks both directions. Mirror the core test harness in `core/mod.rs` for spinning up a core (`core::spawn(...)`) and configuring a channel + file audio. Use an ephemeral port (`"127.0.0.1:0"`).

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::FrameEvent;
    use crate::ids::ChannelId;

    // Build a minimal CoreHandle-backed harness. Reuse the same construction
    // the core integration tests use (file backend, AFSK1200 channel). If the
    // core exposes a test helper, call it; otherwise replicate the
    // ConfigureChannel + ConfigureAudio command sequence here.
    async fn harness_with_afsk_channel() -> CoreHandle { /* see core tests */ unimplemented!() }

    #[tokio::test]
    async fn rxframe_reaches_a_connected_kiss_client() {
        let core = harness_with_afsk_channel().await;
        let reg = KissRegistry::default();
        let addr = reg.start(core.clone(), ChannelId(0), "127.0.0.1:0").await.unwrap();

        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        // Give the writer task a moment to subscribe before we publish.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let payload = vec![0x82, 0xA0, 0xC0]; // arbitrary AX.25-ish bytes incl. a FEND
        core.frames
            .send(FrameEvent::RxFrame { channel: ChannelId(0), data: payload.clone(), timestamp_ns: 0 })
            .unwrap();

        use tokio::io::AsyncReadExt;
        let mut got = vec![0u8; 64];
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut got))
            .await
            .expect("no KISS frame arrived")
            .unwrap();
        assert_eq!(&got[..n], &encode_data_frame(&payload)[..]);

        reg.stop(ChannelId(0)).await;
    }

    #[tokio::test]
    async fn kiss_data_frame_from_client_triggers_a_transmit() {
        let core = harness_with_afsk_channel().await;
        let mut tele = core.telemetry.subscribe();
        let reg = KissRegistry::default();
        let addr = reg.start(core.clone(), ChannelId(0), "127.0.0.1:0").await.unwrap();

        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        use tokio::io::AsyncWriteExt;
        // A minimal AX.25 UI frame is fine; the bridge just forwards bytes.
        let ax25 = vec![0x96, 0x70, 0x9A, 0x9A, 0x9E, 0x40, 0x60]; // placeholder dest/src
        client.write_all(&encode_data_frame(&ax25)).await.unwrap();

        // Expect a TransmitStarted (or Complete) telemetry for channel 0.
        let saw_tx = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match tele.recv().await.unwrap() {
                    crate::core::event::TelemetryEvent::TransmitStarted { channel, .. }
                    | crate::core::event::TelemetryEvent::TransmitComplete { channel, .. }
                        if channel == ChannelId(0) => break true,
                    _ => continue,
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(saw_tx, "KISS data frame did not cause a Transmit");

        reg.stop(ChannelId(0)).await;
    }
}
```

Replace `harness_with_afsk_channel`'s `unimplemented!()` with the real setup. Look at `crates/omnimodemd/src/core/mod.rs` tests (around the `transmit_on_moded_channel_enqueues_and_completes` test, ~line 860) for exactly how a core is spawned with a `FileBackend` factory and an AFSK1200 channel is configured via `Command::ConfigureChannel` + `Command::ConfigureAudio` + `Command::ConfigurePtt`. Build the same here and return the `CoreHandle`. Do **not** leave `unimplemented!()` in the committed code.

- [ ] **Step 4: Run the listener tests**

Run: `source /tmp/buildenv.sh 2>/dev/null; cargo test -p omnimodemd kiss::listener 2>&1 | tail -25`
Expected: both async tests PASS. If `rxframe_reaches_a_connected_kiss_client` is flaky on the 50 ms subscribe gap, increase it — the writer task must call `core.frames.subscribe()` before the `send`, or the broadcast value is missed (broadcast only delivers to existing subscribers).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/kiss/mod.rs crates/omnimodemd/src/kiss/listener.rs
git commit -m "kiss: in-daemon KISS-over-TCP listener bridging Transmit <-> RxFrame"
```

---

### Task 4: Proto — `ConfigureKissListener`

**Files:**
- Modify: `proto/omnimodem.proto`

- [ ] **Step 1: Add the RPC and messages**

In the `service ModemControl` block (after `rpc ReleaseTxLease`, the last RPC, line ~49):

```protobuf
  // Start or stop an in-daemon KISS-over-TCP listener for a packet channel,
  // so legacy TNC apps (Direwolf, APRX, pat, Xastir) speak KISS to omnimodem.
  // The channel must be a packet mode (AFSK 1200 AX.25).
  rpc ConfigureKissListener(ConfigureKissListenerRequest) returns (ConfigureKissListenerResponse);
```

And new messages near the end of the file (after the Phase-5 messages):

```protobuf
// ---------------------------------------------------------------------------
// Phase 6: KISS<->gRPC translator
// ---------------------------------------------------------------------------

message ConfigureKissListenerRequest {
  uint32 channel = 1;
  string bind_addr = 2;   // "host:port", e.g. "127.0.0.1:8001"; ":0" picks a port
  bool enable = 3;        // true = start/replace; false = stop (bind_addr ignored)
}

message ConfigureKissListenerResponse {
  string bound_addr = 1;  // actual bound "host:port" when enabled (empty when stopped)
  bool active = 2;        // whether a listener is now running for the channel
}
```

- [ ] **Step 2: Verify the proto compiles**

Run: `source /tmp/buildenv.sh 2>/dev/null; cargo build -p omnimodemd 2>&1 | tail -15`
Expected: builds; prost generates `proto::ConfigureKissListenerRequest/Response` and the service-trait method `configure_kiss_listener` (which will be unimplemented until Task 5 — a missing-trait-method error here is expected and resolved in Task 5; if it blocks the build, do Task 5's handler in the same commit).

- [ ] **Step 3: Commit**

```bash
git add proto/omnimodem.proto
git commit -m "proto: add ConfigureKissListener RPC for the KISS bridge"
```

---

### Task 5: gRPC handler + registry on `ControlService`

**Files:**
- Modify: `crates/omnimodemd/src/grpc/service.rs:14-33` (struct + `new`) and the `impl ModemControl` block
- Test: covered by Task 3's listener tests + a handler smoke test here

- [ ] **Step 1: Give `ControlService` a registry**

Replace the struct and `new` (lines 14-23):

```rust
/// Shared gRPC service state: a handle to the sync core plus the KISS listener
/// registry (async-edge only; not part of the DSP core).
#[derive(Clone)]
pub struct ControlService {
    pub(crate) core: CoreHandle,
    pub(crate) kiss: crate::kiss::listener::KissRegistry,
}

impl ControlService {
    pub fn new(core: CoreHandle) -> Self {
        ControlService { core, kiss: crate::kiss::listener::KissRegistry::default() }
    }
```

(All four `ControlService::new(core)` call sites in `lib.rs`/`main.rs` are unchanged — the registry is created internally.)

- [ ] **Step 2: Implement the handler**

Add to `impl ModemControl for ControlService`:

```rust
    async fn configure_kiss_listener(
        &self,
        request: Request<proto::ConfigureKissListenerRequest>,
    ) -> Result<Response<proto::ConfigureKissListenerResponse>, Status> {
        let req = request.into_inner();
        let channel = ChannelId(req.channel);

        if !req.enable {
            self.kiss.stop(channel).await;
            return Ok(Response::new(proto::ConfigureKissListenerResponse {
                bound_addr: String::new(),
                active: false,
            }));
        }

        // Validate: the channel must exist and be a packet mode. Query state.
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::GetState { reply: tx })?;
        let snap = rx.await.map_err(|_| Status::unavailable("core dropped reply"))?;
        let ch = snap
            .channels
            .iter()
            .find(|c| c.id == channel)
            .ok_or_else(|| Status::not_found(format!("unknown channel {}", req.channel)))?;
        if !is_packet_mode(&ch.mode) {
            return Err(Status::failed_precondition(format!(
                "channel {} mode '{}' is not a packet mode; KISS needs AFSK 1200 (AX.25)",
                req.channel, ch.mode
            )));
        }

        if req.bind_addr.is_empty() {
            return Err(Status::invalid_argument("bind_addr must be set when enable=true"));
        }
        let bound = self
            .kiss
            .start(self.core.clone(), channel, &req.bind_addr)
            .await
            .map_err(|e| match e {
                crate::kiss::listener::KissError::Bind(io) => {
                    Status::failed_precondition(format!("bind {}: {}", req.bind_addr, io))
                }
            })?;
        Ok(Response::new(proto::ConfigureKissListenerResponse {
            bound_addr: bound.to_string(),
            active: true,
        }))
    }
```

Add a small free helper at the bottom of `service.rs`:

```rust
/// KISS only makes sense for AX.25 packet modes. Today that is AFSK 1200.
fn is_packet_mode(mode: &str) -> bool {
    matches!(mode, "afsk1200")
}
```

(`ModemSnapshot.channels` items expose `id` and `mode` — confirm the field names against `crate::supervisor::ModemSnapshot` / `ChannelState`; the snapshot used by `GetState` is built in `supervisor::snapshot`. If the snapshot exposes `mode` as the parsed `ModeConfig` rather than the string, match on that instead — adjust `is_packet_mode` to take the snapshot's type. Verify before writing the test.)

- [ ] **Step 3: Write a handler smoke test**

If `crates/omnimodemd/tests/` or a `grpc` test module spins up the service in-process, add a test that calls `configure_kiss_listener` with `enable=true` on an AFSK1200 channel and asserts `active == true` and a non-empty `bound_addr`, then `enable=false` returns `active == false`. Otherwise rely on Task 3's listener tests plus a unit test of `is_packet_mode`:

```rust
#[test]
fn only_afsk_is_a_packet_mode() {
    assert!(is_packet_mode("afsk1200"));
    assert!(!is_packet_mode("ft8"));
    assert!(!is_packet_mode("none"));
}
```

- [ ] **Step 4: Build and test**

Run: `source /tmp/buildenv.sh 2>/dev/null; cargo test -p omnimodemd kiss 2>&1 | tail -20 && cargo build -p omnimodemd 2>&1 | tail -5`
Expected: PASS + clean build (the `ModemControl` trait is now fully implemented).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/grpc/service.rs
git commit -m "grpc: ConfigureKissListener handler + per-channel KISS registry"
```

---

### Task 6: Full build, clippy, regression sweep

**Files:** none (verification)

- [ ] **Step 1: Whole-crate build + clippy**

Run: `source /tmp/buildenv.sh 2>/dev/null; cargo clippy -p omnimodemd --all-targets 2>&1 | tail -30`
Expected: no errors. Resolve any unused-import warnings from the new module.

- [ ] **Step 2: Full test suite**

Run: `source /tmp/buildenv.sh 2>/dev/null; cargo test -p omnimodemd 2>&1 | tail -30`
Expected: all pass, including `kiss::codec` (7), `kiss::listener` (2), and the unchanged Phase 1-5 suites.

- [ ] **Step 3: Manual smoke (optional, documents real use)**

With a real or file-backed AFSK1200 channel configured, call the RPC (via your gRPC client) `ConfigureKissListener{channel:0, bind_addr:"127.0.0.1:8001", enable:true}`, then point Direwolf at it: in `direwolf.conf` set `KISSPORT 8001` style or run `kissattach`/an app configured for a KISS TCP host of `127.0.0.1:8001`. Send an APRS beacon; confirm it keys PTT and transmits; confirm an off-air packet appears in the app. (This needs hardware or a loopback file path; it is not a CI gate.)

- [ ] **Step 4: Commit any fixups**

```bash
git add -A
git commit -m "kiss bridge: clippy + test fixups"
```

---

## Build prerequisites (read before Task 1)

This crate links ALSA (via `cpal`) and compiles the proto with `protoc`, so a bare environment cannot build it. Ensure available (root or a userspace prefix both work):

- `libasound2-dev` + `pkg-config` (provides `alsa.pc` + `libasound.so`).
- `protobuf-compiler` (provides `protoc`); set `PROTOC` to its path if not on `PATH`.

If installed into a userspace prefix, export `PKG_CONFIG_PATH`, `PKG_CONFIG_SYSROOT_DIR`, `RUSTFLAGS=-L<libdir>`, `LD_LIBRARY_PATH`, and `PROTOC` before any `cargo` command (a `source /tmp/buildenv.sh` referenced in the steps is one way to carry these).

---

## Self-Review

- **Spec coverage:** "a gRPC call configures/invokes a KISS listener" — Task 4 (`ConfigureKissListener` RPC) + Task 5 (handler that starts/stops). KISS framing correctness — Tasks 1-2 (encode/decode KAT + round-trip). Host→air — Task 3 (`bridge_connection` read half → `Command::Transmit`). Air→host — Task 3 (writer half subscribing to `FrameEvent::RxFrame`, filtered by channel, KISS-encoded). Start/stop/replace lifecycle — Task 3 (`KissRegistry::start`/`stop`, idempotent replace, JoinSet teardown). Mode validation (KISS = packet only) — Task 5 (`is_packet_mode`). Out-of-DSP-core constraint — architecture section + Task 3 (uses only `core.commands` / `core.frames`).
- **Placeholder scan:** the `harness_with_afsk_channel` `unimplemented!()` in Task 3 Step 3 is explicitly called out as must-replace, with a precise pointer to the core test it mirrors — not a silent TODO. The Task 5 note to confirm `ModemSnapshot` field names is a verification instruction, not a deferred design decision (both branches are spelled out). No "add error handling"/"TBD" left.
- **Type consistency:** `KissRegistry` (`start`/`stop`/`bound_addr`), `KissFrame` (`port`/`cmd`/`data`/`is_data`), `KissDecoder` (`new`/`push`), `encode_data_frame`, and `KissError::Bind` are used with identical signatures everywhere they appear (Tasks 1-5). The RPC message field names (`channel`/`bind_addr`/`enable`; `bound_addr`/`active`) match between proto (Task 4) and handler (Task 5).
