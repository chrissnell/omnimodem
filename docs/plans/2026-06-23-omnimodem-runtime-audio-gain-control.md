# Runtime Audio Gain Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give a gRPC client a `SetAudioGain(channel, rx_gain, tx_gain)` RPC that adjusts a channel's RX input gain and TX output gain at runtime, taking effect on the already-running RX/TX workers without reconfiguring audio.

**Architecture:** Today the modem only *reports* level (dBFS telemetry); there is no setter, TX amplitude is whatever the modulator emits, and RX gain is unity. We add a per-channel `AudioGain` holder — two lock-free `Arc<AtomicU32>` cells storing `f32` linear multipliers via `f32::to_bits`/`from_bits` — created at `ConfigureAudio`, cloned into the RX and TX worker threads at spawn, and mutated by a new `SetAudioGain` command. The RX worker multiplies captured samples by `rx_gain` before feeding the demod; the TX worker multiplies modulated samples by `tx_gain` before the `i16` conversion. Because the workers read the shared atomic every loop iteration, a `SetAudioGain` call updates a live channel instantly with no respawn and no lock on the hot path.

**Tech Stack:** Rust, tonic/prost (gRPC), `std::sync::atomic`, plain `std::thread` sync core.

---

## File Structure

- `proto/omnimodem.proto` — add `SetAudioGain` RPC + request/response (additive, per `proto/VERSIONING.md`).
- `crates/omnimodem/src/core/gain.rs` — **new** `AudioGain` shared-cell type (the only new file).
- `crates/omnimodem/src/core/command.rs` — `Command::SetAudioGain`.
- `crates/omnimodem/src/core/mod.rs` — own the per-channel `AudioGain` map, create at `configure_audio`, clone into workers at spawn, handle the command.
- `crates/omnimodem/src/core/rx_worker.rs` — apply `rx_gain` to captured samples.
- `crates/omnimodem/src/core/tx_worker.rs` — apply `tx_gain` before `i16` conversion.
- `crates/omnimodem/src/grpc/service.rs` — `set_audio_gain` handler.

Convention note: existing phase plans live in `docs/plans/`; this plan follows that location. This plan is independent of the flexible-audio-device-binding plan; if both land, apply them in either order — the only shared file is `core/mod.rs` (`configure_audio` and `try_spawn_workers`), where the edits touch different lines.

---

### Task 1: The `AudioGain` shared-cell type

**Files:**
- Create: `crates/omnimodem/src/core/gain.rs`
- Modify: `crates/omnimodem/src/core/mod.rs` (add `mod gain;` / `pub(crate) use`)
- Test: `crates/omnimodem/src/core/gain.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Create `crates/omnimodem/src/core/gain.rs`:

```rust
//! Per-channel runtime audio gain: lock-free linear multipliers for RX and TX,
//! shared between the core (writer) and a worker thread (reader). Stored as
//! `f32` bits in an `AtomicU32` so the hot path reads with a single relaxed load
//! and `SetAudioGain` updates a running worker with no respawn.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// A clonable handle to one channel's RX/TX gain. Clones share the same cells.
#[derive(Clone)]
pub struct AudioGain {
    rx: Arc<AtomicU32>,
    tx: Arc<AtomicU32>,
}

impl Default for AudioGain {
    fn default() -> Self {
        AudioGain {
            rx: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            tx: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        }
    }
}

impl AudioGain {
    pub fn rx(&self) -> f32 {
        f32::from_bits(self.rx.load(Ordering::Relaxed))
    }
    pub fn tx(&self) -> f32 {
        f32::from_bits(self.tx.load(Ordering::Relaxed))
    }
    /// Set both gains. Non-finite or negative inputs are clamped to a safe range
    /// so a bad client cannot push NaN/inf into the sample path.
    pub fn set(&self, rx: f32, tx: f32) {
        self.rx.store(sanitize(rx).to_bits(), Ordering::Relaxed);
        self.tx.store(sanitize(tx).to_bits(), Ordering::Relaxed);
    }
}

/// Clamp to `[0.0, 16.0]`; map non-finite to unity. 16x (~+24 dB) is plenty of
/// headroom for a soundcard line level without letting a client send garbage.
fn sanitize(g: f32) -> f32 {
    if g.is_finite() {
        g.clamp(0.0, 16.0)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_unity() {
        let g = AudioGain::default();
        assert_eq!(g.rx(), 1.0);
        assert_eq!(g.tx(), 1.0);
    }

    #[test]
    fn set_is_visible_through_a_clone() {
        let g = AudioGain::default();
        let worker_view = g.clone();
        g.set(2.5, 0.5);
        assert_eq!(worker_view.rx(), 2.5);
        assert_eq!(worker_view.tx(), 0.5);
    }

    #[test]
    fn sanitizes_nan_and_clamps_range() {
        let g = AudioGain::default();
        g.set(f32::NAN, 1000.0);
        assert_eq!(g.rx(), 1.0); // NaN -> unity
        assert_eq!(g.tx(), 16.0); // clamped
        g.set(-3.0, 1.0);
        assert_eq!(g.rx(), 0.0); // negative -> 0 (mute)
    }
}
```

- [ ] **Step 2: Wire the module in**

In `crates/omnimodem/src/core/mod.rs`, add alongside the other `mod` declarations (near `mod command;` / `mod rx_worker;`):

```rust
mod gain;
pub(crate) use gain::AudioGain;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p omnimodem core::gain 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodem/src/core/gain.rs crates/omnimodem/src/core/mod.rs
git commit -m "core: add AudioGain shared-cell type for runtime RX/TX gain"
```

---

### Task 2: RX worker applies `rx_gain`

**Files:**
- Modify: `crates/omnimodem/src/core/rx_worker.rs:46-103` (streaming) and `:111-182` (windowed)
- Test: `crates/omnimodem/src/core/rx_worker.rs` (tests module)

- [ ] **Step 1: Add a `gain: AudioGain` parameter to both spawners**

Add `gain: crate::core::AudioGain,` to the parameter lists of `spawn_streaming` (line 46) and `spawn_windowed` (line 111), after `metrics`. Move `gain` into each spawned thread (capture it in the `move` closure).

- [ ] **Step 2: Apply the gain to captured samples**

In `spawn_streaming`, the chunk is converted at line 74 (`let samples = resample(&mut resampler, to_f32(&chunk));`). Change to apply gain to the float samples before the demod sees them:

```rust
                            let mut samples = resample(&mut resampler, to_f32(&chunk));
                            let g = gain.rx();
                            if g != 1.0 {
                                for s in samples.iter_mut() {
                                    *s *= g;
                                }
                            }
```

In `spawn_windowed`, the equivalent line is 143 (`buf.extend_from_slice(&resample(&mut resampler, to_f32(&chunk)));`). Change to:

```rust
                            let mut chunk_samples = resample(&mut resampler, to_f32(&chunk));
                            let g = gain.rx();
                            if g != 1.0 {
                                for s in chunk_samples.iter_mut() {
                                    *s *= g;
                                }
                            }
                            buf.extend_from_slice(&chunk_samples);
```

(Reading `gain.rx()` once per chunk — not per sample — keeps the hot path to one relaxed load per chunk.)

- [ ] **Step 3: Write the failing test**

Add to the `rx_worker.rs` tests module (near `rx_worker_decodes_a_windowed_ft8_message`). Drive a low-amplitude file/synthetic capture that the demod misses at unity but decodes once `rx_gain` is raised — or, if the existing tests assert on emitted `dbfs`/level, assert that the reported level rises with gain. Concretely, mirror the existing streaming test's harness and assert that with `gain.set(_, _)` raising `rx` to e.g. `8.0`, a known-too-quiet AFSK capture now produces a frame while at unity it produces none:

```rust
#[test]
fn rx_gain_lifts_a_quiet_capture_into_decode_range() {
    let gain = crate::core::AudioGain::default();
    // ... build the streaming worker exactly like the existing AFSK rx test,
    // passing `gain.clone()` as the new arg, feeding a -30 dBFS capture ...
    gain.set(8.0, 1.0); // boost RX before the capture replays
    // assert: at least one RxFrame arrives on the frames receiver.
}
```

If constructing a precisely-calibrated quiet corpus is impractical in-tree, instead assert the simpler, fully-deterministic invariant: feed a constant-amplitude capture, set `rx_gain = 4.0`, and assert the emitted `ChannelMetrics.dbfs` is ~12 dB (`20*log10(4)`) higher than the unity-gain run on the same input. Use the existing telemetry-capture helper.

- [ ] **Step 4: Update all existing `spawn_streaming` / `spawn_windowed` call sites**

The only production caller is `try_spawn_workers` in `core/mod.rs` (Task 4 wires the real gain there). Every existing **test** that calls `spawn_streaming`/`spawn_windowed` must pass `crate::core::AudioGain::default()` as the new argument. Find them: `rg 'spawn_streaming|spawn_windowed' crates/omnimodem/src` and add the default to each test call.

- [ ] **Step 5: Run the RX worker tests**

Run: `cargo test -p omnimodem rx_worker 2>&1 | tail -20`
Expected: PASS, including the new gain test and all pre-existing decode tests (unity-gain default leaves them unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/omnimodem/src/core/rx_worker.rs
git commit -m "rx_worker: apply runtime rx_gain to captured samples"
```

---

### Task 3: TX worker applies `tx_gain`

**Files:**
- Modify: `crates/omnimodem/src/core/tx_worker.rs:41-55` (`TxWorkerCfg`), `:118-141` (`run`)
- Test: `crates/omnimodem/src/core/tx_worker.rs:226-270` (tests module)

- [ ] **Step 1: Add `gain` to `TxWorkerCfg`**

In `TxWorkerCfg` (line 41), add after `slot_s`:

```rust
    /// Runtime TX output gain (linear multiplier, 1.0 == unity).
    pub gain: crate::core::AudioGain,
```

- [ ] **Step 2: Apply `tx_gain` before the i16 conversion**

In `run` (lines 140-141), change the PCM conversion to scale by the live TX gain:

```rust
        let g = cfg.gain.tx();
        let pcm: Vec<i16> = samples
            .iter()
            .map(|&s| ((s * g).clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
```

(The clamp stays *after* the multiply, so boosting gain can drive into the rails but never produces out-of-range `i16` — same safety the unity path had.)

- [ ] **Step 3: Update the existing TX worker test + any other constructors**

The test `worker_modulates_and_plays_a_queued_text_frame` (line 226) builds a `TxWorkerCfg`. Add `gain: crate::core::AudioGain::default(),` to that literal. Search for any other `TxWorkerCfg {` construction (`rg 'TxWorkerCfg' crates/omnimodem/src`) and add the field there too — including the real one in `core/mod.rs::try_spawn_workers` (Task 4 sets it to the channel's real gain).

- [ ] **Step 4: Write the failing gain test**

Add to the tx_worker tests module:

```rust
#[test]
fn tx_gain_scales_played_amplitude() {
    // Build two workers on FileBackend sinks with the same text frame, one at
    // unity and one at gain.tx() = 0.5, mirroring
    // worker_modulates_and_plays_a_queued_text_frame's setup.
    // Assert: peak |sample| of the 0.5-gain run is ~half the unity run's peak.
}
```

Use the existing backend's captured-`played` buffer (the test at line 268 already reads `backend.played`); compare peak absolute sample values between a unity worker and a `gain.set(_, 0.5)` worker fed the identical frame.

- [ ] **Step 5: Run the TX worker tests**

Run: `cargo test -p omnimodem tx_worker 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/omnimodem/src/core/tx_worker.rs
git commit -m "tx_worker: apply runtime tx_gain before i16 conversion"
```

---

### Task 4: Core owns the per-channel gain and wires it into workers

**Files:**
- Modify: `crates/omnimodem/src/core/mod.rs:100-115` (`LiveBindings`), `:289-327` (`configure_audio`), `:351-419` (`try_spawn_workers`)
- Test: `crates/omnimodem/src/core/mod.rs` (tests module)

- [ ] **Step 1: Add a gain map to `LiveBindings`**

In `LiveBindings` (after `metrics`, line 114):

```rust
    /// Per-channel runtime audio gain, shared with the RX/TX workers.
    gains: HashMap<ChannelId, AudioGain>,
```

- [ ] **Step 2: Ensure a gain entry exists at `configure_audio`**

At the end of `configure_audio` (after the `live.audio.insert(...)` around line 325), add:

```rust
    live.gains.entry(id).or_default();
```

so a channel always has a gain cell once audio is bound (default unity).

- [ ] **Step 3: Clone the gain into both workers at spawn**

In `try_spawn_workers`, obtain the channel's gain once near the top (after `let mode = ...`):

```rust
    let gain = live.gains.entry(channel).or_default().clone();
```

Pass `gain.clone()` as the new last argument to `RxWorker::spawn_streaming(...)` and `RxWorker::spawn_windowed(...)`, and set `gain` in the `TxWorkerCfg { ... }` literal (the field added in Task 3).

- [ ] **Step 4: Run the core tests**

Run: `cargo test -p omnimodem core:: 2>&1 | tail -20`
Expected: PASS (existing behavior unchanged at unity gain).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/core/mod.rs
git commit -m "core: own per-channel AudioGain and pass it to RX/TX workers"
```

---

### Task 5: The `SetAudioGain` command and handler

**Files:**
- Modify: `proto/omnimodem.proto` (RPC + messages)
- Modify: `crates/omnimodem/src/core/command.rs:13-68`
- Modify: `crates/omnimodem/src/core/mod.rs` (command dispatch)
- Modify: `crates/omnimodem/src/grpc/service.rs`
- Test: `crates/omnimodem/src/core/mod.rs` and/or `grpc` tests

- [ ] **Step 1: Add the proto RPC and messages**

In the `service ModemControl` block (after `rpc GetMetrics`, line 45):

```protobuf
  // Adjust a channel's RX input gain and TX output gain at runtime. Linear
  // multipliers (1.0 == unity); takes effect on the running workers.
  rpc SetAudioGain(SetAudioGainRequest) returns (SetAudioGainResponse);
```

And new messages near the other Phase-5 messages:

```protobuf
message SetAudioGainRequest {
  uint32 channel = 1;
  float rx_gain = 2;   // capture gain, linear (1.0 == unity)
  float tx_gain = 3;   // playback gain, linear (1.0 == unity)
}

message SetAudioGainResponse {}
```

- [ ] **Step 2: Add the command variant**

In `command.rs`, add to the `Command` enum:

```rust
    /// Set a channel's runtime RX/TX audio gain (linear multipliers).
    SetAudioGain {
        channel: ChannelId,
        rx_gain: f32,
        tx_gain: f32,
        reply: oneshot::Sender<Result<(), CoreError>>,
    },
```

- [ ] **Step 3: Handle the command in the core**

In the core's command `match` (in `core/mod.rs`), add an arm. An unknown channel is an error; otherwise create-or-update the gain cell so the call works whether or not audio/workers exist yet:

```rust
        Command::SetAudioGain { channel, rx_gain, tx_gain, reply } => {
            let res = if supervisor.has_channel(channel) {
                live.gains.entry(channel).or_default().set(rx_gain, tx_gain);
                Ok(())
            } else {
                Err(CoreError::UnknownChannel(channel))
            };
            let _ = reply.send(res);
        }
```

(Because the worker holds a *clone* of the same `Arc` cells, this update is seen by a running worker on its next chunk — no respawn.)

- [ ] **Step 4: Add the gRPC handler**

In `grpc/service.rs`, add to the `impl ModemControl`:

```rust
    async fn set_audio_gain(
        &self,
        request: Request<proto::SetAudioGainRequest>,
    ) -> Result<Response<proto::SetAudioGainResponse>, Status> {
        let req = request.into_inner();
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::SetAudioGain {
            channel: ChannelId(req.channel),
            rx_gain: req.rx_gain,
            tx_gain: req.tx_gain,
            reply: tx,
        })?;
        rx.await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::SetAudioGainResponse {}))
    }
```

- [ ] **Step 5: Write the failing end-to-end test**

Add a core-level test that configures a channel, sends `Command::SetAudioGain`, and asserts the stored gain changed:

```rust
#[test]
fn set_audio_gain_updates_the_channels_cell() {
    // Build the core harness, ConfigureChannel + ConfigureAudio on channel 1
    // (mirror the existing audio-configured core test), then send
    // Command::SetAudioGain { rx_gain: 3.0, tx_gain: 0.25, .. } and await reply.
    // Assert the reply is Ok and the channel's AudioGain reads rx()==3.0,
    // tx()==0.25 (expose live.gains via the same test hook the other core
    // tests use, or assert behaviorally through a worker's output).
}
```

Match the existing core test harness style (drive real `Command`s over the `mpsc`/`oneshot` path). Also add the unknown-channel case asserting `CoreError::UnknownChannel`.

- [ ] **Step 6: Run the build and tests**

Run: `cargo build -p omnimodem && cargo test -p omnimodem 2>&1 | tail -30`
Expected: PASS across the crate.

- [ ] **Step 7: Commit**

```bash
git add proto/omnimodem.proto crates/omnimodem/src/core/command.rs crates/omnimodem/src/core/mod.rs crates/omnimodem/src/grpc/service.rs
git commit -m "feat: SetAudioGain RPC for runtime RX/TX gain control"
```

---

### Task 6: Lint and full regression sweep

**Files:** none (verification)

- [ ] **Step 1: Clippy the crate**

Run: `cargo clippy -p omnimodem --all-targets 2>&1 | tail -30`
Expected: no errors. Fix any unused-import notes from the new `gain` module wiring.

- [ ] **Step 2: Full test run**

Run: `cargo test -p omnimodem 2>&1 | tail -30`
Expected: all pass, including `core::gain`, `rx_worker`, `tx_worker`, core command, and gRPC tests.

- [ ] **Step 3: Commit any fixups**

```bash
git add -A
git commit -m "runtime gain control: clippy + test fixups"
```

---

## Self-Review

- **Spec coverage:** runtime adjustable RX gain — Task 2 (RX worker scales captured samples) + Task 4/5 (live update path). Runtime adjustable TX gain — Task 3 (TX worker scales before i16) + Task 4/5. The RPC itself — Task 5 (proto + command + handler). Takes effect without respawn — Task 1 (shared `Arc<AtomicU32>` cloned into the worker) + Task 5 Step 3 note. Bad-input safety — Task 1 `sanitize` (NaN→unity, clamp `[0,16]`).
- **Placeholder scan:** the worker-gain tests (Task 2 Step 3, Task 3 Step 4, Task 5 Step 5) describe the harness in prose because the exact constructor differs by the existing test style in each file; each gives the concrete assertion (dBFS delta ≈ `20·log10(gain)`, peak-amplitude halving, `rx()/tx()` readback) so there is a checkable target, not a vague "write a test." No "TBD"/"add validation" placeholders.
- **Type consistency:** `AudioGain` exposes `rx()`, `tx()`, `set(rx, tx)` and `Default` — used identically in command handling (`set`), RX worker (`rx()`), TX worker (`tx()`), and core wiring (`.entry(_).or_default().clone()`). The `gain` field/arg is the last parameter everywhere it is added (both RX spawners, `TxWorkerCfg`).
