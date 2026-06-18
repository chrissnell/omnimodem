# Omnimodem Phase 1 — Program Structure & gRPC Control Plane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the Omnimodem workspace and a fully-working gRPC control plane against a *stub* synchronous core — no DSP, no audio, no PTT hardware — proving the async-edge / sync-core architecture end-to-end.

**Architecture:** A hard line separates an async control edge (tonic + tokio gRPC handlers) from a synchronous core (plain `std::thread`). gRPC handlers validate requests and push `Command`s over a bounded `std::sync::mpsc::SyncSender` into the core; the core mutates a `Supervisor` (channels, placeholder device cache, placeholder PTT registry), persists config to SQLite keyed on a placeholder `DeviceId`, and emits events back out over two `tokio::broadcast` channels — one **lossless** class for decoded frames, one **lossy** class for telemetry. `SubscribeEvents` replays a state snapshot on subscribe, then streams live events. The default transport is a Unix domain socket with `SO_PEERCRED` peer-uid authorization; an mTLS hook is stubbed for future routable binds.

**Tech Stack:** Rust 2021, `tonic` 0.12 (gRPC) + `prost` 0.13, `tokio` 1.x, `async-stream` 0.3, `rusqlite` 0.32 (bundled SQLite), `tokio-stream` 0.1, `libc` 0.2 (`SO_PEERCRED` socket file mode), `thiserror`, `tracing` + `tracing-subscriber`.

---

## Scope

This plan covers **only Phase 1** from `docs/design/2026-06-17-omnimodem-design.md` ("Phase 1 — Program structure & gRPC control plane"). It deliberately excludes:

- Real audio backends, `DeviceId` resolution, resampling, capture fan-out (Phase 2).
- Real PTT drivers and hotplug eviction (Phase 2).
- The DSP/FEC batteries toolkit and mode framework (Phase 3+).

Everything the core does is a stub: channels carry a placeholder `mode` string, "transmit" is simulated, and the device identity is a single hard-coded `DeviceId::placeholder()`. The point of Phase 1 is to lock down the **expensive-to-retrofit** decisions — the async/sync boundary, the backpressure policy, local authz, the proto versioning policy, and the persistence keying — so later phases plug into a stable spine.

**Exit criterion (the gate this plan must satisfy):** over a real Unix domain socket, a gRPC client can (1) configure a virtual channel, (2) subscribe to events and receive a state snapshot, and (3) drive a fake "transmit" round-trip — receiving a unary ack *and* observing `TransmitStarted` + `TransmitComplete` on the event stream — with no audio devices or DSP present. Task 13 is that end-to-end test.

## File Structure

New repository layout (a Cargo workspace with one binary crate; mirrors Graywolf's workspace-with-member shape so future crates split out cleanly):

```
omnimodem/
  Cargo.toml                         workspace manifest
  proto/
    omnimodem.proto                  the ModemControl service + messages
    VERSIONING.md                    proto stability/versioning policy
  crates/
    omnimodemd/
      Cargo.toml                     binary crate manifest
      build.rs                       tonic-build codegen
      src/
        main.rs                      entrypoint: arg parse, wire core + server, shutdown
        lib.rs                       library surface (so integration tests can spawn the server)
        proto.rs                     tonic include! + package/version constants
        ids.rs                       ChannelId / TransmitId / DeviceId newtypes
        core/
          mod.rs                     sync core thread: command loop + transmit simulation
          command.rs                 Command enum (mpsc into core)
          event.rs                   FrameEvent (lossless) + TelemetryEvent (lossy)
          error.rs                   CoreError
        supervisor/
          mod.rs                     Supervisor: channels, device cache, ptt registry, snapshot()
          channel.rs                 ChannelConfig / ChannelState
          device.rs                  placeholder DeviceId cache
          ptt.rs                     placeholder PttRegistry
        persist/
          mod.rs                     SQLite Store: open / upsert_channel / load_channels
        grpc/
          mod.rs                     re-exports
          service.rs                 ModemControl impl: unary handlers
          subscribe.rs               SubscribeEvents: snapshot + dual-class fan-out
          convert.rs                 domain <-> proto conversions
        authz/
          mod.rs                     Transport enum + selection
          uds.rs                     UDS bind, socket-file mode, SO_PEERCRED interceptor
          tls.rs                     mTLS hook (Phase-1 stub)
      tests/
        unary.rs                     integration: unary RPCs over in-process UDS
        subscribe.rs                 integration: snapshot-on-subscribe + live events
        e2e.rs                       THE exit-criterion round-trip test
```

**Boundaries.** `core/` owns the sync side and knows nothing about tonic or proto. `grpc/` owns the async side and translates proto <-> domain in `convert.rs`. `supervisor/` is pure state with no I/O except through `persist/`. `authz/` is transport-only. This keeps the async/sync seam — the one thing Phase 1 exists to prove — visible and testable in isolation.

---

## Task 1: Workspace skeleton that compiles

**Files:**
- Create: `Cargo.toml`
- Create: `crates/omnimodemd/Cargo.toml`
- Create: `crates/omnimodemd/src/lib.rs`
- Create: `crates/omnimodemd/src/main.rs`
- Create: `.gitignore`

- [ ] **Step 1: Write the workspace manifest**

Create `Cargo.toml`:

```toml
[workspace]
members = ["crates/omnimodemd"]
resolver = "2"

[workspace.package]
edition = "2021"
license = "MIT"
repository = "https://github.com/chrissnell/omnimodem"

[workspace.dependencies]
tonic = "0.12"
prost = "0.13"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "sync", "signal", "time"] }
tokio-stream = { version = "0.1", features = ["net"] }
async-stream = "0.3"
rusqlite = { version = "0.32", features = ["bundled"] }
libc = "0.2"
thiserror = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[build-dependencies]
tonic-build = "0.12"
```

- [ ] **Step 2: Write the crate manifest**

Create `crates/omnimodemd/Cargo.toml`:

```toml
[package]
name = "omnimodemd"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "omnimodemd"
path = "src/main.rs"

[dependencies]
tonic.workspace = true
prost.workspace = true
tokio.workspace = true
tokio-stream.workspace = true
async-stream.workspace = true
rusqlite.workspace = true
libc.workspace = true
thiserror.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true

[build-dependencies]
tonic-build.workspace = true
```

- [ ] **Step 3: Write a placeholder lib + main so the crate builds**

Create `crates/omnimodemd/src/lib.rs`:

```rust
//! Omnimodem daemon library surface.
//!
//! The binary in `main.rs` is a thin wrapper; everything testable lives here so
//! integration tests in `tests/` can spawn the server in-process.

/// Crate version, surfaced to clients in the gRPC handshake metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
```

Create `crates/omnimodemd/src/main.rs`:

```rust
fn main() {
    println!("omnimodemd {}", omnimodemd::VERSION);
}
```

Create `.gitignore`:

```
/target
*.sqlite
*.sqlite-journal
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: success; produces `target/debug/omnimodemd`.

- [ ] **Step 5: Smoke-run the binary**

Run: `cargo run -p omnimodemd`
Expected: prints `omnimodemd 0.1.0`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/omnimodemd/Cargo.toml crates/omnimodemd/src/lib.rs crates/omnimodemd/src/main.rs .gitignore
git commit -m "Scaffold omnimodem workspace and omnimodemd crate"
```

---

## Task 2: Proto definition + codegen

The `ModemControl` service: unary command-and-control plus server-streaming `SubscribeEvents`. The vocabulary is lifted from Graywolf's `proto/graywolf.proto` (channel/frame/level/status shapes) but restructured for gRPC and a `v1` package. Frame and telemetry messages are deliberately separated because they live in different backpressure classes (Task 9).

**Files:**
- Create: `proto/omnimodem.proto`
- Create: `crates/omnimodemd/build.rs`
- Create: `crates/omnimodemd/src/proto.rs`
- Modify: `crates/omnimodemd/src/lib.rs`
- Test: `crates/omnimodemd/src/proto.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the proto**

Create `proto/omnimodem.proto`:

```proto
// Omnimodem control-plane API.
//
// Stability: package is versioned (omnimodem.v1). Within a major version,
// changes are ADDITIVE ONLY — see proto/VERSIONING.md. Field tags are never
// reused or renumbered.

syntax = "proto3";

package omnimodem.v1;

// Command-and-control plane for the Omnimodem software modem.
service ModemControl {
  // Create or update a virtual channel. Idempotent on channel id.
  rpc ConfigureChannel(ConfigureChannelRequest) returns (ConfigureChannelResponse);

  // Snapshot the current modem state (channels + status).
  rpc GetState(GetStateRequest) returns (ModemState);

  // Enqueue a frame for transmission on a channel. In Phase 1 this is
  // simulated: the core acknowledges with a transmit id and emits
  // TransmitStarted/TransmitComplete events. Returns once the frame is
  // accepted onto the channel's TX queue, not when it leaves the air.
  rpc Transmit(TransmitRequest) returns (TransmitResponse);

  // Subscribe to the event stream. The first message is always a snapshot
  // (Event.snapshot) reflecting state at subscription time; live events follow.
  rpc SubscribeEvents(SubscribeRequest) returns (stream Event);
}

// ---------------------------------------------------------------------------
// Unary request/response
// ---------------------------------------------------------------------------

message ConfigureChannelRequest {
  uint32 channel = 1;     // logical channel id; reused id updates in place
  string name = 2;        // operator-facing label
  string mode = 3;        // Phase 1 placeholder, e.g. "none"
}

message ConfigureChannelResponse {
  uint32 channel = 1;
}

message GetStateRequest {}

message TransmitRequest {
  uint32 channel = 1;
  bytes payload = 2;      // opaque frame bytes; not interpreted in Phase 1
}

message TransmitResponse {
  uint64 transmit_id = 1; // monotonically increasing per process
}

message SubscribeRequest {}

// ---------------------------------------------------------------------------
// State snapshot
// ---------------------------------------------------------------------------

message ModemState {
  repeated ChannelInfo channels = 1;
}

message ChannelInfo {
  uint32 channel = 1;
  string name = 2;
  string mode = 3;
  string device_id = 4;   // stable device identity (placeholder in Phase 1)
  bool running = 5;
}

// ---------------------------------------------------------------------------
// Event stream
// ---------------------------------------------------------------------------

message Event {
  oneof kind {
    ModemState snapshot = 1;            // always first on a fresh subscription
    ChannelConfigured channel_configured = 2;
    TransmitStarted transmit_started = 3;
    TransmitComplete transmit_complete = 4;
    RxFrame rx_frame = 5;               // LOSSLESS class
    AudioLevel audio_level = 6;         // LOSSY class
    Status status = 7;                  // LOSSY class
  }
}

message ChannelConfigured { uint32 channel = 1; }

message TransmitStarted {
  uint32 channel = 1;
  uint64 transmit_id = 2;
}

message TransmitComplete {
  uint32 channel = 1;
  uint64 transmit_id = 2;
}

// A decoded RX frame. No DSP exists in Phase 1, so this is emitted only by
// tests/tools; it is defined now to pin the LOSSLESS backpressure contract.
message RxFrame {
  uint32 channel = 1;
  bytes data = 2;
  uint64 timestamp_ns = 3;
}

message AudioLevel {
  uint32 channel = 1;
  float dbfs = 2;
}

message Status {
  uint32 channel = 1;
  uint64 tx_frames = 2;
}
```

- [ ] **Step 2: Write the build script**

Create `crates/omnimodemd/build.rs`:

```rust
fn main() {
    tonic_build::configure()
        .build_server(true)
        .build_client(true) // clients are generated for our own integration tests
        .compile_protos(&["../../proto/omnimodem.proto"], &["../../proto"])
        .expect("failed to compile omnimodem.proto");
}
```

- [ ] **Step 3: Write the proto module with version constants and a test**

Create `crates/omnimodemd/src/proto.rs`:

```rust
//! Generated gRPC types for the omnimodem.v1 package.

tonic::include_proto!("omnimodem.v1");

/// Proto package name, surfaced for handshake/debug.
pub const PACKAGE: &str = "omnimodem.v1";

/// Proto API major version. Within this major, changes are additive only.
pub const API_VERSION_MAJOR: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_types_are_constructible() {
        let req = ConfigureChannelRequest {
            channel: 0,
            name: "test".into(),
            mode: "none".into(),
        };
        assert_eq!(req.name, "test");

        // The Event oneof must carry a snapshot variant.
        let ev = Event {
            kind: Some(event::Kind::Snapshot(ModemState { channels: vec![] })),
        };
        assert!(ev.kind.is_some());
    }
}
```

- [ ] **Step 4: Expose the proto module from the crate**

In `crates/omnimodemd/src/lib.rs`, add below the `VERSION` const:

```rust
pub mod proto;
```

- [ ] **Step 5: Run the test to verify codegen works**

Run: `cargo test -p omnimodemd proto::tests::generated_types_are_constructible`
Expected: PASS (codegen ran, types exist, oneof variant names match).

- [ ] **Step 6: Commit**

```bash
git add proto/omnimodem.proto crates/omnimodemd/build.rs crates/omnimodemd/src/proto.rs crates/omnimodemd/src/lib.rs
git commit -m "Add ModemControl proto and tonic codegen"
```

---

## Task 3: Proto versioning policy

A written stability policy is a Phase-1 deliverable (design: "publish a stability/versioning policy for the proto from day one"). It is a doc, not code, but it is the contract third-party frontends rely on.

**Files:**
- Create: `proto/VERSIONING.md`

- [ ] **Step 1: Write the policy**

Create `proto/VERSIONING.md`:

```markdown
# Omnimodem gRPC API — Stability & Versioning Policy

Third-party frontends are a primary goal, so the wire contract is versioned
from day one.

## Package versioning

- The proto package carries a major version: `omnimodem.v1`.
- The major version follows semantic versioning at the API level.

## Additive-only within a major

Within a major version (`v1`), every change MUST be backward compatible:

- New messages, fields, RPCs, and enum values may be added.
- Existing field tags are NEVER reused, renumbered, or repurposed.
- Fields are NEVER removed; deprecate them (`// deprecated`) and `reserved`
  the tag if they must go.
- Enum value numbers are stable; the zero value stays `*_UNSPECIFIED` where one
  is defined.
- RPC method names and their request/response message types are stable.

## Breaking changes

A breaking change requires a new package (`omnimodem.v2`) served alongside `v1`
during a deprecation window. The major version constant
(`proto::API_VERSION_MAJOR`) is bumped in lockstep.

## Review gate

Any PR touching `proto/omnimodem.proto` must confirm in its description that the
change is additive within the current major, or that it introduces a new major.
```

- [ ] **Step 2: Commit**

```bash
git add proto/VERSIONING.md
git commit -m "Document proto versioning policy"
```

---

## Task 4: Identity newtypes

Typed ids prevent mixing a channel id with a transmit id, and pin the placeholder device identity that config is keyed on.

**Files:**
- Create: `crates/omnimodemd/src/ids.rs`
- Modify: `crates/omnimodemd/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/omnimodemd/src/ids.rs`:

```rust
//! Strongly-typed identifiers used across the core/supervisor.

/// Logical channel id (matches the proto `channel` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelId(pub u32);

/// Per-process monotonic transmit id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransmitId(pub u64);

/// Stable device identity. In Phase 1 there is exactly one placeholder value;
/// Phase 2 replaces the inner string with a real cross-platform identity
/// derived from durable USB/ALSA/serial attributes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

impl DeviceId {
    /// The single placeholder device used until Phase 2 lands real detection.
    pub fn placeholder() -> Self {
        DeviceId("virtual:0".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_is_stable() {
        assert_eq!(DeviceId::placeholder(), DeviceId::placeholder());
        assert_eq!(DeviceId::placeholder().0, "virtual:0");
    }

    #[test]
    fn channel_and_transmit_ids_are_distinct_types() {
        let c = ChannelId(1);
        let t = TransmitId(1);
        assert_eq!(c.0 as u64, t.0); // values can match...
        // ...but the types cannot be confused at compile time (compile check).
    }
}
```

- [ ] **Step 2: Wire the module in**

In `crates/omnimodemd/src/lib.rs`, add:

```rust
pub mod ids;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p omnimodemd ids::`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodemd/src/ids.rs crates/omnimodemd/src/lib.rs
git commit -m "Add ChannelId/TransmitId/DeviceId newtypes"
```

---

## Task 5: SQLite persistence store

Config is persisted to a SQLite file owned by the modem, keyed on the stable `DeviceId` (design: "Key config on the stable `DeviceId`, never on the volatile `/dev` path"). Writes stay off any DSP hot path — in Phase 1 there is no audio pump, so the core thread performs them directly; a comment records that this moves to the control edge / a dedicated thread once DSP lands.

**Files:**
- Create: `crates/omnimodemd/src/persist/mod.rs`
- Modify: `crates/omnimodemd/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/omnimodemd/src/persist/mod.rs`:

```rust
//! SQLite-backed configuration store.
//!
//! Config outlives any single gRPC client (frontends are external), so it lives
//! in a modem-owned SQLite file. Channels are keyed on the stable `DeviceId`,
//! not a volatile device path, so a device that moves nodes still binds.
//!
//! Phase 1 note: writes happen on the core thread. There is no audio pump yet,
//! so this cannot stall the sample path. When DSP lands (Phase 3), persistence
//! moves to the control edge or a dedicated writer thread per the design.

use crate::ids::{ChannelId, DeviceId};
use crate::supervisor::channel::ChannelConfig;
use rusqlite::Connection;

/// Errors from the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// A SQLite-backed config store.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) a store at `path` and apply the schema.
    pub fn open(path: &std::path::Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open an in-memory store (tests).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS channels (
                 id        INTEGER PRIMARY KEY,
                 name      TEXT NOT NULL,
                 mode      TEXT NOT NULL,
                 device_id TEXT NOT NULL
             );",
        )?;
        Ok(Store { conn })
    }

    /// Insert or update a channel config (idempotent on channel id).
    pub fn upsert_channel(&self, cfg: &ChannelConfig) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO channels (id, name, mode, device_id)
                 VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 mode = excluded.mode,
                 device_id = excluded.device_id;",
            rusqlite::params![cfg.id.0, cfg.name, cfg.mode, cfg.device_id.0],
        )?;
        Ok(())
    }

    /// Load all persisted channels, ordered by id.
    pub fn load_channels(&self) -> Result<Vec<ChannelConfig>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, mode, device_id FROM channels ORDER BY id;")?;
        let rows = stmt.query_map([], |row| {
            Ok(ChannelConfig {
                id: ChannelId(row.get::<_, u32>(0)?),
                name: row.get(1)?,
                mode: row.get(2)?,
                device_id: DeviceId(row.get::<_, String>(3)?),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(id: u32, name: &str) -> ChannelConfig {
        ChannelConfig {
            id: ChannelId(id),
            name: name.to_string(),
            mode: "none".to_string(),
            device_id: DeviceId::placeholder(),
        }
    }

    #[test]
    fn upsert_then_load_roundtrips() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_channel(&cfg(0, "vfo-a")).unwrap();
        store.upsert_channel(&cfg(1, "vfo-b")).unwrap();

        let loaded = store.load_channels().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, ChannelId(0));
        assert_eq!(loaded[0].name, "vfo-a");
        assert_eq!(loaded[1].name, "vfo-b");
        assert_eq!(loaded[0].device_id, DeviceId::placeholder());
    }

    #[test]
    fn upsert_is_idempotent_on_id() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_channel(&cfg(0, "first")).unwrap();
        store.upsert_channel(&cfg(0, "second")).unwrap();

        let loaded = store.load_channels().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "second");
    }
}
```

> Note: this module references `crate::supervisor::channel::ChannelConfig`, defined in Task 6. Implement Task 6 before running this task's tests (or stub `ChannelConfig` first). The two are split because persistence and state are different responsibilities; build Task 6 first if executing strictly in order — the plan lists persistence first only to keep the storage contract adjacent to the schema. **Recommended execution order: do Task 6 Step 1 (define `ChannelConfig`) before Task 5's test run.**

- [ ] **Step 2: Wire the module in**

In `crates/omnimodemd/src/lib.rs`, add:

```rust
pub mod persist;
```

- [ ] **Step 3: Run the tests (after Task 6 Step 1 defines ChannelConfig)**

Run: `cargo test -p omnimodemd persist::`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodemd/src/persist/mod.rs crates/omnimodemd/src/lib.rs
git commit -m "Add SQLite config store keyed on DeviceId"
```

---

## Task 6: Supervisor skeleton

The Supervisor owns the live channels, the (placeholder) device cache, the (placeholder) PTT registry, and produces the state snapshot. This is the evolution of Graywolf's `struct Modem` in-memory-state role, restructured behind a clean snapshot interface.

**Files:**
- Create: `crates/omnimodemd/src/supervisor/channel.rs`
- Create: `crates/omnimodemd/src/supervisor/device.rs`
- Create: `crates/omnimodemd/src/supervisor/ptt.rs`
- Create: `crates/omnimodemd/src/supervisor/mod.rs`
- Modify: `crates/omnimodemd/src/lib.rs`

- [ ] **Step 1: Define channel types**

Create `crates/omnimodemd/src/supervisor/channel.rs`:

```rust
//! Channel configuration and runtime state.

use crate::ids::{ChannelId, DeviceId};

/// Persisted, operator-supplied channel configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelConfig {
    pub id: ChannelId,
    pub name: String,
    /// Phase 1 placeholder mode label (e.g. "none"); becomes a parametric
    /// `ModeConfig` in Phase 3.
    pub mode: String,
    /// Stable device this channel binds to (placeholder in Phase 1).
    pub device_id: DeviceId,
}

/// Live channel state: its config plus whether the (stub) pipeline is running.
#[derive(Debug, Clone)]
pub struct ChannelState {
    pub config: ChannelConfig,
    pub running: bool,
}

impl ChannelState {
    pub fn new(config: ChannelConfig) -> Self {
        // No DSP in Phase 1, so a configured channel is immediately "running"
        // in the stub sense (it can accept simulated transmits).
        ChannelState { config, running: true }
    }
}
```

- [ ] **Step 2: Define the placeholder device cache**

Create `crates/omnimodemd/src/supervisor/device.rs`:

```rust
//! Placeholder device cache.
//!
//! Phase 2 replaces this with real enumeration, stable `DeviceId` resolution,
//! and hotplug handling. Phase 1 only needs to vend the single placeholder
//! identity that channel config is keyed on.

use crate::ids::DeviceId;

/// Caches resolved devices. In Phase 1 it knows exactly one placeholder device.
#[derive(Debug, Default)]
pub struct DeviceCache;

impl DeviceCache {
    pub fn new() -> Self {
        DeviceCache
    }

    /// The device a newly-configured channel binds to in Phase 1.
    pub fn default_device(&self) -> DeviceId {
        DeviceId::placeholder()
    }
}
```

- [ ] **Step 3: Define the placeholder PTT registry**

Create `crates/omnimodemd/src/supervisor/ptt.rs`:

```rust
//! Placeholder PTT registry.
//!
//! The real `PortRegistry` (multi-handle, hotplug eviction, per-OS drivers)
//! lands in Phase 2. Phase 1 only records key/unkey intent so the transmit
//! simulation has something to flip, and so the cross-channel shared-state
//! shape exists from the start.

use crate::ids::ChannelId;
use std::collections::HashMap;

/// Tracks simulated PTT state per channel.
#[derive(Debug, Default)]
pub struct PttRegistry {
    keyed: HashMap<ChannelId, bool>,
}

impl PttRegistry {
    pub fn new() -> Self {
        PttRegistry::default()
    }

    /// Simulate keying the transmitter for a channel. Returns the prior state.
    pub fn key(&mut self, channel: ChannelId) -> bool {
        self.keyed.insert(channel, true).unwrap_or(false)
    }

    /// Simulate releasing PTT for a channel.
    pub fn unkey(&mut self, channel: ChannelId) {
        self.keyed.insert(channel, false);
    }

    pub fn is_keyed(&self, channel: ChannelId) -> bool {
        self.keyed.get(&channel).copied().unwrap_or(false)
    }
}
```

- [ ] **Step 4: Write the Supervisor with a failing snapshot test**

Create `crates/omnimodemd/src/supervisor/mod.rs`:

```rust
//! The Supervisor: owns live channels, the device cache, the PTT registry, and
//! the persistence store. Evolution of Graywolf's `struct Modem` state role.

pub mod channel;
pub mod device;
pub mod ptt;

use crate::ids::ChannelId;
use crate::persist::Store;
use channel::{ChannelConfig, ChannelState};
use device::DeviceCache;
use ptt::PttRegistry;
use std::collections::BTreeMap;

/// An immutable point-in-time view of modem state, used for snapshot-on-subscribe.
#[derive(Debug, Clone)]
pub struct ModemSnapshot {
    pub channels: Vec<ChannelConfig>,
    pub running: Vec<bool>,
}

/// Owns all live in-memory state plus the persistence store.
pub struct Supervisor {
    channels: BTreeMap<ChannelId, ChannelState>,
    devices: DeviceCache,
    ptt: PttRegistry,
    store: Store,
}

impl Supervisor {
    /// Build a Supervisor, restoring any persisted channels from `store`.
    pub fn new(store: Store) -> Result<Self, crate::persist::StoreError> {
        let mut channels = BTreeMap::new();
        for cfg in store.load_channels()? {
            channels.insert(cfg.id, ChannelState::new(cfg));
        }
        Ok(Supervisor {
            channels,
            devices: DeviceCache::new(),
            ptt: PttRegistry::new(),
            store,
        })
    }

    /// Apply a channel configuration: persist it, then update live state.
    pub fn configure_channel(
        &mut self,
        id: ChannelId,
        name: String,
        mode: String,
    ) -> Result<(), crate::persist::StoreError> {
        let cfg = ChannelConfig {
            id,
            name,
            mode,
            device_id: self.devices.default_device(),
        };
        self.store.upsert_channel(&cfg)?;
        self.channels
            .entry(id)
            .and_modify(|s| s.config = cfg.clone())
            .or_insert_with(|| ChannelState::new(cfg));
        Ok(())
    }

    pub fn has_channel(&self, id: ChannelId) -> bool {
        self.channels.contains_key(&id)
    }

    /// Mutable access to the PTT registry (for the transmit simulation).
    pub fn ptt_mut(&mut self) -> &mut PttRegistry {
        &mut self.ptt
    }

    /// Produce an immutable snapshot of current state.
    pub fn snapshot(&self) -> ModemSnapshot {
        let mut channels = Vec::with_capacity(self.channels.len());
        let mut running = Vec::with_capacity(self.channels.len());
        for st in self.channels.values() {
            channels.push(st.config.clone());
            running.push(st.running);
        }
        ModemSnapshot { channels, running }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_then_snapshot_reflects_channel() {
        let store = Store::open_in_memory().unwrap();
        let mut sup = Supervisor::new(store).unwrap();
        sup.configure_channel(ChannelId(0), "vfo-a".into(), "none".into())
            .unwrap();

        let snap = sup.snapshot();
        assert_eq!(snap.channels.len(), 1);
        assert_eq!(snap.channels[0].name, "vfo-a");
        assert!(snap.running[0]);
        assert!(sup.has_channel(ChannelId(0)));
    }

    #[test]
    fn reconfigure_updates_in_place() {
        let store = Store::open_in_memory().unwrap();
        let mut sup = Supervisor::new(store).unwrap();
        sup.configure_channel(ChannelId(0), "first".into(), "none".into())
            .unwrap();
        sup.configure_channel(ChannelId(0), "second".into(), "none".into())
            .unwrap();

        let snap = sup.snapshot();
        assert_eq!(snap.channels.len(), 1);
        assert_eq!(snap.channels[0].name, "second");
    }

    #[test]
    fn new_supervisor_restores_persisted_channels() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_channel(&ChannelConfig {
                id: ChannelId(3),
                name: "restored".into(),
                mode: "none".into(),
                device_id: crate::ids::DeviceId::placeholder(),
            })
            .unwrap();
        let sup = Supervisor::new(store).unwrap();
        assert!(sup.has_channel(ChannelId(3)));
    }
}
```

- [ ] **Step 5: Wire the module in**

In `crates/omnimodemd/src/lib.rs`, add:

```rust
pub mod supervisor;
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p omnimodemd supervisor:: persist::`
Expected: PASS (Supervisor's 3 tests + persistence's 2 tests; `ChannelConfig` now resolves).

- [ ] **Step 7: Commit**

```bash
git add crates/omnimodemd/src/supervisor/ crates/omnimodemd/src/lib.rs
git commit -m "Add Supervisor skeleton with snapshot and persistence restore"
```

---

## Task 7: Command/Event spine + sync core thread

This is the heart of Phase 1: the async/sync boundary. Commands arrive over a bounded `std::sync::mpsc::SyncSender` (chosen because `SyncSender` is `Sync`, so the tonic service can hold it, and bounded gives command-side backpressure). Replies go back over a `tokio::sync::oneshot` (its `send` is callable from the sync thread). Events flow out over two `tokio::broadcast` channels — one lossless, one lossy. The core is a plain `std::thread`; no tokio runs inside it.

**Files:**
- Create: `crates/omnimodemd/src/core/error.rs`
- Create: `crates/omnimodemd/src/core/event.rs`
- Create: `crates/omnimodemd/src/core/command.rs`
- Create: `crates/omnimodemd/src/core/mod.rs`
- Modify: `crates/omnimodemd/src/lib.rs`

- [ ] **Step 1: Define core errors**

Create `crates/omnimodemd/src/core/error.rs`:

```rust
//! Errors surfaced by the sync core to the async control edge.

use crate::ids::ChannelId;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("unknown channel {0:?}")]
    UnknownChannel(ChannelId),
    #[error("persistence error: {0}")]
    Persist(String),
    #[error("core shutting down")]
    Closed,
}

impl From<crate::persist::StoreError> for CoreError {
    fn from(e: crate::persist::StoreError) -> Self {
        CoreError::Persist(e.to_string())
    }
}
```

- [ ] **Step 2: Define the two event classes**

Create `crates/omnimodemd/src/core/event.rs`:

```rust
//! Events emitted by the core, split into two backpressure classes.
//!
//! Design policy (locked in Phase 1): decoded frames are LOSSLESS — never
//! silently dropped; a client that can't keep up is disconnected. Telemetry
//! (levels, status, transmit notifications) is LOSSY — only the latest value
//! matters, so dropping intermediates under lag is fine.

use crate::ids::{ChannelId, TransmitId};

/// LOSSLESS class. Carried on a dedicated broadcast; a subscriber that lags is
/// disconnected rather than allowed to miss a frame.
#[derive(Debug, Clone)]
pub enum FrameEvent {
    RxFrame {
        channel: ChannelId,
        data: Vec<u8>,
        timestamp_ns: u64,
    },
}

/// LOSSY class. Carried on a separate broadcast; lag drops intermediates.
#[derive(Debug, Clone)]
pub enum TelemetryEvent {
    ChannelConfigured { channel: ChannelId },
    TransmitStarted { channel: ChannelId, transmit_id: TransmitId },
    TransmitComplete { channel: ChannelId, transmit_id: TransmitId },
    AudioLevel { channel: ChannelId, dbfs: f32 },
    Status { channel: ChannelId, tx_frames: u64 },
}
```

- [ ] **Step 3: Define commands**

Create `crates/omnimodemd/src/core/command.rs`:

```rust
//! Commands sent from the async control edge into the sync core.
//!
//! Each command that needs an acknowledgement carries a `tokio::oneshot`
//! reply sender. `oneshot::Sender::send` is not async, so the sync core thread
//! can answer without a runtime; the async handler awaits the receiver.

use crate::core::error::CoreError;
use crate::ids::{ChannelId, TransmitId};
use crate::supervisor::ModemSnapshot;
use tokio::sync::oneshot;

pub enum Command {
    ConfigureChannel {
        id: ChannelId,
        name: String,
        mode: String,
        reply: oneshot::Sender<Result<(), CoreError>>,
    },
    Transmit {
        channel: ChannelId,
        payload: Vec<u8>,
        reply: oneshot::Sender<Result<TransmitId, CoreError>>,
    },
    GetState {
        reply: oneshot::Sender<ModemSnapshot>,
    },
    Shutdown,
}
```

- [ ] **Step 4: Write the core thread with a failing test**

Create `crates/omnimodemd/src/core/mod.rs`:

```rust
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
```

- [ ] **Step 5: Wire the module in**

In `crates/omnimodemd/src/lib.rs`, add:

```rust
pub mod core;
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p omnimodemd core::`
Expected: PASS (2 tests). Events flow over broadcast, transmit ack returns `TransmitId(1)`, unknown-channel transmit errors.

- [ ] **Step 7: Commit**

```bash
git add crates/omnimodemd/src/core/ crates/omnimodemd/src/lib.rs
git commit -m "Add command/event spine and stub sync core thread"
```

---

## Task 8: domain <-> proto conversions + unary gRPC handlers

The async edge. `convert.rs` is the single place domain types become proto and back, keeping tonic out of `core/`/`supervisor/`. `service.rs` implements the three unary RPCs by sending a `Command` + awaiting its oneshot reply.

**Files:**
- Create: `crates/omnimodemd/src/grpc/convert.rs`
- Create: `crates/omnimodemd/src/grpc/service.rs`
- Create: `crates/omnimodemd/src/grpc/mod.rs`
- Modify: `crates/omnimodemd/src/lib.rs`
- Test: `crates/omnimodemd/tests/unary.rs`

- [ ] **Step 1: Write conversions**

Create `crates/omnimodemd/src/grpc/convert.rs`:

```rust
//! Domain <-> proto conversions. The only module that bridges the two.

use crate::core::error::CoreError;
use crate::core::event::{FrameEvent, TelemetryEvent};
use crate::proto;
use crate::supervisor::ModemSnapshot;
use tonic::Status;

/// Map a core error to a gRPC status.
pub fn core_error_to_status(e: CoreError) -> Status {
    match e {
        CoreError::UnknownChannel(_) => Status::not_found(e.to_string()),
        CoreError::Persist(_) => Status::internal(e.to_string()),
        CoreError::Closed => Status::unavailable(e.to_string()),
    }
}

/// Build a proto `ModemState` from a snapshot.
pub fn snapshot_to_proto(snap: &ModemSnapshot) -> proto::ModemState {
    let channels = snap
        .channels
        .iter()
        .zip(snap.running.iter())
        .map(|(c, running)| proto::ChannelInfo {
            channel: c.id.0,
            name: c.name.clone(),
            mode: c.mode.clone(),
            device_id: c.device_id.0.clone(),
            running: *running,
        })
        .collect();
    proto::ModemState { channels }
}

/// Wrap a frame event as a proto `Event`.
pub fn frame_event_to_proto(ev: FrameEvent) -> proto::Event {
    let kind = match ev {
        FrameEvent::RxFrame { channel, data, timestamp_ns } => {
            proto::event::Kind::RxFrame(proto::RxFrame {
                channel: channel.0,
                data,
                timestamp_ns,
            })
        }
    };
    proto::Event { kind: Some(kind) }
}

/// Wrap a telemetry event as a proto `Event`.
pub fn telemetry_event_to_proto(ev: TelemetryEvent) -> proto::Event {
    use proto::event::Kind;
    let kind = match ev {
        TelemetryEvent::ChannelConfigured { channel } => {
            Kind::ChannelConfigured(proto::ChannelConfigured { channel: channel.0 })
        }
        TelemetryEvent::TransmitStarted { channel, transmit_id } => {
            Kind::TransmitStarted(proto::TransmitStarted {
                channel: channel.0,
                transmit_id: transmit_id.0,
            })
        }
        TelemetryEvent::TransmitComplete { channel, transmit_id } => {
            Kind::TransmitComplete(proto::TransmitComplete {
                channel: channel.0,
                transmit_id: transmit_id.0,
            })
        }
        TelemetryEvent::AudioLevel { channel, dbfs } => {
            Kind::AudioLevel(proto::AudioLevel { channel: channel.0, dbfs })
        }
        TelemetryEvent::Status { channel, tx_frames } => {
            Kind::Status(proto::Status { channel: channel.0, tx_frames })
        }
    };
    proto::Event { kind: Some(kind) }
}
```

- [ ] **Step 2: Write the unary service**

Create `crates/omnimodemd/src/grpc/service.rs`:

```rust
//! The `ModemControl` gRPC service implementation (unary handlers here;
//! `SubscribeEvents` lives in `subscribe.rs` and is added via the same struct).

use crate::core::command::Command;
use crate::core::CoreHandle;
use crate::grpc::convert::{core_error_to_status, snapshot_to_proto};
use crate::ids::ChannelId;
use crate::proto;
use crate::proto::modem_control_server::ModemControl;
use tokio::sync::oneshot;
use tonic::{Request, Response, Status};

/// Shared gRPC service state: just a handle to the sync core.
#[derive(Clone)]
pub struct ControlService {
    pub(crate) core: CoreHandle,
}

impl ControlService {
    pub fn new(core: CoreHandle) -> Self {
        ControlService { core }
    }

    /// Push a command into the core, mapping a full/closed queue to a status.
    pub(crate) fn send_command(&self, cmd: Command) -> Result<(), Status> {
        self.core
            .commands
            .try_send(cmd)
            .map_err(|_| Status::unavailable("core command queue full or closed"))
    }
}

#[tonic::async_trait]
impl ModemControl for ControlService {
    async fn configure_channel(
        &self,
        request: Request<proto::ConfigureChannelRequest>,
    ) -> Result<Response<proto::ConfigureChannelResponse>, Status> {
        let req = request.into_inner();
        if req.name.is_empty() {
            return Err(Status::invalid_argument("channel name must not be empty"));
        }
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigureChannel {
            id: ChannelId(req.channel),
            name: req.name,
            mode: req.mode,
            reply: tx,
        })?;
        rx.await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::ConfigureChannelResponse { channel: req.channel }))
    }

    async fn get_state(
        &self,
        _request: Request<proto::GetStateRequest>,
    ) -> Result<Response<proto::ModemState>, Status> {
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::GetState { reply: tx })?;
        let snap = rx.await.map_err(|_| Status::unavailable("core dropped reply"))?;
        Ok(Response::new(snapshot_to_proto(&snap)))
    }

    async fn transmit(
        &self,
        request: Request<proto::TransmitRequest>,
    ) -> Result<Response<proto::TransmitResponse>, Status> {
        let req = request.into_inner();
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::Transmit {
            channel: ChannelId(req.channel),
            payload: req.payload,
            reply: tx,
        })?;
        let transmit_id = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::TransmitResponse { transmit_id: transmit_id.0 }))
    }

    // SubscribeEvents is implemented in subscribe.rs as part of this same impl
    // block via an `include!`-free split: see Task 9, which replaces this file's
    // impl with the full trait. (Until Task 9, the streaming method is stubbed
    // below so the trait is satisfied and unary tests can run.)
    type SubscribeEventsStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<proto::Event, Status>> + Send + 'static>,
    >;

    async fn subscribe_events(
        &self,
        _request: Request<proto::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        Err(Status::unimplemented("SubscribeEvents lands in Task 9"))
    }
}
```

- [ ] **Step 3: Write the grpc module root**

Create `crates/omnimodemd/src/grpc/mod.rs`:

```rust
pub mod convert;
pub mod service;
pub mod subscribe;

pub use service::ControlService;
```

Create an empty placeholder so the module resolves until Task 9 fills it — `crates/omnimodemd/src/grpc/subscribe.rs`:

```rust
//! SubscribeEvents fan-out — implemented in Task 9.
```

- [ ] **Step 4: Wire the module in**

In `crates/omnimodemd/src/lib.rs`, add:

```rust
pub mod grpc;
```

- [ ] **Step 5: Add a helper to spawn an in-process server over UDS (for tests)**

In `crates/omnimodemd/src/lib.rs`, add a test-support function below the module declarations:

```rust
use std::path::Path;

/// Spawn the full control plane (core + gRPC) listening on a UDS at `path`.
/// Returns the core's join handle and a shutdown trigger. Used by integration
/// tests and by `main.rs`. Authz is applied by `authz::serve_uds` (Task 10);
/// this no-authz variant exists for unary/subscribe tests.
pub async fn serve_uds_no_authz(
    db_path: &Path,
    sock_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use grpc::ControlService;
    use proto::modem_control_server::ModemControlServer;
    use tokio_stream::wrappers::UnixListenerStream;

    let store = persist::Store::open(db_path)?;
    let supervisor = supervisor::Supervisor::new(store)?;
    let (core, _join) = core::spawn(supervisor);
    let svc = ControlService::new(core);

    let _ = std::fs::remove_file(sock_path);
    let listener = tokio::net::UnixListener::bind(sock_path)?;
    let incoming = UnixListenerStream::new(listener);

    tonic::transport::Server::builder()
        .add_service(ModemControlServer::new(svc))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
```

- [ ] **Step 6: Write the failing integration test**

Create `crates/omnimodemd/tests/unary.rs`:

```rust
//! Integration: unary RPCs over an in-process UDS server (no authz).

use omnimodemd::proto::modem_control_client::ModemControlClient;
use omnimodemd::proto::{ConfigureChannelRequest, GetStateRequest, TransmitRequest};
use tokio::net::UnixStream;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

async fn connect(sock: std::path::PathBuf) -> ModemControlClient<tonic::transport::Channel> {
    // The URI authority is ignored; the connector dials the UDS path.
    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let sock = sock.clone();
            async move { Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(UnixStream::connect(sock).await?)) }
        }))
        .await
        .unwrap();
    ModemControlClient::new(channel)
}

#[tokio::test]
async fn configure_get_transmit_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("omnimodem.sock");
    let db = dir.path().join("omnimodem.sqlite");

    let sock_srv = sock.clone();
    tokio::spawn(async move {
        omnimodemd::serve_uds_no_authz(&db, &sock_srv).await.unwrap();
    });
    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let mut client = connect(sock).await;

    // Configure.
    let resp = client
        .configure_channel(ConfigureChannelRequest {
            channel: 0,
            name: "vfo-a".into(),
            mode: "none".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.channel, 0);

    // GetState reflects it.
    let state = client.get_state(GetStateRequest {}).await.unwrap().into_inner();
    assert_eq!(state.channels.len(), 1);
    assert_eq!(state.channels[0].name, "vfo-a");
    assert_eq!(state.channels[0].device_id, "virtual:0");

    // Transmit returns a monotonic id.
    let tx = client
        .transmit(TransmitRequest { channel: 0, payload: vec![1, 2, 3] })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(tx.transmit_id, 1);

    // Empty name is rejected.
    let err = client
        .configure_channel(ConfigureChannelRequest { channel: 1, name: "".into(), mode: "none".into() })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}
```

- [ ] **Step 7: Add dev-dependencies for the integration tests**

In `crates/omnimodemd/Cargo.toml`, append:

```toml
[dev-dependencies]
tempfile = "3"
tower = { version = "0.5", features = ["util"] }
hyper-util = { version = "0.1", features = ["tokio"] }
```

- [ ] **Step 8: Run the test to verify it fails, then passes**

Run: `cargo test -p omnimodemd --test unary`
Expected: PASS — configure/get/transmit round-trip works over UDS; empty name rejected with `InvalidArgument`.

- [ ] **Step 9: Commit**

```bash
git add crates/omnimodemd/src/grpc/ crates/omnimodemd/src/lib.rs crates/omnimodemd/Cargo.toml crates/omnimodemd/tests/unary.rs
git commit -m "Add unary gRPC handlers bridging to the sync core over UDS"
```

---

## Task 9: SubscribeEvents — snapshot-on-subscribe + dual-class backpressure

This replaces the stub `subscribe_events` with the real implementation and locks down the backpressure policy: the subscriber task subscribes to **both** broadcasts *before* requesting the snapshot (so no event between subscribe and snapshot is lost — at-least-once), emits the snapshot first, then merges live frames (lossless: lag ⇒ disconnect with `resource_exhausted`) and telemetry (lossy: lag ⇒ skip and continue) into the single outbound stream.

**Files:**
- Modify: `crates/omnimodemd/src/grpc/service.rs` (remove stub `subscribe_events` + associated type; delegate to `subscribe.rs`)
- Create/replace: `crates/omnimodemd/src/grpc/subscribe.rs`
- Test: `crates/omnimodemd/tests/subscribe.rs`

- [ ] **Step 1: Replace the stub in service.rs**

In `crates/omnimodemd/src/grpc/service.rs`, delete the `type SubscribeEventsStream = ...;` line and the entire stub `async fn subscribe_events(...)` body, replacing both with a delegation:

```rust
    type SubscribeEventsStream = crate::grpc::subscribe::EventStream;

    async fn subscribe_events(
        &self,
        request: Request<proto::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        crate::grpc::subscribe::subscribe(self, request).await
    }
```

- [ ] **Step 2: Write the subscribe implementation**

Replace `crates/omnimodemd/src/grpc/subscribe.rs` with:

```rust
//! SubscribeEvents: snapshot-on-subscribe + dual-class backpressure fan-out.
//!
//! Backpressure policy (design, locked in Phase 1):
//!   * Frames are LOSSLESS — if this subscriber lags the frame ring, we end the
//!     stream with `resource_exhausted` rather than silently dropping a frame.
//!   * Telemetry is LOSSY — on lag we skip dropped intermediates and continue.
//!
//! Ordering: we subscribe to both broadcasts BEFORE asking the core for the
//! snapshot. Any event the core emits after our subscription is therefore
//! captured in our receivers, so the snapshot + live stream is at-least-once
//! (a change applied between subscribe and snapshot may appear in both; clients
//! treat the snapshot as authoritative and tolerate a duplicate follow-up).

use crate::core::command::Command;
use crate::grpc::convert::{frame_event_to_proto, snapshot_to_proto, telemetry_event_to_proto};
use crate::grpc::service::ControlService;
use crate::proto;
use std::pin::Pin;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::oneshot;
use tonic::{Request, Response, Status};

/// The boxed stream type returned to tonic.
pub type EventStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<proto::Event, Status>> + Send + 'static>>;

pub async fn subscribe(
    svc: &ControlService,
    _request: Request<proto::SubscribeRequest>,
) -> Result<Response<EventStream>, Status> {
    // Subscribe FIRST so nothing emitted after this point is lost.
    let mut frame_rx = svc.core.frames.subscribe();
    let mut tele_rx = svc.core.telemetry.subscribe();

    // Then request the snapshot.
    let (tx, rx) = oneshot::channel();
    svc.send_command(Command::GetState { reply: tx })?;
    let snapshot = rx
        .await
        .map_err(|_| Status::unavailable("core dropped snapshot reply"))?;

    let stream = async_stream::try_stream! {
        // 1) Snapshot is always the first message.
        yield proto::Event {
            kind: Some(proto::event::Kind::Snapshot(snapshot_to_proto(&snapshot))),
        };

        // 2) Merge both classes until the client goes away or a frame is lost.
        loop {
            tokio::select! {
                frame = frame_rx.recv() => match frame {
                    Ok(ev) => yield frame_event_to_proto(ev),
                    // LOSSLESS: we would have to drop a frame — disconnect instead.
                    Err(RecvError::Lagged(n)) => {
                        Err(Status::resource_exhausted(
                            format!("client lagged frame stream by {n}; disconnecting to avoid dropping frames"),
                        ))?;
                    }
                    Err(RecvError::Closed) => break,
                },
                tele = tele_rx.recv() => match tele {
                    Ok(ev) => yield telemetry_event_to_proto(ev),
                    // LOSSY: skip the dropped intermediates and keep going.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                },
            }
        }
    };

    Ok(Response::new(Box::pin(stream) as EventStream))
}
```

- [ ] **Step 3: Write the failing integration test**

Create `crates/omnimodemd/tests/subscribe.rs`:

```rust
//! Integration: snapshot-on-subscribe + live event delivery.

use omnimodemd::proto::event::Kind;
use omnimodemd::proto::modem_control_client::ModemControlClient;
use omnimodemd::proto::{ConfigureChannelRequest, SubscribeRequest, TransmitRequest};
use tokio::net::UnixStream;
use tokio_stream::StreamExt;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

async fn connect(sock: std::path::PathBuf) -> ModemControlClient<tonic::transport::Channel> {
    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let sock = sock.clone();
            async move { Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(UnixStream::connect(sock).await?)) }
        }))
        .await
        .unwrap();
    ModemControlClient::new(channel)
}

#[tokio::test]
async fn snapshot_then_live_events() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("omnimodem.sock");
    let db = dir.path().join("omnimodem.sqlite");

    let sock_srv = sock.clone();
    tokio::spawn(async move {
        omnimodemd::serve_uds_no_authz(&db, &sock_srv).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let mut client = connect(sock).await;

    // Pre-configure a channel so the snapshot is non-empty.
    client
        .configure_channel(ConfigureChannelRequest { channel: 0, name: "vfo-a".into(), mode: "none".into() })
        .await
        .unwrap();

    // Subscribe; first message must be the snapshot.
    let mut stream = client.subscribe_events(SubscribeRequest {}).await.unwrap().into_inner();
    let first = stream.next().await.unwrap().unwrap();
    match first.kind.unwrap() {
        Kind::Snapshot(s) => {
            assert_eq!(s.channels.len(), 1);
            assert_eq!(s.channels[0].name, "vfo-a");
        }
        other => panic!("expected snapshot first, got {other:?}"),
    }

    // Now transmit and observe the live Started/Complete on the stream.
    client.transmit(TransmitRequest { channel: 0, payload: vec![9] }).await.unwrap();

    let mut saw_started = false;
    let mut saw_complete = false;
    while !(saw_started && saw_complete) {
        let ev = stream.next().await.unwrap().unwrap();
        match ev.kind.unwrap() {
            Kind::TransmitStarted(s) => { assert_eq!(s.channel, 0); saw_started = true; }
            Kind::TransmitComplete(c) => { assert_eq!(c.channel, 0); saw_complete = true; }
            // ChannelConfigured from any concurrent config is fine to ignore.
            _ => {}
        }
    }
    assert!(saw_started && saw_complete);
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p omnimodemd --test subscribe`
Expected: PASS — snapshot arrives first, then live `TransmitStarted`/`TransmitComplete`.

- [ ] **Step 5: Re-run the full suite to confirm nothing regressed**

Run: `cargo test -p omnimodemd`
Expected: all unit + integration tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/omnimodemd/src/grpc/service.rs crates/omnimodemd/src/grpc/subscribe.rs crates/omnimodemd/tests/subscribe.rs
git commit -m "Implement SubscribeEvents with snapshot and dual-class backpressure"
```

---

## Task 10: UDS authorization — socket mode + SO_PEERCRED

Opening the control socket means the ability to key a transmitter under the operator's license, so authz is required even on the default local transport. On UDS: restrict the socket file mode to `0600` and reject any peer whose uid differs from the server's via `SO_PEERCRED` (Linux). tonic exposes per-connection `UdsConnectInfo` carrying `peer_cred`; an interceptor reads it and returns `unauthenticated` on mismatch.

**Files:**
- Create: `crates/omnimodemd/src/authz/uds.rs`
- Create: `crates/omnimodemd/src/authz/mod.rs`
- Modify: `crates/omnimodemd/src/lib.rs`
- Test: `crates/omnimodemd/src/authz/uds.rs` (inline unit test for the policy check)

- [ ] **Step 1: Write the peer-uid policy + a failing unit test**

Create `crates/omnimodemd/src/authz/uds.rs`:

```rust
//! UDS authorization: socket-file mode hardening + SO_PEERCRED peer-uid check.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Decide whether a peer uid is allowed. Phase 1 policy: the peer must be the
/// same uid the daemon runs as. (An allowlist arrives with multi-user setups.)
pub fn peer_uid_allowed(server_uid: u32, peer_uid: u32) -> bool {
    peer_uid == server_uid
}

/// The uid this process runs as.
pub fn current_uid() -> u32 {
    // Safe: getuid() has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Harden the socket file so only the owner can connect (mode 0600). UDS
/// connect permission is governed by the socket file's mode on Linux.
pub fn harden_socket_mode(path: &Path) -> std::io::Result<()> {
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_uid_allowed_other_denied() {
        assert!(peer_uid_allowed(1000, 1000));
        assert!(!peer_uid_allowed(1000, 1001));
        assert!(!peer_uid_allowed(0, 1000));
    }
}
```

- [ ] **Step 2: Write the authz module with the tonic interceptor and serve helper**

Create `crates/omnimodemd/src/authz/mod.rs`:

```rust
//! Transport selection and authorization for the control plane.

pub mod tls;
pub mod uds;

use crate::grpc::ControlService;
use crate::proto::modem_control_server::ModemControlServer;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::server::UdsConnectInfo;
use tonic::{Request, Status};

/// Which transport the daemon binds. UDS is the secure default; routable binds
/// require mTLS (Phase 1 only stubs the hook — see `tls`).
pub enum Transport {
    /// Unix domain socket (default). Peer-uid checked via SO_PEERCRED.
    Uds { path: std::path::PathBuf },
    /// Loopback TCP. NOTE: exposes EVERY local user — no peer isolation.
    TcpLoopback { addr: std::net::SocketAddr },
    /// Routable bind. Requires mTLS, which is not implemented in Phase 1.
    Routable { addr: std::net::SocketAddr },
}

/// Serve the control plane over a UDS with SO_PEERCRED authorization.
pub async fn serve_uds(
    svc: ControlService,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    uds::harden_socket_mode(path)?;
    let incoming = UnixListenerStream::new(listener);

    let server_uid = uds::current_uid();
    let interceptor = move |req: Request<()>| -> Result<Request<()>, Status> {
        match req.extensions().get::<UdsConnectInfo>() {
            Some(info) => {
                let peer_uid = info
                    .peer_cred
                    .ok_or_else(|| Status::unauthenticated("no peer credentials"))?
                    .uid();
                if uds::peer_uid_allowed(server_uid, peer_uid) {
                    Ok(req)
                } else {
                    Err(Status::unauthenticated(format!(
                        "peer uid {peer_uid} not authorized"
                    )))
                }
            }
            None => Err(Status::unauthenticated("no connection info")),
        }
    };

    tonic::transport::Server::builder()
        .add_service(ModemControlServer::with_interceptor(svc, interceptor))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
```

- [ ] **Step 3: Wire the module in**

In `crates/omnimodemd/src/lib.rs`, add:

```rust
pub mod authz;
```

- [ ] **Step 4: Run the unit test**

Run: `cargo test -p omnimodemd authz::uds::tests`
Expected: PASS — same-uid allowed, differing uid denied.

> Note on the SO_PEERCRED happy path: it is exercised end-to-end in Task 13 (the e2e test connects as the same uid that runs the test, so the interceptor admits it). A negative end-to-end test (connecting as a different uid) requires a second OS user and is out of scope for CI; the policy function `peer_uid_allowed` is unit-tested here in isolation, and the interceptor wiring is covered by the passing e2e connection.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/authz/uds.rs crates/omnimodemd/src/authz/mod.rs crates/omnimodemd/src/lib.rs
git commit -m "Add UDS authz: socket mode hardening and SO_PEERCRED peer-uid check"
```

---

## Task 11: mTLS hook + transport selection

The mTLS path is mandatory before any routable bind but not implemented in Phase 1 — it must exist as a hook that fails closed, so a future routable bind cannot silently run unauthenticated. Transport selection makes the loopback-TCP exposure explicit and refuses routable binds outright.

**Files:**
- Create: `crates/omnimodemd/src/authz/tls.rs`
- Modify: `crates/omnimodemd/src/authz/mod.rs` (add `select` + test)

- [ ] **Step 1: Write the mTLS hook stub**

Create `crates/omnimodemd/src/authz/tls.rs`:

```rust
//! mTLS hook for routable binds.
//!
//! Phase 1 does NOT implement mTLS. This hook exists so the routable code path
//! fails CLOSED: any attempt to bind a routable interface errors here instead
//! of silently serving an unauthenticated, internet-reachable transmitter
//! control socket. Phase 5 fills this in (cert loading + per-method authz).

/// Error returned when a routable bind is attempted in Phase 1.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("mTLS is required for routable binds and is not implemented yet (Phase 5)")]
    NotImplemented,
}

/// Build the mTLS server config for a routable bind. Always errors in Phase 1.
pub fn routable_tls_config() -> Result<(), TlsError> {
    Err(TlsError::NotImplemented)
}
```

- [ ] **Step 2: Add transport selection with a failing test**

In `crates/omnimodemd/src/authz/mod.rs`, append:

```rust
/// Validate a chosen transport before binding. Returns a warning string for
/// transports that are allowed-but-risky, or an error for disallowed ones.
pub fn validate_transport(t: &Transport) -> Result<Option<String>, tls::TlsError> {
    match t {
        Transport::Uds { .. } => Ok(None),
        Transport::TcpLoopback { .. } => Ok(Some(
            "loopback TCP exposes every local user; no per-peer authorization is enforced"
                .to_string(),
        )),
        // Fails closed: routable requires mTLS, unimplemented in Phase 1.
        Transport::Routable { .. } => {
            tls::routable_tls_config()?;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uds_is_clean() {
        let t = Transport::Uds { path: "/tmp/x.sock".into() };
        assert_eq!(validate_transport(&t).unwrap(), None);
    }

    #[test]
    fn loopback_warns() {
        let t = Transport::TcpLoopback { addr: "127.0.0.1:9000".parse().unwrap() };
        assert!(validate_transport(&t).unwrap().is_some());
    }

    #[test]
    fn routable_fails_closed() {
        let t = Transport::Routable { addr: "0.0.0.0:9000".parse().unwrap() };
        assert!(validate_transport(&t).is_err());
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p omnimodemd authz::`
Expected: PASS — UDS clean, loopback warns, routable fails closed.

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodemd/src/authz/tls.rs crates/omnimodemd/src/authz/mod.rs
git commit -m "Add mTLS hook stub and fail-closed transport selection"
```

---

## Task 12: main.rs wiring + graceful shutdown

Wire everything: parse a socket path and db path, build the store + supervisor + core, validate the transport, log any warning, and serve over the authorized UDS until SIGINT.

**Files:**
- Modify: `crates/omnimodemd/src/main.rs`

- [ ] **Step 1: Replace main with the full wiring**

Replace `crates/omnimodemd/src/main.rs` with:

```rust
//! omnimodemd entrypoint: wire the sync core to the authorized gRPC edge.

use omnimodemd::authz::{self, Transport};
use omnimodemd::core;
use omnimodemd::grpc::ControlService;
use omnimodemd::persist::Store;
use omnimodemd::supervisor::Supervisor;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Phase 1 config is environment-driven; a real arg parser arrives later.
    let runtime_dir = std::env::var("OMNIMODEM_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("omnimodem"));
    std::fs::create_dir_all(&runtime_dir)?;
    let sock_path = runtime_dir.join("omnimodem.sock");
    let db_path = runtime_dir.join("omnimodem.sqlite");

    let transport = Transport::Uds { path: sock_path.clone() };
    if let Some(warning) = authz::validate_transport(&transport)? {
        tracing::warn!("{warning}");
    }

    let store = Store::open(&db_path)?;
    let supervisor = Supervisor::new(store)?;
    let (core_handle, _join) = core::spawn(supervisor);
    let svc = ControlService::new(core_handle);

    tracing::info!(socket = %sock_path.display(), "omnimodemd {} serving", omnimodemd::VERSION);

    // serve_uds runs until the process is signalled; Ctrl-C tears it down.
    tokio::select! {
        res = authz::serve_uds(svc, &sock_path) => { res?; }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutdown signal received");
        }
    }

    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}
```

- [ ] **Step 2: Verify the whole crate builds (binary + lib + tests)**

Run: `cargo build -p omnimodemd && cargo test -p omnimodemd --no-run`
Expected: success; binary and all test harnesses compile.

- [ ] **Step 3: Manually smoke-test the running daemon**

Run (terminal 1): `OMNIMODEM_RUNTIME_DIR=/tmp/omni-smoke cargo run -p omnimodemd`
Expected: logs `omnimodemd 0.1.0 serving` with the socket path; `ls -l /tmp/omni-smoke/omnimodem.sock` shows mode `srw-------` (0600). Ctrl-C exits cleanly and removes the socket.

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodemd/src/main.rs
git commit -m "Wire omnimodemd main: core + authorized UDS gRPC edge with graceful shutdown"
```

---

## Task 13: End-to-end exit-criterion test

The gate. Over a **real UDS with authz enabled** (`serve_uds`, not the no-authz helper), a client configures a channel, subscribes and receives a snapshot, and drives a fake transmit round-trip — unary ack **and** streamed `TransmitStarted`/`TransmitComplete` — with no audio or DSP present. Connecting as the same uid that runs the test exercises the SO_PEERCRED happy path.

**Files:**
- Create: `crates/omnimodemd/src/lib.rs` helper `serve_uds_authz_for_test` (thin wrapper around `authz::serve_uds`)
- Test: `crates/omnimodemd/tests/e2e.rs`

- [ ] **Step 1: Add a server-spawn helper that uses the authorized path**

In `crates/omnimodemd/src/lib.rs`, below `serve_uds_no_authz`, add:

```rust
/// Spawn the control plane over an AUTHORIZED UDS (SO_PEERCRED enforced).
/// Used by the e2e exit-criterion test and by anything that wants the real
/// production transport in-process.
pub async fn serve_uds_authz_for_test(
    db_path: &Path,
    sock_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = persist::Store::open(db_path)?;
    let supervisor = supervisor::Supervisor::new(store)?;
    let (core, _join) = core::spawn(supervisor);
    let svc = grpc::ControlService::new(core);
    authz::serve_uds(svc, sock_path).await
}
```

- [ ] **Step 2: Write the exit-criterion test**

Create `crates/omnimodemd/tests/e2e.rs`:

```rust
//! Phase 1 EXIT CRITERION (design doc): over gRPC, a client configures a
//! virtual channel, subscribes to events (receiving a snapshot), and completes
//! a fake transmit round-trip end-to-end — with no audio devices or DSP, over
//! the authorized UDS transport.

use omnimodemd::proto::event::Kind;
use omnimodemd::proto::modem_control_client::ModemControlClient;
use omnimodemd::proto::{ConfigureChannelRequest, SubscribeRequest, TransmitRequest};
use tokio::net::UnixStream;
use tokio_stream::StreamExt;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

async fn connect(sock: std::path::PathBuf) -> ModemControlClient<tonic::transport::Channel> {
    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let sock = sock.clone();
            async move { Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(UnixStream::connect(sock).await?)) }
        }))
        .await
        .unwrap();
    ModemControlClient::new(channel)
}

#[tokio::test]
async fn phase1_exit_criterion_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("omnimodem.sock");
    let db = dir.path().join("omnimodem.sqlite");

    let sock_srv = sock.clone();
    tokio::spawn(async move {
        omnimodemd::serve_uds_authz_for_test(&db, &sock_srv).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Connecting as the same uid that runs the test passes SO_PEERCRED.
    let mut client = connect(sock).await;

    // (1) Configure a virtual channel.
    let cfg = client
        .configure_channel(ConfigureChannelRequest { channel: 0, name: "vfo-a".into(), mode: "none".into() })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(cfg.channel, 0);

    // (2) Subscribe; the first event is the state snapshot.
    let mut stream = client.subscribe_events(SubscribeRequest {}).await.unwrap().into_inner();
    match stream.next().await.unwrap().unwrap().kind.unwrap() {
        Kind::Snapshot(s) => {
            assert_eq!(s.channels.len(), 1);
            assert_eq!(s.channels[0].name, "vfo-a");
            assert_eq!(s.channels[0].device_id, "virtual:0");
        }
        other => panic!("expected snapshot first, got {other:?}"),
    }

    // (3) Drive the fake transmit round-trip: unary ack...
    let tx = client
        .transmit(TransmitRequest { channel: 0, payload: b"hello".to_vec() })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(tx.transmit_id, 1);

    // ...and the streamed Started + Complete for the same transmit id.
    let mut started = None;
    let mut completed = None;
    while started.is_none() || completed.is_none() {
        match stream.next().await.unwrap().unwrap().kind.unwrap() {
            Kind::TransmitStarted(s) => started = Some(s.transmit_id),
            Kind::TransmitComplete(c) => completed = Some(c.transmit_id),
            _ => {}
        }
    }
    assert_eq!(started, Some(1));
    assert_eq!(completed, Some(1));
}
```

- [ ] **Step 3: Run the exit-criterion test**

Run: `cargo test -p omnimodemd --test e2e`
Expected: PASS — the full configure → subscribe(snapshot) → transmit(ack + Started + Complete) round-trip succeeds over the authorized UDS.

- [ ] **Step 4: Run the entire suite once more**

Run: `cargo test -p omnimodemd`
Expected: every unit + integration test passes (`proto`, `ids`, `persist`, `supervisor`, `core`, `authz`, `unary`, `subscribe`, `e2e`).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/lib.rs crates/omnimodemd/tests/e2e.rs
git commit -m "Add Phase 1 exit-criterion end-to-end test over authorized UDS"
```

---

## Self-Review

**1. Spec coverage** (Phase 1 deliverables from the design doc, each mapped to a task):

| Phase 1 deliverable (design doc) | Task(s) |
|---|---|
| Cargo workspace / program structure | 1 |
| `ModemControl` gRPC service — unary C2 | 2, 8 |
| Server-streaming `SubscribeEvents` with snapshot-on-subscribe | 2, 9 |
| Async/sync split: tokio handlers → `mpsc` → stub sync core | 7, 8 |
| Events back via `tokio::broadcast` | 7, 9 |
| Supervisor skeleton (channels, device cache, PTT registry) | 6 |
| SQLite persistence keyed on placeholder `DeviceId` | 5, 6 |
| Lossless-frames / lossy-telemetry backpressure policy | 7 (classes), 9 (enforcement) |
| Local authz: UDS + `SO_PEERCRED` | 10 |
| mTLS hook for routable binds | 11 |
| Proto versioning policy | 3 |
| Exit criterion: configure → subscribe → fake transmit round-trip | 13 |

No deliverable is unmapped.

**2. Placeholder scan:** No "TBD"/"add error handling"/"write tests for the above" placeholders remain — every code step carries complete code, every test step carries full assertions, and every run step states the expected result. The only intentional *stub* is the `subscribe_events` method in Task 8, explicitly replaced in Task 9 (called out in-line); this is a sequencing scaffold, not a plan gap.

**3. Type consistency:** Identifier types (`ChannelId`, `TransmitId`, `DeviceId`) and `ChannelConfig`/`ChannelState`/`ModemSnapshot`/`Command`/`FrameEvent`/`TelemetryEvent`/`CoreHandle`/`ControlService` are defined once and referenced with matching field names and signatures throughout (e.g. `ModemSnapshot { channels, running }` is constructed in Task 6 and consumed identically in `snapshot_to_proto` in Task 8; `TelemetryEvent` variants in Task 7 match the `match` arms in `telemetry_event_to_proto` in Task 8 and the proto `Event.kind` variants in Task 2). The proto oneof accessor names (`event::Kind::Snapshot`, `TransmitStarted`, etc.) are used consistently in `convert.rs`, `subscribe.rs`, and all three test files.

**Cross-task ordering note:** Task 5 (persistence) references `ChannelConfig`, which is defined in Task 6 Step 1. When executing strictly in number order, define `ChannelConfig` (Task 6 Step 1) before running Task 5's tests — this dependency is flagged in Task 5's body. All other tasks are in dependency order.
