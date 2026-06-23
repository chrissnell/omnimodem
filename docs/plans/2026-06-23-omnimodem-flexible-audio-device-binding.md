# Flexible Audio Device Binding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a channel bind its RX (capture) audio and TX (playback) audio to *different* devices, while the common single-device case keeps working unchanged.

**Architecture:** Today `ConfigureAudio` takes one `device_id` and opens both capture and playback on that one backend; `LiveBindings.audio` stores a single `(DeviceId, u32)` "rig" used for the RX worker, the TX worker, the PTT interlock, and the TX lease. We add an **optional** `tx_device_id` / `tx_sample_rate` to the request that default to the capture device when empty, replace the single-rig tuple with an `AudioBinding { rx_dev, rx_rate, tx_dev, tx_rate }`, and thread the RX device to the RX worker and the TX device to the TX worker / interlock / lease. Because the interlock and lease are keyed by `DeviceId`, split rigs then behave correctly for free: RX on rig A keeps decoding while we transmit on rig B, but a shared single rig still mutes RX during our own TX.

**Tech Stack:** Rust, tonic/prost (gRPC), rusqlite (persistence), plain `std::thread` sync core.

---

## File Structure

- `proto/omnimodem.proto` — add two request fields + one response field (additive, per `proto/VERSIONING.md`).
- `crates/omnimodemd/src/supervisor/channel.rs` — add `tx_device_id` / `tx_sample_rate` to `ChannelConfig`.
- `crates/omnimodemd/src/persist/mod.rs` — schema columns, idempotent migration, upsert, load.
- `crates/omnimodemd/src/supervisor/mod.rs` — `configure_audio` signature + store.
- `crates/omnimodemd/src/core/command.rs` — `Command::ConfigureAudio` fields.
- `crates/omnimodemd/src/core/mod.rs` — `AudioBinding` struct, `configure_audio` opens RX/TX on their own backends, and every `live.audio` reader updated.
- `crates/omnimodemd/src/grpc/service.rs` — parse `tx_device_id` (default to capture device), return `actual_tx_sample_rate`.

Convention note: existing phase plans live in `docs/plans/`; this plan follows that location.

---

### Task 1: Proto — optional split-device fields

**Files:**
- Modify: `proto/omnimodem.proto:170-179`

- [ ] **Step 1: Edit the request and response messages**

Replace the `ConfigureAudioRequest` / `ConfigureAudioResponse` block (currently lines 170-179) with:

```protobuf
message ConfigureAudioRequest {
  uint32 channel = 1;
  string device_id = 2;       // capture (RX) device; config keys on this
  uint32 sample_rate = 3;     // requested RX working rate (clamped to 48 kHz)
  uint32 fanout = 4;          // capture consumers; 0/1 == no fan-out
  // Optional split playback (TX) device. Empty == use `device_id` (the common
  // single-rig case). Set to bind capture and playback to different cards.
  string tx_device_id = 5;
  uint32 tx_sample_rate = 6;  // requested TX working rate; 0 == same as sample_rate
}

message ConfigureAudioResponse {
  uint32 actual_sample_rate = 1;     // rate the capture stream actually opened at
  uint32 actual_tx_sample_rate = 2;  // rate the playback stream actually opened at
}
```

- [ ] **Step 2: Verify the proto compiles via the build**

Run: `cargo build -p omnimodemd 2>&1 | tail -20`
Expected: builds (prost regenerates `proto::ConfigureAudioRequest` with the new `tx_device_id` / `tx_sample_rate` and `ConfigureAudioResponse::actual_tx_sample_rate`). Unused-field warnings are fine at this step.

- [ ] **Step 3: Commit**

```bash
git add proto/omnimodem.proto
git commit -m "proto: add optional split RX/TX audio device fields to ConfigureAudio"
```

---

### Task 2: Persist the TX device on `ChannelConfig`

**Files:**
- Modify: `crates/omnimodemd/src/supervisor/channel.rs:8-22`
- Modify: `crates/omnimodemd/src/persist/mod.rs:43-145`
- Test: `crates/omnimodemd/src/persist/mod.rs` (tests module at bottom)

- [ ] **Step 1: Write the failing round-trip test**

Add to the `#[cfg(test)]` module in `persist/mod.rs` (near the existing `roundtrips_*` test around line 259):

```rust
#[test]
fn roundtrips_split_tx_device() {
    let store = Store::open_in_memory().unwrap();
    let mut c = sample_channel(); // existing test helper
    c.device_id = DeviceId::AlsaCard { card_name: "Capture".into() };
    c.tx_device_id = DeviceId::AlsaCard { card_name: "Playback".into() };
    c.tx_sample_rate = 44_100;
    store.upsert_channel(&c).unwrap();

    let loaded = store.load_channels().unwrap();
    assert_eq!(loaded[0].tx_device_id, DeviceId::AlsaCard { card_name: "Playback".into() });
    assert_eq!(loaded[0].tx_sample_rate, 44_100);
}
```

If `sample_channel()` does not exist, build the `ChannelConfig` inline mirroring the existing `roundtrips_*` test's construction, adding `tx_device_id: DeviceId::placeholder()` and `tx_sample_rate: 0` to satisfy the struct.

- [ ] **Step 2: Run it to verify it fails to compile**

Run: `cargo test -p omnimodemd persist:: 2>&1 | tail -20`
Expected: FAIL — `no field tx_device_id on type ChannelConfig`.

- [ ] **Step 3: Add the struct fields**

In `channel.rs`, inside `ChannelConfig` (after `device_id` / `sample_rate`, around line 17):

```rust
    /// Playback (TX) device. Defaults to `device_id` when a client does not
    /// split RX/TX. Persisted so a split binding survives restart.
    pub tx_device_id: DeviceId,
    /// Requested TX working rate; 0 == follow `sample_rate`.
    pub tx_sample_rate: u32,
```

- [ ] **Step 4: Migrate the schema and read/write the columns**

In `persist/mod.rs`:

1. In the `CREATE TABLE IF NOT EXISTS channels (...)` block (line 43), add two columns after `fanout`:

```sql
                 tx_device_id   TEXT NOT NULL DEFAULT '',
                 tx_sample_rate INTEGER NOT NULL DEFAULT 0,
```

2. In the idempotent `ALTER` migration list (lines 63-69), add:

```rust
            "ALTER TABLE channels ADD COLUMN tx_device_id TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE channels ADD COLUMN tx_sample_rate INTEGER NOT NULL DEFAULT 0",
```

3. In `upsert_channel` (lines 85-106): add `tx_device_id, tx_sample_rate` to the INSERT column list, two more `?` placeholders, the matching `excluded.*` lines in the `ON CONFLICT ... DO UPDATE SET`, and bind `cfg.tx_device_id.to_canonical_string()` and `cfg.tx_sample_rate` in the params (mirror how `device_id` / `sample_rate` are handled).

4. In `load_channels` (lines 120-137): add `tx_device_id, tx_sample_rate` to the SELECT list and populate the struct. An empty `tx_device_id` string means "follow capture", so resolve it as:

```rust
                tx_device_id: {
                    let s: String = row.get(/* new index */)?;
                    if s.is_empty() {
                        // legacy / single-rig row: fall back to the capture device
                        DeviceId::parse(&row.get::<_, String>(3)?)
                            .unwrap_or_else(DeviceId::placeholder)
                    } else {
                        DeviceId::parse(&s).unwrap_or_else(DeviceId::placeholder)
                    }
                },
                tx_sample_rate: row.get(/* new index */)?,
```

Adjust the two `/* new index */` values to the column positions you added (they follow `fanout` at index 5, before the `ptt_*` columns — so the `ptt_*` row indices shift by 2; update those too).

- [ ] **Step 5: Run the persist tests**

Run: `cargo test -p omnimodemd persist:: 2>&1 | tail -20`
Expected: PASS, including the existing `migrates_a_phase1_schema_and_backfills_defaults` (an old row with no `tx_*` columns backfills `''` / `0` and resolves `tx_device_id` to the capture device).

- [ ] **Step 6: Commit**

```bash
git add crates/omnimodemd/src/supervisor/channel.rs crates/omnimodemd/src/persist/mod.rs
git commit -m "persist: store optional split TX audio device per channel"
```

---

### Task 3: Supervisor stores the TX device

**Files:**
- Modify: `crates/omnimodemd/src/supervisor/mod.rs:90-105`
- Test: `crates/omnimodemd/src/supervisor/mod.rs` (tests module)

- [ ] **Step 1: Update `configure_audio` to accept and store the TX device**

Change the signature and body:

```rust
    pub fn configure_audio(
        &mut self,
        id: ChannelId,
        device_id: DeviceId,
        sample_rate: u32,
        fanout: u32,
        tx_device_id: DeviceId,
        tx_sample_rate: u32,
    ) -> Result<(), crate::persist::StoreError> {
        let Some(state) = self.channels.get_mut(&id) else {
            return Ok(());
        };
        state.config.device_id = device_id;
        state.config.sample_rate = if sample_rate == 0 { MAX_SAMPLE_RATE } else { sample_rate };
        state.config.fanout = fanout;
        state.config.tx_device_id = tx_device_id;
        state.config.tx_sample_rate = tx_sample_rate;
        let cfg = state.config.clone();
        self.store.upsert_channel(&cfg)
    }
```

- [ ] **Step 2: Fix the existing supervisor test call sites**

The existing tests around line 218/231 call `configure_audio(...)`. Update each call to pass the two new args. For the single-device cases pass the capture device again and `0`:

```rust
    sup.configure_audio(id, dev.clone(), 44_100, 1, dev.clone(), 0).unwrap();
```

- [ ] **Step 3: Run the supervisor tests**

Run: `cargo test -p omnimodemd supervisor:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodemd/src/supervisor/mod.rs
git commit -m "supervisor: thread TX device through configure_audio"
```

---

### Task 4: Command carries the TX device

**Files:**
- Modify: `crates/omnimodemd/src/core/command.rs:28-34`

- [ ] **Step 1: Add fields to `Command::ConfigureAudio`**

```rust
    ConfigureAudio {
        id: ChannelId,
        device_id: DeviceId,
        sample_rate: u32,
        fanout: u32,
        tx_device_id: DeviceId,
        tx_sample_rate: u32,
        reply: oneshot::Sender<Result<ConfigureAudioOk, CoreError>>,
    },
```

- [ ] **Step 2: Add the reply type**

The reply now returns both opened rates. Add near `LeaseGrant` at the bottom of `command.rs`:

```rust
/// Opened rates from `ConfigureAudio`: the capture rate and the playback rate.
#[derive(Debug, Clone, Copy)]
pub struct ConfigureAudioOk {
    pub rx_rate: u32,
    pub tx_rate: u32,
}
```

- [ ] **Step 3: Verify it compiles (handlers updated in Task 5/6)**

Run: `cargo build -p omnimodemd 2>&1 | tail -20`
Expected: FAILs in `core/mod.rs` and `grpc/service.rs` (callers not yet updated). That is expected — Tasks 5 and 6 fix them. Do not commit yet; commit at the end of Task 5 with the core change.

---

### Task 5: Core opens RX and TX on their own devices

**Files:**
- Modify: `crates/omnimodemd/src/core/mod.rs:100-115` (`LiveBindings`)
- Modify: `crates/omnimodemd/src/core/mod.rs:289-327` (`configure_audio`)
- Modify: `crates/omnimodemd/src/core/mod.rs` — every `live.audio` reader (lines 262, 275, 361, 400, 506, 510, 590) and the `Command::ConfigureAudio` match arm
- Test: `crates/omnimodemd/src/core/mod.rs` (tests module)

- [ ] **Step 1: Introduce the `AudioBinding` struct and update `LiveBindings`**

Replace the `audio` field (line 104) and add the struct above `LiveBindings`:

```rust
/// A channel's resolved audio binding: capture (RX) and playback (TX) devices,
/// which may be the same `DeviceId` (single rig) or differ (split rigs).
#[derive(Clone)]
struct AudioBinding {
    rx_dev: DeviceId,
    rx_rate: u32,
    tx_dev: DeviceId,
    tx_rate: u32,
}
```

and in `LiveBindings`:

```rust
    /// Audio binding per channel (RX + TX device & rate). The interlock gates on
    /// `tx_dev`; the RX worker reads on `rx_dev`.
    audio: HashMap<ChannelId, AudioBinding>,
```

- [ ] **Step 2: Update `configure_audio` to open each stream on its own backend**

Replace lines 289-327 with a version that resolves both devices and opens capture on RX, playback on TX (defaulting TX to RX when the caller passed the same id / a zero rate):

```rust
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
) -> Result<ConfigureAudioOk, CoreError> {
    if !supervisor.has_channel(id) {
        return Err(CoreError::UnknownChannel(id));
    }
    let tx_rate_req = if tx_sample_rate == 0 { sample_rate } else { tx_sample_rate };
    supervisor.configure_audio(
        id, device_id.clone(), sample_rate, fanout, tx_device_id.clone(), tx_sample_rate,
    )?;

    supervisor.device_cache_mut().refresh(enumerator);
    let resolve = |dev: &DeviceId| -> Result<_, CoreError> {
        supervisor
            .device_cache_mut()
            .resolve(dev)
            .cloned()
            .ok_or_else(|| {
                CoreError::Audio(crate::audio::AudioError::DeviceNotFound(
                    dev.to_canonical_string(),
                ))
            })
    };

    let rx_desc = resolve(&device_id)?;
    let capture = (audio_factory)(&rx_desc).open_capture(sample_rate)?;
    let rx_rate = capture.sample_rate;

    let tx_desc = resolve(&tx_device_id)?;
    let playback = (audio_factory)(&tx_desc).open_playback(tx_rate_req)?;
    let tx_rate = playback.sample_rate;

    live.captures.insert(id, capture);
    live.sinks.insert(id, playback);
    live.audio.insert(
        id,
        AudioBinding { rx_dev: device_id, rx_rate, tx_dev: tx_device_id, tx_rate },
    );
    Ok(ConfigureAudioOk { rx_rate, tx_rate })
}
```

(`device_cache_mut().resolve` borrows `supervisor` mutably; call `resolve(&device_id)?` and `resolve(&tx_device_id)?` sequentially as written — each borrow ends before the next, so there is no aliasing.)

- [ ] **Step 3: Update the `Command::ConfigureAudio` match arm**

Where the core dispatches `Command::ConfigureAudio` (the arm that calls `configure_audio(...)`), destructure and forward the two new fields and the new reply type:

```rust
        Command::ConfigureAudio {
            id, device_id, sample_rate, fanout, tx_device_id, tx_sample_rate, reply,
        } => {
            let res = configure_audio(
                supervisor, &*enumerator, &audio_factory, live,
                id, device_id, sample_rate, fanout, tx_device_id, tx_sample_rate,
            );
            let _ = reply.send(res);
        }
```

- [ ] **Step 4: Update every other `live.audio` reader**

Each site changes from tuple destructuring to the named field that is correct for its purpose:

- Line 262 & 275 (`AcquireTxLease` / `ReleaseTxLease`): the lease is per **TX** rig →
  `live.audio.get(&channel).map(|b| b.tx_dev.clone())`
- Line 361 (`try_spawn_workers`, RX worker rig): the RX worker reads the **capture** rig →
  `let rig = live.audio.get(&channel).map(|b| b.rx_dev.clone());`
- Line 400 (`try_spawn_workers`, TX worker `(rig, rate)`): the TX worker plays on the **TX** rig →
  `(live.audio.get(&channel).map(|b| (b.tx_dev.clone(), b.tx_rate)), registry::build_modulator(&mode))`
- Line 506 (`transmit` legacy `have_audio`): unchanged in meaning — `live.audio.contains_key(&channel)` still holds.
- Line 510 (`transmit` legacy `(rig, rate)`): legacy TX cycle keys/plays on the **TX** rig →
  `let b = live.audio.get(&channel).cloned().unwrap(); let (rig, rate) = (b.tx_dev, b.tx_rate);`
- Line 590 (`poll_hotplug`): a departed device may match **either** the RX or TX side. Replace the single match with: if `b.rx_dev == gone || b.tx_dev == gone`, evict the channel's audio (`live.audio.remove(&c)`) and its workers, same as today.

Also update `key_ptt` (around lines 430-434): the manual-key interlock acts on the **TX** rig →
`.map(|b| b.tx_dev.clone())` (keep the `.or_else(|| live.ptt_dev.get(&channel).cloned())` fallback).

- [ ] **Step 5: Write the failing split-device integration test**

Add to the core tests module (near the existing `transmit_on_moded_channel_*` test). It uses `FileBackend` for two distinct device ids and asserts the binding records both:

```rust
#[test]
fn configure_audio_binds_distinct_rx_and_tx_devices() {
    // Build a core harness with a FileBackend factory keyed by device label
    // (mirror the existing audio-configured core test's setup).
    let h = test_core_with_two_file_devices(); // see existing helpers
    let rx = DeviceId::AlsaCard { card_name: "RX".into() };
    let tx = DeviceId::AlsaCard { card_name: "TX".into() };

    h.configure_channel(ChannelId(1), "rx-tx", "afsk1200");
    let ok = h.configure_audio_split(ChannelId(1), &rx, 48_000, 1, &tx, 48_000);
    assert!(ok.rx_rate > 0 && ok.tx_rate > 0);

    let binding = h.live_audio(ChannelId(1)).expect("bound");
    assert_eq!(binding.rx_dev, rx);
    assert_eq!(binding.tx_dev, tx);
}
```

If the existing core tests drive the core over its real `mpsc`/`oneshot` command path rather than helper methods, write this test the same way they do (send `Command::ConfigureChannel` then `Command::ConfigureAudio { tx_device_id: tx, tx_sample_rate: 48_000, .. }` with a `oneshot` reply) and assert on `ConfigureAudioOk`. Match the established test style in the file — do not invent a helper API that isn't there.

- [ ] **Step 6: Run it to verify it fails, then passes**

Run: `cargo test -p omnimodemd core:: 2>&1 | tail -30`
Expected: the new test FAILs first if written before the code, then PASSes after Steps 1-4. The existing `transmit_on_moded_channel_enqueues_and_completes` and `rx_worker_*` tests must still pass (single-device path: `tx_dev == rx_dev`).

- [ ] **Step 7: Commit**

```bash
git add crates/omnimodemd/src/core/command.rs crates/omnimodemd/src/core/mod.rs
git commit -m "core: open RX and TX audio on independent devices via AudioBinding"
```

---

### Task 6: gRPC handler defaults TX to the capture device

**Files:**
- Modify: `crates/omnimodemd/src/grpc/service.rs:98-121`
- Test: `crates/omnimodemd/src/grpc/service.rs` or the crate's gRPC test module

- [ ] **Step 1: Parse `tx_device_id` (empty == capture device) and return both rates**

Replace the `configure_audio` handler body (lines 102-120) with:

```rust
        let req = request.into_inner();
        if req.device_id.is_empty() {
            return Err(Status::invalid_argument("device_id must not be empty"));
        }
        let device_id = DeviceId::parse(&req.device_id)
            .ok_or_else(|| Status::invalid_argument(format!("unparseable device_id {}", req.device_id)))?;
        // Empty tx_device_id == single-rig: TX plays on the capture device.
        let tx_device_id = if req.tx_device_id.is_empty() {
            device_id.clone()
        } else {
            DeviceId::parse(&req.tx_device_id).ok_or_else(|| {
                Status::invalid_argument(format!("unparseable tx_device_id {}", req.tx_device_id))
            })?
        };
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigureAudio {
            id: ChannelId(req.channel),
            device_id,
            sample_rate: req.sample_rate,
            fanout: req.fanout,
            tx_device_id,
            tx_sample_rate: req.tx_sample_rate,
            reply: tx,
        })?;
        let ok = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::ConfigureAudioResponse {
            actual_sample_rate: ok.rx_rate,
            actual_tx_sample_rate: ok.tx_rate,
        }))
```

Add `use crate::core::command::ConfigureAudioOk;` if the import is needed (it is only referenced by type inference here, so likely not).

- [ ] **Step 2: Write a handler test for the default-TX behavior**

If the crate has a gRPC-level test that calls `configure_audio` against an in-process core (search for an existing `configure_audio` service test), add a case asserting that an **empty** `tx_device_id` yields a binding where `tx_dev == device_id`, and a populated one yields the split. If there is no service-level harness, this behavior is already covered by Task 5's core test plus the parse logic here; in that case add a focused unit test for the "empty tx_device_id falls back to device_id" parse branch only.

- [ ] **Step 3: Run the gRPC tests**

Run: `cargo test -p omnimodemd grpc:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodemd/src/grpc/service.rs
git commit -m "grpc: accept optional tx_device_id, default to capture device"
```

---

### Task 7: Full build, clippy, and regression sweep

**Files:** none (verification)

- [ ] **Step 1: Build and lint the whole crate**

Run: `cargo build -p omnimodemd && cargo clippy -p omnimodemd --all-targets 2>&1 | tail -30`
Expected: no errors. Fix any unused-import or `too_many_arguments` clippy notes introduced (the `#[allow(clippy::too_many_arguments)]` already on `configure_audio` covers the extra params).

- [ ] **Step 2: Run the whole crate test suite**

Run: `cargo test -p omnimodemd 2>&1 | tail -30`
Expected: all pass, including persistence migration, supervisor, core, and gRPC tests.

- [ ] **Step 3: Commit any lint fixups**

```bash
git add -A
git commit -m "flexible audio binding: clippy + test fixups"
```

---

## Self-Review

- **Spec coverage:** RX/TX on different devices — Task 5 (`AudioBinding`, independent `open_capture`/`open_playback`). Backward compatible single-rig default — Task 6 (empty `tx_device_id` → capture device) + Task 2 (legacy rows backfill). Persistence of the split — Task 2. Correct interlock/lease behavior for split rigs — Task 5 Step 4 (lease + interlock key on `tx_dev`; RX worker reads `rx_dev`). Hotplug eviction on either device — Task 5 Step 4 (`poll_hotplug` matches `rx_dev || tx_dev`).
- **Placeholder scan:** the `/* new index */` markers in Task 2 Step 4 are deliberate column-index choices the implementer fills from the column order they added; the surrounding code shows exactly how. No "add error handling"/"TBD" placeholders remain.
- **Type consistency:** `AudioBinding { rx_dev, rx_rate, tx_dev, tx_rate }` and `ConfigureAudioOk { rx_rate, tx_rate }` are used identically everywhere they appear (Tasks 4, 5, 6). `configure_audio` (supervisor) and `configure_audio` (core) signatures both end with `tx_device_id, tx_sample_rate` in that order.
