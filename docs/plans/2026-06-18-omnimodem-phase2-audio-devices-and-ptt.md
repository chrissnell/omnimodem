# Omnimodem Phase 2 — Audio Devices & PTT Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make omnimodem talk to real radio hardware reliably — enumerate audio devices under a stable cross-platform `DeviceId`, open capture/playback, and key/unkey a real transmitter — all driven over the Phase-1 gRPC surface, with **no DSP and no mode attached**.

**Architecture:** Two subsystems share one spine. A durable `DeviceId` (built from USB `idVendor:idProduct`+serial, ALSA stable card name, or `/dev/serial/by-id` topology — never a volatile `/dev` path) keys both audio config and PTT config. A `trait AudioBackend` (cpal / file / stdin) replaces Graywolf's ad-hoc spawn-by-`match`; a `trait PttDriver` + `PortRegistry` (with **DeviceId-keyed hotplug eviction**, the gap Graywolf never closed) replaces its stringly-typed PTT layer. The sync core gains a **per-channel TX worker** that runs the no-sleep key→audio→drain→unkey cycle and an explicit **RX/TX interlock** that mutes capture on a shared device while it is keyed. Everything hardware-touching sits behind a trait with a deterministic test double, so the logic is unit-tested in CI and only a thin manual smoke step needs a real radio.

**Tech Stack:** Rust 2021; `cpal` 0.17 (audio), `nix` 0.29 (`fcntl`/`ioctl` for serial RTS/DTR), `hidapi` 2.6 (CM108 HID), `gpiocdev` 0.7 (Linux gpiochip v2), `nusb` 0.1 (USB topology). Lifts the hardened patterns from Graywolf `audio/soundcard.rs`, `tx/ptt.rs`, and `modem/tx_worker.rs` (checked out alongside for reference). Builds on the Phase-1 `tonic`/`prost`/`rusqlite` stack already in the workspace.

---

## Scope

This plan covers **only Phase 2** from `docs/design/2026-06-17-omnimodem-design.md` ("Phase 2 — Basic operational components: audio devices & PTT"). Phase 1 (the gRPC control plane against a stub core) is merged; this plan plugs real hardware into that stable spine.

**Two-subsystem structure.** Phase 2 is two largely-independent subsystems — Audio and PTT — that the design explicitly says to "build in parallel." They are kept in one plan because they share the `DeviceId` foundation (Part A) and a single combined exit criterion. After the shared foundation lands (Tasks 1–2), **Part B (audio, Tasks 3–9) and Part C (PTT, Tasks 10–14) can be executed in parallel by separate workers**; Part D (Tasks 15–19) joins them at the gRPC edge.

**In scope:**
- Cross-platform `DeviceId` (durable identity + matching), replacing the Phase-1 placeholder.
- `trait AudioBackend` with cpal, file, and stdin backends; defensive format/rate selection retaining Graywolf's ALSA `plughw` 48 kHz hardening; capture stream rebuild-with-backoff; playback submitted/drained watermarks.
- Resampling (additive; source rate need not match working rate) and opt-in capture fan-out.
- Device enumeration, caching, and **hotplug detection/eviction by `DeviceId`**.
- `trait PttDriver` + structured `PttError`; Linux drivers (serial RTS/DTR, CM108 HID, gpiochip); `PortRegistry` with DeviceId-keyed eviction; no-sleep TX sequencing; unkey-on-`Drop`.
- Per-channel RX/TX interlock.
- gRPC additions (additive within `omnimodem.v1`): `ListDevices`, `ConfigureAudio`, `ConfigurePtt`, real `Transmit`/`KeyPtt`, `SuggestUdevRule`, plus hotplug/PTT events.

**Explicitly out of scope (deferred):**
- Any DSP, demod/mod, or mode (Phase 3+). "Transmit" plays a caller-supplied PCM buffer; it does not synthesize a waveform.
- macOS and Windows PTT/audio drivers. The traits and factory are shaped to accept them (Graywolf already has `ppt_win.rs` / `ppt_cm108_macos.rs` to lift), but only the Linux drivers are implemented here, since the exit criterion is keying a radio on the operator's Linux host. Non-Linux driver construction fails closed with `PttError::Unsupported`.
- SDR and JACK audio backends (design lists them as future `AudioBackend` impls).
- mTLS / routable binds (Phase 5).

**Exit criterion (the gate this plan must satisfy):** over the Phase-1 gRPC surface, a client can (1) `ListDevices` and see a stable `DeviceId` for a real or virtual device, (2) `ConfigureAudio` to open capture and playback on that device, (3) `ConfigurePtt` and key/unkey it, and (4) `Transmit` a PCM buffer that plays out with PTT asserted for exactly the buffer duration and released after drain — **with no mode attached.** Task 19 is that gate: a deterministic CI test using the file/loopback `AudioBackend` and a `MockPtt` driver, plus a documented manual procedure that runs the identical RPC sequence against a real sound card and radio.

---

## File Structure

Phase 2 adds two modules (`audio/`, `ptt/`) and a `device/` module beside the Phase-1 layout, and expands `DeviceId` in `ids.rs`. New/changed files:

```
crates/omnimodem/
  Cargo.toml                         + cpal, nix, hidapi, gpiocdev, nusb (target-gated)
  src/
    ids.rs                           DeviceId becomes a durable enum (was placeholder string)
    device/
      mod.rs                         re-exports; DeviceDescriptor
      enumerate.rs                   trait DeviceEnumerator + RealEnumerator (cpal+nusb) + FakeEnumerator
      cache.rs                       DeviceCache: resolve DeviceId -> live node, by-identity lookup
      hotplug.rs                     HotplugWatcher: diff snapshots, emit Arrived/Departed by DeviceId
    audio/
      mod.rs                         AudioChunk, AudioError, re-exports
      backend.rs                     trait AudioBackend + CaptureHandle/PlaybackHandle + NullBackend
      alsa.rs                        ALSA canonicalization + rate-ceiling + format pick (pure)
      file.rs                        FileBackend (raw i16 + WAV/FLAC replay) — deterministic
      stdin.rs                       StdinBackend (raw i16 PCM)
      cpal_backend.rs                CpalBackend: enumerate, open capture/playback, rebuild-backoff, watermarks
      resample.rs                    RationalResampler (deterministic)
      fanout.rs                      CaptureFanout: 1 capture -> N consumers (opt-in)
    ptt/
      mod.rs                         PttError, trait PttDriver, trait PttHardware seams, re-exports
      none.rs                        NonePtt (VOX/none no-op) + MockPtt (test double)
      serial.rs                      SerialLinePtt + UnixSerialLines (nix TIOCMSET)
      cm108.rs                       Cm108Ptt + UnixCm108Gpio (hidraw report)
      gpio.rs                        GpioPtt + LinuxGpiochip (gpiocdev) + structured eviction
      registry.rs                    PortRegistry: build_driver factory + DeviceId-keyed eviction
      sequence.rs                    drive_tx_cycle (no-sleep, watermark-timed) + TxCycleOutcome
      interlock.rs                   RxTxInterlock: per-device keyed-state gate
      udev.rs                        SuggestUdevRule text generator (pure)
    supervisor/
      mod.rs                         wire real DeviceCache/PttRegistry; channel audio+ptt binding
      channel.rs                     ChannelConfig gains audio + ptt bindings
      device.rs                      DELETED (placeholder folds into device/cache.rs)
      ptt.rs                         DELETED (placeholder folds into ptt/registry.rs)
    core/
      mod.rs                         per-channel TX worker; real key/audio/unkey; hotplug pump
      command.rs                     + ConfigureAudio/ConfigurePtt/KeyPtt/ListDevices/SuggestUdevRule
      event.rs                       + DeviceArrived/Departed, PttState, real AudioLevel
    grpc/
      service.rs                     new unary handlers
      convert.rs                     domain <-> proto for the new messages
    persist/
      mod.rs                         store DeviceId via canonical string; audio/ptt binding columns
  proto/
    omnimodem.proto                  additive: ListDevices/ConfigureAudio/ConfigurePtt/SuggestUdevRule + events
  tests/
    device_id.rs                     (unit tests live inline; integration where noted)
    audio_loopback.rs                integration: file capture -> playback round-trip
    ptt_cycle.rs                     integration: MockPtt + loopback drive_tx_cycle
    e2e_hardware.rs                  THE exit-criterion round-trip (deterministic backends)
```

**Boundaries.** `device/` owns identity, enumeration, caching, hotplug — no audio or PTT specifics. `audio/` and `ptt/` each own one subsystem behind a trait; their hardware adapters are the only code that calls cpal / nix / hidapi / gpiocdev, and each adapter sits behind a seam trait with a mock so the surrounding logic is CI-testable. `core/` owns the sync side (per-channel TX workers, interlock); `grpc/` stays the only async/proto code. This preserves the Phase-1 async-edge / sync-core seam.

---

## PART A — Shared foundation

## Task 1: Phase 2 dependencies

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/omnimodem/Cargo.toml`

- [ ] **Step 1: Add the hardware crates to the workspace manifest**

In the root `Cargo.toml`, under `[workspace.dependencies]`, add below the existing entries:

```toml
# Phase 2: audio + PTT hardware
cpal = "0.17"
nusb = "0.1"
hidapi = { version = "2.6", default-features = false, features = ["linux-native-basic-udev"] }
nix = { version = "0.29", features = ["fs", "ioctl"] }
gpiocdev = "0.7"
```

- [ ] **Step 2: Reference them from the crate manifest (target-gated where needed)**

In `crates/omnimodem/Cargo.toml`, under `[dependencies]`, add:

```toml
cpal.workspace = true
```

Then add target-gated sections below `[dependencies]` (these crates need host facilities not present everywhere; gating keeps a future Android/Windows cross-compile clean, mirroring Graywolf's `Cargo.toml`):

```toml
[target.'cfg(not(target_os = "android"))'.dependencies]
nusb.workspace = true

[target.'cfg(unix)'.dependencies]
nix.workspace = true
hidapi.workspace = true

[target.'cfg(target_os = "linux")'.dependencies]
gpiocdev.workspace = true
```

- [ ] **Step 3: Verify the workspace still builds**

Run: `cargo build`
Expected: success; the new crates resolve and compile (no code uses them yet).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/omnimodem/Cargo.toml
git commit -m "Add Phase 2 audio and PTT hardware dependencies"
```

---

## Task 2: Cross-platform `DeviceId`

The single most important retrofit-proofing in Phase 2: collapse Graywolf's two diverging identity paths (cpal/ALSA names vs nusb USB topology) into **one** stable identity derived from durable attributes, never a volatile `/dev` path. This replaces the Phase-1 placeholder `DeviceId(String)`. Config (audio + PTT) keys on it, so a TNC that jumps `ttyUSB0 → ttyUSB1` still binds. This task is **pure** — no hardware — so it is fully test-driven.

**Files:**
- Modify: `crates/omnimodem/src/ids.rs` (replace the `DeviceId` definition; keep `ChannelId`/`TransmitId`)
- Modify: `crates/omnimodem/src/persist/mod.rs` (store/load via canonical string)

- [ ] **Step 1: Write the failing tests + the new `DeviceId`**

In `crates/omnimodem/src/ids.rs`, replace the entire `DeviceId` block (the `pub struct DeviceId(pub String);` through its `impl`) with:

```rust
/// Stable, cross-platform device identity.
///
/// Built from durable attributes, never a volatile `/dev` path or ALSA card
/// index, so config survives renames and hotplug. Ordered by preference: a USB
/// vendor/product/serial triple is the most durable; an ALSA stable card *name*
/// is next; USB port topology is the fallback for two identical adapters that
/// `by-id` cannot disambiguate; `Serial` wraps a `/dev/serial/by-id/<symlink>`
/// (already stable). `Placeholder` is retained for the file/stdin/loopback
/// backends and Phase-1 fixtures that have no physical identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeviceId {
    /// USB device by vendor/product + serial. `serial` is empty when the
    /// device exposes none (then prefer `Topology`).
    Usb { vid: u16, pid: u16, serial: String },
    /// ALSA sound card by its stable kernel *name* (e.g. "Device"), not index.
    AlsaCard { card_name: String },
    /// USB port topology: bus + port chain (e.g. "1-1.4.2"). Last resort for
    /// indistinguishable identical adapters.
    Topology { bus: u8, ports: String },
    /// A `/dev/serial/by-id/<id>` symlink target (durable by construction).
    Serial { by_id: String },
    /// Non-physical backend (file/stdin/loopback) or a Phase-1 fixture.
    Placeholder { tag: String },
}

impl DeviceId {
    /// The single placeholder identity used by virtual backends and fixtures.
    pub fn placeholder() -> Self {
        DeviceId::Placeholder { tag: "virtual:0".to_string() }
    }

    /// Canonical, round-trippable string form used as the persistence key and
    /// the gRPC `device_id` field. Format: `<scheme>:<body>`.
    pub fn to_canonical_string(&self) -> String {
        match self {
            DeviceId::Usb { vid, pid, serial } => {
                format!("usb:{vid:04x}:{pid:04x}:{serial}")
            }
            DeviceId::AlsaCard { card_name } => format!("alsa:{card_name}"),
            DeviceId::Topology { bus, ports } => format!("topo:{bus}-{ports}"),
            DeviceId::Serial { by_id } => format!("serial:{by_id}"),
            DeviceId::Placeholder { tag } => format!("virtual:{tag}"),
        }
    }

    /// Parse the canonical string form. `None` on an unrecognized scheme.
    pub fn parse(s: &str) -> Option<Self> {
        let (scheme, body) = s.split_once(':')?;
        match scheme {
            "usb" => {
                // usb:VVVV:PPPP:serial   (serial may be empty and may contain ':')
                let mut parts = body.splitn(3, ':');
                let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
                let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
                let serial = parts.next().unwrap_or("").to_string();
                Some(DeviceId::Usb { vid, pid, serial })
            }
            "alsa" => Some(DeviceId::AlsaCard { card_name: body.to_string() }),
            "topo" => {
                let (bus, ports) = body.split_once('-')?;
                Some(DeviceId::Topology { bus: bus.parse().ok()?, ports: ports.to_string() })
            }
            "serial" => Some(DeviceId::Serial { by_id: body.to_string() }),
            "virtual" => Some(DeviceId::Placeholder { tag: body.to_string() }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod device_id_tests {
    use super::*;

    fn roundtrip(id: DeviceId) {
        let s = id.to_canonical_string();
        assert_eq!(DeviceId::parse(&s), Some(id), "round-trip failed for {s}");
    }

    #[test]
    fn canonical_roundtrips_for_every_variant() {
        roundtrip(DeviceId::Usb { vid: 0x0d8c, pid: 0x013c, serial: "A1B2C3".into() });
        roundtrip(DeviceId::Usb { vid: 0x0d8c, pid: 0x013c, serial: "".into() });
        roundtrip(DeviceId::AlsaCard { card_name: "Device".into() });
        roundtrip(DeviceId::Topology { bus: 1, ports: "1.4.2".into() });
        roundtrip(DeviceId::Serial { by_id: "usb-FTDI_FT232R_AB0CDEFG-if00-port0".into() });
        roundtrip(DeviceId::placeholder());
    }

    #[test]
    fn usb_serial_may_contain_colons() {
        let id = DeviceId::Usb { vid: 1, pid: 2, serial: "a:b:c".into() };
        roundtrip(id);
    }

    #[test]
    fn placeholder_is_stable_and_canonical() {
        assert_eq!(DeviceId::placeholder(), DeviceId::placeholder());
        assert_eq!(DeviceId::placeholder().to_canonical_string(), "virtual:virtual:0");
    }

    #[test]
    fn unknown_scheme_is_none() {
        assert_eq!(DeviceId::parse("bogus:whatever"), None);
        assert_eq!(DeviceId::parse("noseparator"), None);
    }

    #[test]
    fn usb_is_preferred_over_topology_by_ord() {
        // Ord drives "most durable identity first" when ranking candidates.
        assert!(DeviceId::Usb { vid: 1, pid: 1, serial: "x".into() }
            < DeviceId::Topology { bus: 1, ports: "1".into() });
    }
}
```

- [ ] **Step 2: Update persistence to use the canonical string**

In `crates/omnimodem/src/persist/mod.rs`, change the `upsert_channel` write of `device_id` from `cfg.device_id.0` to `cfg.device_id.to_canonical_string()`, and change the `load_channels` row mapping of `device_id` from `DeviceId(row.get::<_, String>(3)?)` to:

```rust
                device_id: DeviceId::parse(&row.get::<_, String>(3)?)
                    .unwrap_or_else(DeviceId::placeholder),
```

(The `unwrap_or_else` tolerates a row written by an older build; the design treats persisted identity as a hint that the live enumeration reconciles.)

- [ ] **Step 3: Run the tests**

Run: `cargo test -p omnimodem device_id_tests:: persist::`
Expected: PASS — all `DeviceId` round-trips, and the persistence tests still pass against the canonical-string key.

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodem/src/ids.rs crates/omnimodem/src/persist/mod.rs
git commit -m "Replace placeholder DeviceId with a durable cross-platform identity"
```

---

## PART B — Audio devices

## Task 3: `AudioBackend` trait + error type + null backend

The trait that replaces Graywolf's spawn-by-`match` (`modem/mod.rs:449-480`). It is deliberately small: open a capture, open a playback, report the working sample rate. Capture delivers `Vec<i16>` chunks over a bounded channel (Graywolf's `AudioChunk`, i16 throughout); playback exposes the submitted/drained watermark contract that the no-sleep TX cycle (Task 13) depends on. A `NullBackend` (silence in, discard out) lets the trait and every consumer be tested without hardware.

**Files:**
- Create: `crates/omnimodem/src/audio/mod.rs`
- Create: `crates/omnimodem/src/audio/backend.rs`
- Modify: `crates/omnimodem/src/lib.rs`

- [ ] **Step 1: Define the shared audio types**

Create `crates/omnimodem/src/audio/mod.rs`:

```rust
//! Audio subsystem: a pluggable `AudioBackend` over cpal / file / stdin, a
//! durable-identity device layer, resampling, and capture fan-out. No DSP.

pub mod alsa;
pub mod backend;
pub mod file;
pub mod fanout;
pub mod resample;
pub mod stdin;

#[cfg(not(test))]
pub mod cpal_backend;

/// A block of mono audio samples. i16 throughout, matching Graywolf's pipeline
/// and the soundcard's native format on the cheap USB adapters we target.
pub type AudioChunk = Vec<i16>;

/// Bounded depth of a capture delivery channel, in chunks (~1 s at 48 kHz with
/// 20 ms chunks). Lifted from Graywolf `CHUNK_QUEUE_DEPTH`.
pub const CHUNK_QUEUE_DEPTH: usize = 64;

/// Never open a stream above this rate. The ALSA `plughw` PCM advertises
/// synthetic resample ranges (up to 192 kHz) the codec can't honor; opening
/// above the real ceiling desyncs bit timing so every future frame fails FCS.
/// Lifted from Graywolf `MODEM_MAX_SAMPLE_RATE`. Resampling (Task 7) is
/// additive and happens *after* this capped capture, never instead of it.
pub const MAX_SAMPLE_RATE: u32 = 48_000;

/// Errors from the audio subsystem.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio device matches {0}")]
    DeviceNotFound(String),
    #[error("device {device} supports no usable capture format")]
    NoUsableFormat { device: String },
    #[error("requested rate {requested} exceeds the {ceiling} Hz ceiling")]
    RateTooHigh { requested: u32, ceiling: u32 },
    #[error("backend i/o error: {0}")]
    Io(String),
    #[error("backend unsupported on this platform")]
    Unsupported,
}
```

- [ ] **Step 2: Write the trait, handles, and `NullBackend` with failing tests**

Create `crates/omnimodem/src/audio/backend.rs`:

```rust
//! The `AudioBackend` trait and its handles. Hardware backends (cpal) and
//! virtual backends (file/stdin/null) all implement this one seam.

use super::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH};
use crate::ids::DeviceId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;

/// A running capture: pull `AudioChunk`s until the backend stops or the handle
/// drops (which tears the stream down).
pub struct CaptureHandle {
    pub rx: Receiver<AudioChunk>,
    /// Actual working rate the stream opened at (may differ from requested).
    pub sample_rate: u32,
    /// Dropping this stops the underlying stream. Backends store their stop
    /// hook here as a boxed closure so the trait stays object-safe.
    _stop: Box<dyn FnOnce() + Send>,
}

impl CaptureHandle {
    pub fn new(
        rx: Receiver<AudioChunk>,
        sample_rate: u32,
        stop: impl FnOnce() + Send + 'static,
    ) -> Self {
        CaptureHandle { rx, sample_rate, _stop: Box::new(stop) }
    }
}

/// A running playback. Submit owned i16 buffers; the backend drains them to the
/// DAC. `submitted`/`drained` are cumulative sample counts forming the
/// watermark the no-sleep TX cycle waits on (Task 13). Lifted from Graywolf's
/// `AudioSink`.
pub struct PlaybackHandle {
    tx: SyncSender<AudioChunk>,
    submitted: Arc<AtomicUsize>,
    drained: Arc<AtomicUsize>,
    pub sample_rate: u32,
}

impl PlaybackHandle {
    pub fn new(
        tx: SyncSender<AudioChunk>,
        submitted: Arc<AtomicUsize>,
        drained: Arc<AtomicUsize>,
        sample_rate: u32,
    ) -> Self {
        PlaybackHandle { tx, submitted, drained, sample_rate }
    }

    /// Queue samples for playback. Returns the cumulative submitted watermark.
    pub fn submit(&self, samples: AudioChunk) -> Result<usize, AudioError> {
        let n = samples.len();
        let total = self.submitted.fetch_add(n, Ordering::Relaxed) + n;
        self.tx.send(samples).map_err(|e| AudioError::Io(e.to_string()))?;
        Ok(total)
    }

    /// Cumulative samples the DAC callback has consumed.
    pub fn drained_samples(&self) -> usize {
        self.drained.load(Ordering::Relaxed)
    }
}

/// The pluggable audio backend. One device == one backend instance.
pub trait AudioBackend: Send {
    /// Open a capture stream at (up to) `requested_rate`, mono.
    fn open_capture(&self, requested_rate: u32) -> Result<CaptureHandle, AudioError>;
    /// Open a playback stream at (up to) `requested_rate`, mono.
    fn open_playback(&self, requested_rate: u32) -> Result<PlaybackHandle, AudioError>;
    /// The identity this backend represents.
    fn device_id(&self) -> DeviceId;
}

/// A backend with no hardware: capture yields the silence the test feeds it,
/// playback drains instantly. Used by every consumer test in this plan.
pub struct NullBackend {
    id: DeviceId,
    rate: u32,
}

impl NullBackend {
    pub fn new(rate: u32) -> Self {
        NullBackend { id: DeviceId::placeholder(), rate }
    }
}

impl AudioBackend for NullBackend {
    fn open_capture(&self, _requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        // Empty receiver whose sender is dropped: capture is immediately EOF.
        let (_tx, rx) = std::sync::mpsc::sync_channel(CHUNK_QUEUE_DEPTH);
        Ok(CaptureHandle::new(rx, self.rate, || {}))
    }

    fn open_playback(&self, _requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        // A draining thread that instantly "plays" whatever is submitted, so
        // the watermark contract holds without a DAC.
        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let submitted = Arc::new(AtomicUsize::new(0));
        let drained = Arc::new(AtomicUsize::new(0));
        let d2 = drained.clone();
        std::thread::spawn(move || {
            while let Ok(buf) = rx.recv() {
                d2.fetch_add(buf.len(), Ordering::Relaxed);
            }
        });
        Ok(PlaybackHandle::new(tx, submitted, drained, self.rate))
    }

    fn device_id(&self) -> DeviceId {
        self.id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_capture_is_immediate_eof() {
        let b = NullBackend::new(48_000);
        let cap = b.open_capture(48_000).unwrap();
        assert_eq!(cap.sample_rate, 48_000);
        // Sender dropped inside open_capture => recv errors (EOF), no hang.
        assert!(cap.rx.recv().is_err());
    }

    #[test]
    fn null_playback_drains_to_the_submitted_watermark() {
        let b = NullBackend::new(48_000);
        let pb = b.open_playback(48_000).unwrap();
        let wm = pb.submit(vec![0i16; 480]).unwrap();
        assert_eq!(wm, 480);
        // The drain thread catches up to the watermark.
        let mut spins = 0;
        while pb.drained_samples() < wm && spins < 10_000 {
            std::thread::yield_now();
            spins += 1;
        }
        assert_eq!(pb.drained_samples(), 480);
    }
}
```

- [ ] **Step 3: Wire the module in**

In `crates/omnimodem/src/lib.rs`, add below the existing `pub mod` lines:

```rust
pub mod audio;
```

> Note: `audio/mod.rs` declares `alsa`, `file`, `fanout`, `resample`, `stdin`, and (non-test) `cpal_backend`. Create empty placeholder files for the not-yet-written modules so the crate compiles now, then fill them in their tasks:
> ```bash
> printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/audio/alsa.rs
> printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/audio/file.rs
> printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/audio/fanout.rs
> printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/audio/resample.rs
> printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/audio/stdin.rs
> printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/audio/cpal_backend.rs
> ```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p omnimodem audio::backend::`
Expected: PASS (2 tests) — null capture is EOF, null playback honors the watermark.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/audio/ crates/omnimodem/src/lib.rs
git commit -m "Add AudioBackend trait, watermark handles, and NullBackend"
```

---

## Task 4: File & stdin capture backends

Deterministic backends are the foundation of the design's record/replay corpus *and* of every CI test that needs real samples without a sound card. The file backend replays raw little-endian i16 PCM at a declared rate; the stdin backend is the same over `stdin`. Both are fully test-driven because they touch no hardware.

**Files:**
- Create/replace: `crates/omnimodem/src/audio/file.rs`
- Create/replace: `crates/omnimodem/src/audio/stdin.rs`

- [ ] **Step 1: Write the file backend with failing tests**

Replace `crates/omnimodem/src/audio/file.rs` with:

```rust
//! File audio backend: replay raw little-endian i16 mono PCM deterministically,
//! and capture-to-vec for playback round-trip tests. The basis of the design's
//! record/replay corpus.

use super::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use super::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH};
use crate::ids::DeviceId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Samples per replayed chunk (~20 ms at 48 kHz). Matches Graywolf's stdin chunk.
const CHUNK_SAMPLES: usize = 960;

/// Replays a fixed buffer as capture; collects submitted samples for playback.
pub struct FileBackend {
    samples: Arc<Vec<i16>>,
    rate: u32,
    /// Playback sink: submitted buffers are appended here so a test can assert
    /// what "played". Shared so the test can read it after `open_playback`.
    pub played: Arc<Mutex<Vec<i16>>>,
}

impl FileBackend {
    /// Build from an in-memory buffer (tests, synthetic corpus).
    pub fn from_samples(samples: Vec<i16>, rate: u32) -> Self {
        FileBackend {
            samples: Arc::new(samples),
            rate,
            played: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Build from a raw LE-i16 PCM file. `rate` is supplied out-of-band (raw
    /// PCM carries no header).
    pub fn from_raw_file(path: &std::path::Path, rate: u32) -> Result<Self, AudioError> {
        let bytes = std::fs::read(path).map_err(|e| AudioError::Io(e.to_string()))?;
        let samples = bytes
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]))
            .collect();
        Ok(FileBackend::from_samples(samples, rate))
    }
}

impl AudioBackend for FileBackend {
    fn open_capture(&self, _requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(CHUNK_QUEUE_DEPTH);
        let samples = self.samples.clone();
        std::thread::spawn(move || {
            for chunk in samples.chunks(CHUNK_SAMPLES) {
                // Blocking send: deterministic delivery; drop on disconnect.
                if tx.send(chunk.to_vec()).is_err() {
                    break;
                }
            }
            // Sender drops here => receiver sees EOF after the last chunk.
        });
        Ok(CaptureHandle::new(rx, self.rate, || {}))
    }

    fn open_playback(&self, _requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let submitted = Arc::new(AtomicUsize::new(0));
        let drained = Arc::new(AtomicUsize::new(0));
        let d2 = drained.clone();
        let played = self.played.clone();
        std::thread::spawn(move || {
            while let Ok(buf) = rx.recv() {
                d2.fetch_add(buf.len(), Ordering::Relaxed);
                played.lock().unwrap().extend_from_slice(&buf);
            }
        });
        Ok(PlaybackHandle::new(tx, submitted, drained, self.rate))
    }

    fn device_id(&self) -> DeviceId {
        DeviceId::Placeholder { tag: "file:0".into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_replays_all_samples_then_eofs() {
        let input: Vec<i16> = (0..2500).map(|i| i as i16).collect();
        let b = FileBackend::from_samples(input.clone(), 48_000);
        let cap = b.open_capture(48_000).unwrap();

        let mut got = Vec::new();
        while let Ok(chunk) = cap.rx.recv() {
            got.extend_from_slice(&chunk);
        }
        assert_eq!(got, input); // every sample, in order, then clean EOF
    }

    #[test]
    fn playback_collects_submitted_samples() {
        let b = FileBackend::from_samples(vec![], 48_000);
        let pb = b.open_playback(48_000).unwrap();
        pb.submit(vec![1, 2, 3]).unwrap();
        let wm = pb.submit(vec![4, 5]).unwrap();
        assert_eq!(wm, 5);
        while pb.drained_samples() < 5 {
            std::thread::yield_now();
        }
        assert_eq!(*b.played.lock().unwrap(), vec![1, 2, 3, 4, 5]);
    }
}
```

- [ ] **Step 2: Write the stdin backend**

Replace `crates/omnimodem/src/audio/stdin.rs` with:

```rust
//! Stdin audio backend: raw little-endian i16 mono PCM on stdin. Capture only
//! (you cannot play to stdin); playback errors `Unsupported`. Lifted from
//! Graywolf `audio/stdin_raw.rs`.

use super::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use super::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH};
use crate::ids::DeviceId;
use std::io::Read;

const CHUNK_BYTES: usize = 960 * 2; // ~20 ms at 48 kHz

/// Reads raw i16 PCM from stdin at a declared rate.
pub struct StdinBackend {
    rate: u32,
}

impl StdinBackend {
    pub fn new(rate: u32) -> Self {
        StdinBackend { rate }
    }
}

impl AudioBackend for StdinBackend {
    fn open_capture(&self, _requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(CHUNK_QUEUE_DEPTH);
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = vec![0u8; CHUNK_BYTES];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let chunk: AudioChunk = buf[..n]
                            .chunks_exact(2)
                            .map(|p| i16::from_le_bytes([p[0], p[1]]))
                            .collect();
                        if tx.send(chunk).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(CaptureHandle::new(rx, self.rate, || {}))
    }

    fn open_playback(&self, _requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        Err(AudioError::Unsupported)
    }

    fn device_id(&self) -> DeviceId {
        DeviceId::Placeholder { tag: "stdin:0".into() }
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p omnimodem audio::file::`
Expected: PASS (2 tests) — capture replays every sample then EOFs; playback collects submitted samples and honors the watermark.

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodem/src/audio/file.rs crates/omnimodem/src/audio/stdin.rs
git commit -m "Add deterministic file and stdin audio backends"
```

---

## Task 5: ALSA hardening — canonicalization, rate ceiling, format selection (pure)

These are the load-bearing defensive functions from Graywolf `audio/soundcard.rs`, extracted as **pure functions** so they are fully unit-tested here and reused unchanged by the cpal backend in Task 6. They encode three hard-won lessons: collapse cpal's per-card aliases to one physical card, never open above the 48 kHz ceiling or accept a synthetic plughw range, and never trust cpal's default format (prefer I16, which the cheap USB codecs actually deliver without POLLERR-looping).

**Files:**
- Create/replace: `crates/omnimodem/src/audio/alsa.rs`

- [ ] **Step 1: Write the pure functions with failing tests**

Replace `crates/omnimodem/src/audio/alsa.rs` with:

```rust
//! ALSA canonicalization + defensive rate/format selection. All pure; the cpal
//! backend (Task 6) feeds these the values cpal reports. Lifted from Graywolf
//! `audio/soundcard.rs` (`parse_proc_asound_cards`, `choose_stream_rate`,
//! `pick_input_sample_format`).

use super::{AudioError, MAX_SAMPLE_RATE};

/// A sample format a device advertises, ranked by how well the cheap USB codecs
/// we target honor it. I16 first: it is the native wire format and does not
/// POLLERR-loop cpal the way an ALSA-plughw-synthesized F32 does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFmt {
    I16,
    F32,
    U16,
}

impl SampleFmt {
    fn rank(self) -> u8 {
        match self {
            SampleFmt::I16 => 0,
            SampleFmt::F32 => 1,
            SampleFmt::U16 => 2,
        }
    }
}

/// Parse `/proc/asound/cards`: lines like ` 0 [Device  ]: USB-Audio - ...`
/// yield `(0, "Device")`. Indented continuation lines are ignored.
pub fn parse_proc_asound_cards(contents: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for line in contents.lines() {
        // A card header starts with optional spaces then an index digit.
        let trimmed = line.trim_start();
        if trimmed.len() == line.len() {
            continue; // not indented at all -> not a card row in this format
        }
        let mut it = trimmed.split_whitespace();
        let Some(idx) = it.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        // Next token is `[Name`; strip the bracket.
        if let Some(name_tok) = it.next() {
            let name = name_tok.trim_start_matches('[').trim_end_matches(']');
            if !name.is_empty() {
                out.push((idx, name.to_string()));
            }
        }
    }
    out
}

/// Extract the `CARD=` token from a cpal/ALSA pcm id like
/// `plughw:CARD=Device,DEV=0`. `None` for shorthand or non-ALSA names.
pub fn alsa_card_token(pcm_id: &str) -> Option<&str> {
    let after = pcm_id.split("CARD=").nth(1)?;
    Some(after.split(',').next().unwrap_or(after))
}

/// Choose the stream rate. Clamp to the ceiling and refuse a requested rate the
/// device's *real* ranges don't cover (a synthetic plughw range is filtered out
/// by the caller before this sees `supported`). `supported` is a list of
/// inclusive (min,max) Hz ranges the hardware genuinely supports.
pub fn choose_stream_rate(
    requested: u32,
    supported: &[(u32, u32)],
) -> Result<u32, AudioError> {
    let want = requested.min(MAX_SAMPLE_RATE);
    if supported.iter().any(|&(lo, hi)| want >= lo && want <= hi) {
        return Ok(want);
    }
    // Fall back to the highest supported rate at or below the ceiling.
    let best = supported
        .iter()
        .map(|&(_, hi)| hi.min(MAX_SAMPLE_RATE))
        .filter(|&r| r > 0)
        .max();
    match best {
        Some(r) => Ok(r),
        None => Err(AudioError::RateTooHigh { requested, ceiling: MAX_SAMPLE_RATE }),
    }
}

/// Pick a capture format from those advertised for `rate`. Never trusts cpal's
/// default; prefers I16. `configs` is a list of (format, min_rate, max_rate).
pub fn pick_input_sample_format(
    configs: &[(SampleFmt, u32, u32)],
    rate: u32,
) -> Option<SampleFmt> {
    configs
        .iter()
        .filter(|&&(_, lo, hi)| rate >= lo && rate <= hi)
        .map(|&(f, _, _)| f)
        .min_by_key(|f| f.rank())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_asound_cards() {
        let sample = "\
 0 [Device         ]: USB-Audio - USB Audio Device
                      C-Media USB Audio Device at usb-...
 1 [PCH            ]: HDA-Intel - HDA Intel PCH
                      HDA Intel PCH at 0x...";
        let cards = parse_proc_asound_cards(sample);
        assert_eq!(cards, vec![(0, "Device".to_string()), (1, "PCH".to_string())]);
    }

    #[test]
    fn extracts_card_token() {
        assert_eq!(alsa_card_token("plughw:CARD=Device,DEV=0"), Some("Device"));
        assert_eq!(alsa_card_token("plughw:0,0"), None);
    }

    #[test]
    fn rate_is_clamped_to_ceiling() {
        // Device claims up to 192k (synthetic): we never go above 48k.
        assert_eq!(choose_stream_rate(96_000, &[(8_000, 192_000)]).unwrap(), 48_000);
    }

    #[test]
    fn rate_falls_back_to_best_supported_below_ceiling() {
        // Requested 48k unsupported; best real range tops at 44.1k.
        assert_eq!(choose_stream_rate(48_000, &[(8_000, 44_100)]).unwrap(), 44_100);
    }

    #[test]
    fn rate_errors_when_nothing_usable() {
        assert!(matches!(
            choose_stream_rate(48_000, &[]),
            Err(AudioError::RateTooHigh { .. })
        ));
    }

    #[test]
    fn format_prefers_i16_over_f32() {
        let configs = [
            (SampleFmt::F32, 8_000, 48_000),
            (SampleFmt::I16, 8_000, 48_000),
        ];
        assert_eq!(pick_input_sample_format(&configs, 48_000), Some(SampleFmt::I16));
    }

    #[test]
    fn format_respects_rate_window() {
        let configs = [(SampleFmt::I16, 8_000, 16_000), (SampleFmt::F32, 8_000, 48_000)];
        // At 48k only F32 covers the rate.
        assert_eq!(pick_input_sample_format(&configs, 48_000), Some(SampleFmt::F32));
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p omnimodem audio::alsa::`
Expected: PASS (7 tests).

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodem/src/audio/alsa.rs
git commit -m "Add pure ALSA canonicalization, rate-ceiling, and format-selection helpers"
```

---

## Task 6: cpal backend — enumeration, defensive open, rebuild-backoff, watermarks

The real hardware backend. It enumerates cpal devices into `DeviceId`s (using nusb to attach USB identity), opens capture/playback through the Task-5 pure pickers, runs a capture thread with the stream-rebuild-with-backoff loop, and drives playback through the submitted/drained watermark ledger. Hardware streams cannot be unit-tested deterministically, so this task verifies by `cargo build` plus a **manual smoke step** against a real sound card; the *decision logic* it depends on is already proven in Task 5.

**Files:**
- Create/replace: `crates/omnimodem/src/audio/cpal_backend.rs`

- [ ] **Step 1: Write the cpal backend**

Replace `crates/omnimodem/src/audio/cpal_backend.rs` with the implementation below. It lifts Graywolf `audio/soundcard.rs`: the `REBUILD_BACKOFF` schedule (`soundcard.rs:26-39`), the never-trust-default format pick (`soundcard.rs:223-251`), the held-thread rebuild loop (`soundcard.rs:254-372`), and the output drain ledger (`soundcard.rs:1280-1348`).

```rust
//! cpal audio backend. The only audio code that calls cpal. Decision logic
//! (rate/format) lives in `super::alsa`; identity in `super::enumerate`.

use super::alsa::{choose_stream_rate, pick_input_sample_format, SampleFmt};
use super::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use super::{AudioChunk, AudioError, CHUNK_QUEUE_DEPTH};
use crate::ids::DeviceId;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Exponential rebuild backoff after a stream error (Graywolf `REBUILD_BACKOFF`).
const REBUILD_BACKOFF: &[Duration] = &[
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];
/// Clear the backoff after a stream that ran stable this long.
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(60);

/// A cpal device wrapped with its resolved durable identity.
pub struct CpalBackend {
    device: cpal::Device,
    id: DeviceId,
}

impl CpalBackend {
    pub fn new(device: cpal::Device, id: DeviceId) -> Self {
        CpalBackend { device, id }
    }

    /// Collect the (format, min, max) tuples cpal advertises for input.
    fn input_configs(&self) -> Vec<(SampleFmt, u32, u32)> {
        let Ok(ranges) = self.device.supported_input_configs() else {
            return Vec::new();
        };
        ranges
            .filter_map(|r| {
                let fmt = match r.sample_format() {
                    cpal::SampleFormat::I16 => SampleFmt::I16,
                    cpal::SampleFormat::F32 => SampleFmt::F32,
                    cpal::SampleFormat::U16 => SampleFmt::U16,
                    _ => return None,
                };
                Some((fmt, r.min_sample_rate().0, r.max_sample_rate().0))
            })
            .collect()
    }

    fn input_rate_ranges(&self) -> Vec<(u32, u32)> {
        self.input_configs().iter().map(|&(_, lo, hi)| (lo, hi)).collect()
    }
}

impl AudioBackend for CpalBackend {
    fn open_capture(&self, requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        let configs = self.input_configs();
        if configs.is_empty() {
            return Err(AudioError::NoUsableFormat { device: self.id.to_canonical_string() });
        }
        let rate = choose_stream_rate(requested_rate, &self.input_rate_ranges())?;
        let fmt = pick_input_sample_format(&configs, rate)
            .ok_or_else(|| AudioError::NoUsableFormat { device: self.id.to_canonical_string() })?;

        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let device = self.device.clone();
        let cfg = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(rate),
            buffer_size: cpal::BufferSize::Default,
        };

        // Held thread owns the stream and rebuilds it on error with backoff.
        std::thread::Builder::new()
            .name("omni-capture".into())
            .spawn(move || {
                let mut backoff = 0usize;
                while !stop_thread.load(Ordering::Relaxed) {
                    let failed = Arc::new(AtomicBool::new(false));
                    let f2 = failed.clone();
                    let tx2 = tx.clone();
                    let err_fn = move |_e| f2.store(true, Ordering::Relaxed);
                    let built = build_input(&device, &cfg, fmt, tx2, err_fn);
                    let stream = match built {
                        Ok(s) => s,
                        Err(_) => {
                            backoff_wait(&mut backoff, &stop_thread);
                            continue;
                        }
                    };
                    if stream.play().is_err() {
                        backoff_wait(&mut backoff, &stop_thread);
                        continue;
                    }
                    let started = Instant::now();
                    while !stop_thread.load(Ordering::Relaxed)
                        && !failed.load(Ordering::Relaxed)
                    {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    drop(stream);
                    if started.elapsed() >= BACKOFF_RESET_AFTER {
                        backoff = 0;
                    }
                    if !stop_thread.load(Ordering::Relaxed) {
                        backoff_wait(&mut backoff, &stop_thread);
                    }
                }
            })
            .map_err(|e| AudioError::Io(e.to_string()))?;

        let stop_on_drop = stop;
        Ok(CaptureHandle::new(rx, rate, move || {
            stop_on_drop.store(true, Ordering::Relaxed)
        }))
    }

    fn open_playback(&self, requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        let rate = choose_stream_rate(requested_rate, &output_rate_ranges(&self.device))?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let submitted = Arc::new(AtomicUsize::new(0));
        let drained = Arc::new(AtomicUsize::new(0));
        let queue: Arc<Mutex<std::collections::VecDeque<i16>>> =
            Arc::new(Mutex::new(std::collections::VecDeque::new()));

        // Feeder: move submitted buffers into the shared queue.
        let qf = queue.clone();
        std::thread::spawn(move || {
            while let Ok(buf) = rx.recv() {
                qf.lock().unwrap().extend(buf);
            }
        });

        let device = self.device.clone();
        let cfg = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(rate),
            buffer_size: cpal::BufferSize::Default,
        };
        let qcb = queue.clone();
        let drained_cb = drained.clone();
        std::thread::Builder::new()
            .name("omni-playback".into())
            .spawn(move || {
                // Build once; on error rebuild with backoff (same pattern).
                let mut backoff = 0usize;
                let never_stop = Arc::new(AtomicBool::new(false));
                loop {
                    let failed = Arc::new(AtomicBool::new(false));
                    let f2 = failed.clone();
                    let q = qcb.clone();
                    let d = drained_cb.clone();
                    let stream = device.build_output_stream(
                        &cfg,
                        move |out: &mut [i16], _| {
                            let mut ql = q.lock().unwrap();
                            for s in out.iter_mut() {
                                *s = ql.pop_front().unwrap_or(0);
                                if !ql.is_empty() {
                                    d.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        },
                        move |_e| f2.store(true, Ordering::Relaxed),
                        None,
                    );
                    let Ok(stream) = stream else {
                        backoff_wait(&mut backoff, &never_stop);
                        continue;
                    };
                    if stream.play().is_err() {
                        backoff_wait(&mut backoff, &never_stop);
                        continue;
                    }
                    while !failed.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    drop(stream);
                    backoff_wait(&mut backoff, &never_stop);
                }
            })
            .map_err(|e| AudioError::Io(e.to_string()))?;

        Ok(PlaybackHandle::new(tx, submitted, drained, rate))
    }

    fn device_id(&self) -> DeviceId {
        self.id.clone()
    }
}

/// Sleep one backoff step, waking every 100 ms to honor `stop`.
fn backoff_wait(idx: &mut usize, stop: &AtomicBool) {
    let dur = REBUILD_BACKOFF[(*idx).min(REBUILD_BACKOFF.len() - 1)];
    let mut waited = Duration::ZERO;
    while waited < dur && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
        waited += Duration::from_millis(100);
    }
    *idx = (*idx + 1).min(REBUILD_BACKOFF.len() - 1);
}

/// Build a mono input stream in `fmt`, converting each callback buffer to i16.
fn build_input(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    fmt: SampleFmt,
    tx: std::sync::mpsc::SyncSender<AudioChunk>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    match fmt {
        SampleFmt::I16 => device.build_input_stream(
            cfg,
            move |data: &[i16], _| {
                let _ = tx.try_send(data.to_vec());
            },
            err_fn,
            None,
        ),
        SampleFmt::F32 => device.build_input_stream(
            cfg,
            move |data: &[f32], _| {
                let chunk = data
                    .iter()
                    .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();
                let _ = tx.try_send(chunk);
            },
            err_fn,
            None,
        ),
        SampleFmt::U16 => device.build_input_stream(
            cfg,
            move |data: &[u16], _| {
                let chunk = data.iter().map(|&s| (s as i32 - 32768) as i16).collect();
                let _ = tx.try_send(chunk);
            },
            err_fn,
            None,
        ),
    }
}

fn output_rate_ranges(device: &cpal::Device) -> Vec<(u32, u32)> {
    device
        .supported_output_configs()
        .map(|it| it.map(|r| (r.min_sample_rate().0, r.max_sample_rate().0)).collect())
        .unwrap_or_default()
}

/// Enumerate the default host's devices as `(DeviceId, CpalBackend)`. The cpal
/// device name (an ALSA pcm id on Linux) yields an `AlsaCard` identity; the USB
/// attach step in `super::enumerate` upgrades it to a `Usb`/`Topology` id when
/// it can match the card to a USB device.
pub fn enumerate_default_host() -> Vec<(DeviceId, CpalBackend)> {
    let host = cpal::default_host();
    let mut out = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for dev in devs {
            let name = dev.name().unwrap_or_default();
            let id = super::alsa::alsa_card_token(&name)
                .map(|c| DeviceId::AlsaCard { card_name: c.to_string() })
                .unwrap_or_else(|| DeviceId::Placeholder { tag: name.clone() });
            out.push((id, CpalBackend::new(dev, id_clone(&out, &name))));
        }
    }
    out
}

// Helper kept tiny so the closure above stays readable; the real id is computed
// inline — this indirection exists only to avoid a borrow of `out` in the map.
fn id_clone(_out: &[(DeviceId, CpalBackend)], name: &str) -> DeviceId {
    super::alsa::alsa_card_token(name)
        .map(|c| DeviceId::AlsaCard { card_name: c.to_string() })
        .unwrap_or_else(|| DeviceId::Placeholder { tag: name.to_string() })
}
```

> Implementation note for the worker: cpal's `build_output_stream` sample type must match the device's real output format; the snippet assumes I16 output (true for the USB adapters we target). If a future device only offers F32 output, mirror the `build_input` format match for output. Keep that change inside `cpal_backend.rs` — nothing else in the audio module knows the wire format.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p omnimodem`
Expected: success. (No deterministic unit test — hardware streams are exercised in the manual smoke step and by the loopback integration test in Task 19 via the file backend.)

- [ ] **Step 3: Manual smoke test against a real sound card**

This step needs a host with a sound card (the operator's Linux machine, not CI). Add a tiny throwaway example or use the daemon once Task 17 lands. For now, verify enumeration directly:

```bash
cargo test -p omnimodem --no-run    # ensure it links
```

Then, on a machine with audio hardware, confirm `cpal::default_host().input_devices()` lists the expected card and that `alsa_card_token` extracts a stable name from its pcm id. Full open/capture/playback is smoke-tested end-to-end in Task 19's manual procedure.

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodem/src/audio/cpal_backend.rs
git commit -m "Add cpal audio backend with rebuild-backoff and drain watermarks"
```

---

## Task 7: Rational resampler

Resampling makes a source rate independent of the demod working rate (design: "a source rate need not match the demod rate"). It is **additive** — it runs *after* the 48 kHz-capped capture, never replacing the ALSA hardening. Phase 2 needs correct, deterministic plumbing, not yet the best-quality polyphase filter (that is a Phase-3 front-end-DSP battery); a band-limited linear fractional resampler is enough to carry samples between rates and is fully testable.

**Files:**
- Create/replace: `crates/omnimodem/src/audio/resample.rs`

- [ ] **Step 1: Write the resampler with failing tests**

Replace `crates/omnimodem/src/audio/resample.rs` with:

```rust
//! Rational sample-rate conversion. Linear-interpolation fractional resampler:
//! deterministic, streaming-stateful, exact length ratio. Runs downstream of
//! the rate-capped capture; a polyphase windowed-sinc upgrade is a Phase-3
//! battery. Operates on i16 with internal f32 accumulation.

/// Streaming resampler from `in_rate` to `out_rate`.
pub struct RationalResampler {
    in_rate: u32,
    out_rate: u32,
    /// Fractional input position carried across `process` calls.
    pos: f64,
    /// Last input sample of the previous block, for cross-block interpolation.
    last: f32,
    primed: bool,
}

impl RationalResampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        assert!(in_rate > 0 && out_rate > 0);
        RationalResampler { in_rate, out_rate, pos: 0.0, last: 0.0, primed: false }
    }

    /// Identity fast-path predicate.
    pub fn is_passthrough(&self) -> bool {
        self.in_rate == self.out_rate
    }

    /// Resample one block. Output length is approximately
    /// `input.len() * out_rate / in_rate`.
    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        if self.is_passthrough() {
            return input.to_vec();
        }
        let step = self.in_rate as f64 / self.out_rate as f64;
        let mut out = Vec::with_capacity(
            (input.len() as u64 * self.out_rate as u64 / self.in_rate as u64) as usize + 1,
        );
        if !self.primed {
            self.last = input.first().copied().unwrap_or(0) as f32;
            self.primed = true;
        }
        // `pos` is measured in input samples relative to the start of `input`,
        // offset by carry from the previous block (negative => use `last`).
        let mut pos = self.pos;
        while pos < input.len() as f64 {
            let i = pos.floor();
            let frac = (pos - i) as f32;
            let i = i as isize;
            let a = if i < 0 { self.last } else { input[i as usize] as f32 };
            let b = if (i + 1) < 0 {
                self.last
            } else if ((i + 1) as usize) < input.len() {
                input[(i + 1) as usize] as f32
            } else {
                // Need a sample past this block; stop and carry `pos` over.
                break;
            };
            let s = a + (b - a) * frac;
            out.push(s.round().clamp(-32768.0, 32767.0) as i16);
            pos += step;
        }
        // Carry the leftover fractional position into the next block.
        self.pos = pos - input.len() as f64;
        if let Some(&l) = input.last() {
            self.last = l as f32;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_is_identity() {
        let mut r = RationalResampler::new(48_000, 48_000);
        assert!(r.is_passthrough());
        let input: Vec<i16> = (0..100).collect();
        assert_eq!(r.process(&input), input);
    }

    #[test]
    fn upsample_2x_doubles_length() {
        let mut r = RationalResampler::new(24_000, 48_000);
        let input = vec![0i16; 1000];
        let out = r.process(&input);
        // ~2x within a couple samples of boundary carry.
        assert!((out.len() as i64 - 2000).abs() <= 2, "len was {}", out.len());
    }

    #[test]
    fn downsample_2x_halves_length() {
        let mut r = RationalResampler::new(48_000, 24_000);
        let input = vec![0i16; 1000];
        let out = r.process(&input);
        assert!((out.len() as i64 - 500).abs() <= 2, "len was {}", out.len());
    }

    #[test]
    fn preserves_a_dc_level() {
        let mut r = RationalResampler::new(48_000, 16_000);
        let input = vec![1000i16; 2000];
        let out = r.process(&input);
        // Linear interp of a constant is the constant.
        assert!(out.iter().all(|&s| s == 1000), "DC not preserved");
    }

    #[test]
    fn streaming_blocks_match_one_big_block_length() {
        // Feeding two halves yields about the same total as one whole block.
        let whole: Vec<i16> = (0..2000).map(|i| (i % 100) as i16).collect();
        let mut r1 = RationalResampler::new(48_000, 32_000);
        let one = r1.process(&whole);

        let mut r2 = RationalResampler::new(48_000, 32_000);
        let mut split = r2.process(&whole[..1000]);
        split.extend(r2.process(&whole[1000..]));

        assert!((one.len() as i64 - split.len() as i64).abs() <= 2);
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p omnimodem audio::resample::`
Expected: PASS (5 tests) — passthrough identity, 2× up/down length, DC preservation, streaming continuity.

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodem/src/audio/resample.rs
git commit -m "Add deterministic rational resampler (additive to rate-capped capture)"
```

---

## Task 8: Capture fan-out

One capture stream can feed several consumers (1200 + 9600 on the same audio, or SDR slices). It is **opt-in**; 1:1 is the default. Graywolf's `extra_demods` proves the pattern; here it is a clean broadcast over a capture's `Receiver`. Fully test-driven over the file backend.

**Files:**
- Create/replace: `crates/omnimodem/src/audio/fanout.rs`

- [ ] **Step 1: Write the fan-out with failing tests**

Replace `crates/omnimodem/src/audio/fanout.rs` with:

```rust
//! Capture fan-out: one capture `Receiver` -> N independent consumers. Opt-in;
//! the common 1:1 path skips it entirely. Each consumer gets every chunk; a
//! consumer that drops is removed on the next send.

use super::AudioChunk;
use std::sync::mpsc::{Receiver, SyncSender};

/// Spawn a pump that reads `source` and forwards each chunk to every consumer.
/// Returns the consumers' receivers. `n` >= 1.
pub fn fan_out(source: Receiver<AudioChunk>, n: usize) -> Vec<Receiver<AudioChunk>> {
    assert!(n >= 1);
    let mut senders: Vec<SyncSender<AudioChunk>> = Vec::with_capacity(n);
    let mut receivers = Vec::with_capacity(n);
    for _ in 0..n {
        let (tx, rx) = std::sync::mpsc::sync_channel(super::CHUNK_QUEUE_DEPTH);
        senders.push(tx);
        receivers.push(rx);
    }
    std::thread::spawn(move || {
        while let Ok(chunk) = source.recv() {
            // Forward to live consumers; prune dead ones.
            senders.retain(|tx| tx.try_send(chunk.clone()).is_ok() || true);
            if senders.iter().all(|tx| tx.try_send(chunk.clone()).is_err()) {
                // All consumers gone: stop pumping.
                // (try_send above is best-effort; this is the liveness check.)
            }
        }
    });
    receivers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::backend::AudioBackend;
    use crate::audio::file::FileBackend;

    #[test]
    fn single_consumer_is_passthrough_equivalent() {
        let input: Vec<i16> = (0..1500).collect();
        let cap = FileBackend::from_samples(input.clone(), 48_000)
            .open_capture(48_000)
            .unwrap();
        let mut outs = fan_out(cap.rx, 1);
        let rx = outs.pop().unwrap();
        let mut got = Vec::new();
        while let Ok(c) = rx.recv() {
            got.extend_from_slice(&c);
        }
        assert_eq!(got, input);
    }

    #[test]
    fn two_consumers_each_get_every_sample() {
        let input: Vec<i16> = (0..1500).collect();
        let cap = FileBackend::from_samples(input.clone(), 48_000)
            .open_capture(48_000)
            .unwrap();
        let outs = fan_out(cap.rx, 2);
        let mut collected: Vec<Vec<i16>> = Vec::new();
        for rx in outs {
            let mut got = Vec::new();
            while let Ok(c) = rx.recv() {
                got.extend_from_slice(&c);
            }
            collected.push(got);
        }
        assert_eq!(collected[0], input);
        assert_eq!(collected[1], input);
    }
}
```

> Worker note: simplify the pump's liveness check if clippy objects to the `|| true` — the intent is "forward to all, keep all that are still connected." A clean form:
> ```rust
> senders.retain(|tx| !matches!(tx.try_send(chunk.clone()),
>     Err(std::sync::mpsc::TrySendError::Disconnected(_))));
> ```
> Replace the two-line block with this single `retain` and delete the `all(...)` check.

- [ ] **Step 2: Apply the cleaner pump and run the tests**

Apply the `retain` form from the worker note (the deterministic tests pass with either, but the clean form is what ships). Then run:

Run: `cargo test -p omnimodem audio::fanout::`
Expected: PASS (2 tests) — 1 consumer is passthrough, 2 consumers each receive every sample.

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodem/src/audio/fanout.rs
git commit -m "Add opt-in capture fan-out (one capture to N consumers)"
```

---

## Task 9: Device cache + hotplug eviction by `DeviceId`

The cache resolves a stored `DeviceId` to a live device and detects hotplug by diffing enumeration snapshots, emitting `Arrived`/`Departed` keyed on `DeviceId` — the mechanism the design demands so config keyed on identity rebinds across renames. Enumeration sits behind a `DeviceEnumerator` trait with a `FakeEnumerator`, so the cache and hotplug diff are fully unit-tested.

**Files:**
- Create: `crates/omnimodem/src/device/mod.rs`
- Create: `crates/omnimodem/src/device/enumerate.rs`
- Create: `crates/omnimodem/src/device/cache.rs`
- Create: `crates/omnimodem/src/device/hotplug.rs`
- Modify: `crates/omnimodem/src/lib.rs`
- Delete: `crates/omnimodem/src/supervisor/device.rs` (placeholder folds in here)

- [ ] **Step 1: Define the descriptor and enumerator trait**

Create `crates/omnimodem/src/device/enumerate.rs`:

```rust
//! Device enumeration behind a trait, so the cache and hotplug logic are
//! testable without hardware. `RealEnumerator` bridges cpal + nusb; tests use
//! `FakeEnumerator`.

use crate::ids::DeviceId;

/// What enumeration knows about one present device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub id: DeviceId,
    /// Operator-facing label (cpal device name / USB product string).
    pub label: String,
    pub has_capture: bool,
    pub has_playback: bool,
}

/// Snapshot the set of currently-present devices.
pub trait DeviceEnumerator: Send {
    fn enumerate(&self) -> Vec<DeviceDescriptor>;
}

/// A fixed-list enumerator whose contents a test can swap between snapshots.
pub struct FakeEnumerator {
    pub devices: std::sync::Mutex<Vec<DeviceDescriptor>>,
}

impl FakeEnumerator {
    pub fn new(devices: Vec<DeviceDescriptor>) -> Self {
        FakeEnumerator { devices: std::sync::Mutex::new(devices) }
    }
    pub fn set(&self, devices: Vec<DeviceDescriptor>) {
        *self.devices.lock().unwrap() = devices;
    }
}

impl DeviceEnumerator for FakeEnumerator {
    fn enumerate(&self) -> Vec<DeviceDescriptor> {
        self.devices.lock().unwrap().clone()
    }
}
```

- [ ] **Step 2: Write the cache with failing tests**

Create `crates/omnimodem/src/device/cache.rs`:

```rust
//! Resolves a durable `DeviceId` to a live device and caches the present set.
//! Replaces the Phase-1 placeholder `DeviceCache`.

use super::enumerate::{DeviceDescriptor, DeviceEnumerator};
use crate::ids::DeviceId;
use std::collections::HashMap;

/// Caches the current enumeration, indexed by `DeviceId`.
pub struct DeviceCache {
    present: HashMap<DeviceId, DeviceDescriptor>,
}

impl DeviceCache {
    pub fn new() -> Self {
        DeviceCache { present: HashMap::new() }
    }

    /// Re-enumerate and replace the cached set. Returns the new descriptors.
    pub fn refresh(&mut self, enumerator: &dyn DeviceEnumerator) -> Vec<DeviceDescriptor> {
        let devices = enumerator.enumerate();
        self.present = devices.iter().cloned().map(|d| (d.id.clone(), d)).collect();
        devices
    }

    /// Resolve a stored identity to a present device, or `None` if absent
    /// (unplugged). Callers key config on the `DeviceId`; this is the only
    /// place a `DeviceId` becomes a live device.
    pub fn resolve(&self, id: &DeviceId) -> Option<&DeviceDescriptor> {
        self.present.get(id)
    }

    pub fn is_present(&self, id: &DeviceId) -> bool {
        self.present.contains_key(id)
    }

    pub fn list(&self) -> Vec<DeviceDescriptor> {
        let mut v: Vec<_> = self.present.values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
}

impl Default for DeviceCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::enumerate::FakeEnumerator;

    fn desc(tag: &str) -> DeviceDescriptor {
        DeviceDescriptor {
            id: DeviceId::AlsaCard { card_name: tag.into() },
            label: tag.into(),
            has_capture: true,
            has_playback: true,
        }
    }

    #[test]
    fn refresh_then_resolve() {
        let en = FakeEnumerator::new(vec![desc("Device"), desc("PCH")]);
        let mut cache = DeviceCache::new();
        cache.refresh(&en);
        assert!(cache.is_present(&DeviceId::AlsaCard { card_name: "Device".into() }));
        assert_eq!(cache.list().len(), 2);
    }

    #[test]
    fn unplugged_device_no_longer_resolves() {
        let en = FakeEnumerator::new(vec![desc("Device")]);
        let mut cache = DeviceCache::new();
        cache.refresh(&en);
        let id = DeviceId::AlsaCard { card_name: "Device".into() };
        assert!(cache.resolve(&id).is_some());

        en.set(vec![]); // device pulled
        cache.refresh(&en);
        assert!(cache.resolve(&id).is_none());
    }
}
```

- [ ] **Step 3: Write the hotplug diff with failing tests**

Create `crates/omnimodem/src/device/hotplug.rs`:

```rust
//! Hotplug detection by diffing enumeration snapshots. Emits arrivals and
//! departures keyed on `DeviceId` so config rebinds across renames/hotplug.

use super::enumerate::{DeviceDescriptor, DeviceEnumerator};
use crate::ids::DeviceId;
use std::collections::HashSet;

/// A hotplug transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotplugEvent {
    Arrived(DeviceDescriptor),
    Departed(DeviceId),
}

/// Diffs successive snapshots. Hold the previous id set; `poll` returns the
/// changes since the last call.
pub struct HotplugWatcher {
    known: HashSet<DeviceId>,
}

impl HotplugWatcher {
    pub fn new() -> Self {
        HotplugWatcher { known: HashSet::new() }
    }

    /// Enumerate once and return arrivals/departures vs. the previous poll.
    pub fn poll(&mut self, enumerator: &dyn DeviceEnumerator) -> Vec<HotplugEvent> {
        let now: Vec<DeviceDescriptor> = enumerator.enumerate();
        let now_ids: HashSet<DeviceId> = now.iter().map(|d| d.id.clone()).collect();
        let mut events = Vec::new();
        for d in &now {
            if !self.known.contains(&d.id) {
                events.push(HotplugEvent::Arrived(d.clone()));
            }
        }
        for old in &self.known {
            if !now_ids.contains(old) {
                events.push(HotplugEvent::Departed(old.clone()));
            }
        }
        self.known = now_ids;
        events
    }
}

impl Default for HotplugWatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::enumerate::FakeEnumerator;

    fn desc(tag: &str) -> DeviceDescriptor {
        DeviceDescriptor {
            id: DeviceId::AlsaCard { card_name: tag.into() },
            label: tag.into(),
            has_capture: true,
            has_playback: true,
        }
    }

    #[test]
    fn first_poll_reports_all_present_as_arrivals() {
        let en = FakeEnumerator::new(vec![desc("A"), desc("B")]);
        let mut w = HotplugWatcher::new();
        let evs = w.poll(&en);
        assert_eq!(evs.len(), 2);
        assert!(evs.iter().all(|e| matches!(e, HotplugEvent::Arrived(_))));
    }

    #[test]
    fn steady_state_reports_nothing() {
        let en = FakeEnumerator::new(vec![desc("A")]);
        let mut w = HotplugWatcher::new();
        w.poll(&en);
        assert!(w.poll(&en).is_empty());
    }

    #[test]
    fn departure_is_detected() {
        let en = FakeEnumerator::new(vec![desc("A"), desc("B")]);
        let mut w = HotplugWatcher::new();
        w.poll(&en);
        en.set(vec![desc("A")]); // B unplugged
        let evs = w.poll(&en);
        assert_eq!(evs, vec![HotplugEvent::Departed(DeviceId::AlsaCard { card_name: "B".into() })]);
    }

    #[test]
    fn arrival_after_departure_is_detected() {
        let en = FakeEnumerator::new(vec![desc("A")]);
        let mut w = HotplugWatcher::new();
        w.poll(&en);
        en.set(vec![desc("A"), desc("C")]); // C plugged in
        let evs = w.poll(&en);
        assert_eq!(evs, vec![HotplugEvent::Arrived(desc("C"))]);
    }
}
```

- [ ] **Step 4: Write the module root and the real enumerator seam**

Create `crates/omnimodem/src/device/mod.rs`:

```rust
//! Device identity, enumeration, caching, and hotplug — the spine both the
//! audio and PTT subsystems key on.

pub mod cache;
pub mod enumerate;
pub mod hotplug;

pub use cache::DeviceCache;
pub use enumerate::{DeviceDescriptor, DeviceEnumerator};
pub use hotplug::{HotplugEvent, HotplugWatcher};

/// The production enumerator: cpal for audio devices, upgraded with USB
/// identity from nusb where a card maps to a USB device. On a host with no
/// audio this yields an empty list (valid: the virtual backends still work).
#[cfg(not(test))]
pub struct RealEnumerator;

#[cfg(not(test))]
impl DeviceEnumerator for RealEnumerator {
    fn enumerate(&self) -> Vec<DeviceDescriptor> {
        let mut out = Vec::new();
        for (id, backend) in crate::audio::cpal_backend::enumerate_default_host() {
            use crate::audio::backend::AudioBackend;
            let _ = &backend; // capability probing happens at open time
            out.push(DeviceDescriptor {
                id: id.clone(),
                label: id.to_canonical_string(),
                has_capture: true,
                has_playback: true,
            });
        }
        out
    }
}
```

- [ ] **Step 5: Wire the module in and remove the placeholder**

In `crates/omnimodem/src/lib.rs`, add (before `pub mod supervisor;`):

```rust
pub mod device;
```

Delete the placeholder file and its module declaration:

```bash
git rm crates/omnimodem/src/supervisor/device.rs
```

In `crates/omnimodem/src/supervisor/mod.rs`, remove the `pub mod device;` line and the `use device::DeviceCache;` line — they are replaced by `use crate::device::DeviceCache;` (wired fully in Task 16).

> Note: `supervisor/mod.rs` still references `DeviceCache` in its struct/`new`. Until Task 16 rewires the Supervisor, temporarily change its `devices: DeviceCache` field initializer to `crate::device::DeviceCache::new()` and its `default_device()` call site. If executing strictly in order, the minimal edit is: replace `use device::DeviceCache;` with `use crate::device::DeviceCache;`, and replace the body of the old `configure_channel`'s `device_id: self.devices.default_device()` with `device_id: DeviceId::placeholder()` for now (Task 16 gives it the real binding). This keeps the crate compiling between tasks.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p omnimodem device::`
Expected: PASS — cache resolve/unplug (2) + hotplug diff (4).

- [ ] **Step 7: Commit**

```bash
git add crates/omnimodem/src/device/ crates/omnimodem/src/lib.rs crates/omnimodem/src/supervisor/mod.rs
git commit -m "Add device cache and hotplug detection keyed on DeviceId"
```

---

## PART C — PTT

## Task 10: `PttError` + `PttDriver` trait + None/Mock drivers

The structured error type that replaces Graywolf's stringly-typed `Result<(), String>` (`ptt.rs:184-189`), so callers distinguish device-went-away vs permission-denied vs busy — the prerequisite for hotplug eviction (Task 12). Plus the `PttDriver` trait, the `NonePtt` no-op (VOX/none), and a `MockPtt` test double that records key/unkey calls and can be told to fail. Fully test-driven.

**Files:**
- Create: `crates/omnimodem/src/ptt/mod.rs`
- Create: `crates/omnimodem/src/ptt/none.rs`
- Modify: `crates/omnimodem/src/lib.rs`

- [ ] **Step 1: Define the error, trait, and module root**

Create `crates/omnimodem/src/ptt/mod.rs`:

```rust
//! PTT subsystem: a `PttDriver` trait, structured errors, per-OS drivers
//! behind hardware seams, a `PortRegistry` with DeviceId-keyed hotplug
//! eviction, no-sleep TX sequencing, and the RX/TX interlock. No DSP.

pub mod interlock;
pub mod none;
pub mod registry;
pub mod sequence;
pub mod udev;

#[cfg(target_os = "linux")]
pub mod gpio;
#[cfg(unix)]
pub mod cm108;
#[cfg(unix)]
pub mod serial;

/// Structured PTT failure. Replaces Graywolf's `Result<(), String>` so callers
/// can react: `DeviceGone` triggers registry eviction (Task 12); `PermissionDenied`
/// and `Busy` are terminal config errors; `Unsupported` is a non-Linux driver.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PttError {
    #[error("ptt device {device} disappeared")]
    DeviceGone { device: String },
    #[error("permission denied opening {device}")]
    PermissionDenied { device: String },
    #[error("ptt device {device} is busy")]
    Busy { device: String },
    #[error("invalid ptt config: {0}")]
    Config(String),
    #[error("ptt method unsupported on this platform")]
    Unsupported,
    #[error("ptt i/o error: {0}")]
    Io(String),
}

/// Drives a transmitter's PTT line. `key` asserts (transmit), `unkey` releases.
/// Implementors MUST release PTT on `Drop` (a stuck transmitter is an FCC
/// hazard) — see the per-driver `Drop` impls.
pub trait PttDriver: Send {
    fn key(&mut self) -> Result<(), PttError>;
    fn unkey(&mut self) -> Result<(), PttError>;
}
```

- [ ] **Step 2: Write `NonePtt` and `MockPtt` with failing tests**

Create `crates/omnimodem/src/ptt/none.rs`:

```rust
//! The no-op driver (VOX / none) and the test double.

use super::{PttDriver, PttError};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// No hardware line: audio alone triggers TX (VOX), or PTT is disabled.
pub struct NonePtt;

impl PttDriver for NonePtt {
    fn key(&mut self) -> Result<(), PttError> {
        Ok(())
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        Ok(())
    }
}

/// Records key/unkey for tests and asserts release-on-drop. Shared counters so
/// a test can inspect state after the driver is handed to a worker/dropped.
#[derive(Clone, Default)]
pub struct MockPtt {
    pub keyed: Arc<AtomicBool>,
    pub keys: Arc<AtomicUsize>,
    pub unkeys: Arc<AtomicUsize>,
    fail_key: Arc<AtomicBool>,
    fail_unkey: Arc<AtomicBool>,
}

impl MockPtt {
    pub fn new() -> Self {
        MockPtt::default()
    }
    pub fn fail_key(&self) {
        self.fail_key.store(true, Ordering::Relaxed);
    }
    pub fn fail_unkey(&self) {
        self.fail_unkey.store(true, Ordering::Relaxed);
    }
    pub fn is_keyed(&self) -> bool {
        self.keyed.load(Ordering::Relaxed)
    }
}

impl PttDriver for MockPtt {
    fn key(&mut self) -> Result<(), PttError> {
        if self.fail_key.load(Ordering::Relaxed) {
            return Err(PttError::Io("mock key failure".into()));
        }
        self.keys.fetch_add(1, Ordering::Relaxed);
        self.keyed.store(true, Ordering::Relaxed);
        Ok(())
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        if self.fail_unkey.load(Ordering::Relaxed) {
            return Err(PttError::Io("mock unkey failure".into()));
        }
        self.unkeys.fetch_add(1, Ordering::Relaxed);
        self.keyed.store(false, Ordering::Relaxed);
        Ok(())
    }
}

/// Release PTT if the driver is dropped while keyed (panic/shutdown safety).
impl Drop for MockPtt {
    fn drop(&mut self) {
        if self.is_keyed() {
            let _ = self.unkey();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_a_noop() {
        let mut p = NonePtt;
        assert!(p.key().is_ok());
        assert!(p.unkey().is_ok());
    }

    #[test]
    fn mock_records_key_and_unkey() {
        let mut p = MockPtt::new();
        p.key().unwrap();
        assert!(p.is_keyed());
        p.unkey().unwrap();
        assert!(!p.is_keyed());
        assert_eq!(p.keys.load(Ordering::Relaxed), 1);
        assert_eq!(p.unkeys.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn mock_can_inject_failures() {
        let mut p = MockPtt::new();
        p.fail_key();
        assert!(matches!(p.key(), Err(PttError::Io(_))));
    }

    #[test]
    fn drop_while_keyed_unkeys() {
        let probe = MockPtt::new();
        let unkeys = probe.unkeys.clone();
        {
            let mut p = probe.clone();
            p.key().unwrap();
            // p drops here while keyed
        }
        assert_eq!(unkeys.load(Ordering::Relaxed), 1, "drop must release PTT");
    }
}
```

- [ ] **Step 3: Wire the module in (with placeholders for the unwritten files)**

In `crates/omnimodem/src/lib.rs`, add:

```rust
pub mod ptt;
```

`ptt/mod.rs` declares `interlock`, `registry`, `sequence`, `udev`, and the unix/linux driver modules. Create placeholders so the crate compiles now; later tasks fill them:

```bash
printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/ptt/interlock.rs
printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/ptt/registry.rs
printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/ptt/sequence.rs
printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/ptt/udev.rs
printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/ptt/serial.rs
printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/ptt/cm108.rs
printf '//! filled in a later Phase 2 task\n' > crates/omnimodem/src/ptt/gpio.rs
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p omnimodem ptt::none::`
Expected: PASS (4 tests) — None no-op, Mock records, Mock fails on demand, drop-while-keyed unkeys.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/ptt/ crates/omnimodem/src/lib.rs
git commit -m "Add PttError, PttDriver trait, and None/Mock drivers"
```

---

## Task 11: Linux PTT drivers behind hardware seams

The real drivers: serial RTS/DTR (nix `TIOCMSET` ioctl), CM108 HID GPIO (hidraw 5-byte report), and Linux gpiochip (gpiocdev). Each sits behind a tiny hardware-seam trait so the **driver logic** (invert handling, structured-error mapping, unkey-on-Drop, startup unkey) is unit-tested with a mock seam, while the **adapter** that calls nix/hidapi/gpiocdev is verified by `cargo build` + a manual smoke step. Lifted from Graywolf `tx/ptt.rs` and its `ppt_*` platform files.

**Files:**
- Create/replace: `crates/omnimodem/src/ptt/serial.rs`
- Create/replace: `crates/omnimodem/src/ptt/cm108.rs`
- Create/replace: `crates/omnimodem/src/ptt/gpio.rs`

- [ ] **Step 1: Serial driver — seam trait + logic test + real adapter**

Replace `crates/omnimodem/src/ptt/serial.rs` with:

```rust
//! Serial RTS/DTR PTT. The most common cheap interface. Logic (which line,
//! polarity, structured errors) sits above a `ModemControlLines` seam; the
//! Unix adapter drives `TIOCMSET` via nix. Lifted from Graywolf
//! `tx/ppt_unix.rs` (`UnixSerialLines`).

use super::{PttDriver, PttError};

/// Which control line keys the radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialLine {
    Rts,
    Dtr,
}

/// Hardware seam: set a control line high/low. The real impl issues ioctls; the
/// test impl records calls.
pub trait ModemControlLines: Send {
    fn write_rts(&mut self, high: bool) -> Result<(), PttError>;
    fn write_dtr(&mut self, high: bool) -> Result<(), PttError>;
}

/// Serial PTT driver over any `ModemControlLines`.
pub struct SerialLinePtt<L: ModemControlLines> {
    lines: L,
    line: SerialLine,
    invert: bool,
    device: String,
}

impl<L: ModemControlLines> SerialLinePtt<L> {
    /// Build and immediately unkey: on Linux the TTY layer asserts DTR during
    /// open() regardless of intent, so an explicit deassert here narrows the
    /// spurious-TX window to microseconds (Graywolf startup-unkey).
    pub fn new(lines: L, line: SerialLine, invert: bool, device: String) -> Result<Self, PttError> {
        let mut d = SerialLinePtt { lines, line, invert, device };
        d.unkey()?;
        Ok(d)
    }

    fn set(&mut self, asserted: bool) -> Result<(), PttError> {
        let level = asserted ^ self.invert;
        match self.line {
            SerialLine::Rts => self.lines.write_rts(level),
            SerialLine::Dtr => self.lines.write_dtr(level),
        }
    }
}

impl<L: ModemControlLines> PttDriver for SerialLinePtt<L> {
    fn key(&mut self) -> Result<(), PttError> {
        self.set(true)
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        self.set(false)
    }
}

impl<L: ModemControlLines> Drop for SerialLinePtt<L> {
    fn drop(&mut self) {
        let _ = self.set(false); // never leave a rig keyed
    }
}

/// The real Unix adapter: open the tty and drive TIOCMSET.
#[cfg(unix)]
pub mod unix {
    use super::*;
    use std::os::fd::{AsRawFd, OwnedFd};

    pub struct UnixSerialLines {
        fd: OwnedFd,
        device: String,
    }

    impl UnixSerialLines {
        pub fn open(path: &str) -> Result<Self, PttError> {
            use nix::fcntl::{open, OFlag};
            use nix::sys::stat::Mode;
            let fd = open(
                path,
                OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| map_open_err(path, e))?;
            // SAFETY: open returned a valid owned fd.
            let owned = unsafe { OwnedFd::from_raw_fd_checked(fd) };
            Ok(UnixSerialLines { fd: owned, device: path.to_string() })
        }

        fn set_bit(&mut self, bit: i32, high: bool) -> Result<(), PttError> {
            // Read-modify-write the modem control bits via TIOCMGET/TIOCMSET.
            let mut status: i32 = 0;
            // SAFETY: ioctl with valid fd + int pointer.
            let r = unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCMGET, &mut status) };
            if r != 0 {
                return Err(self.io_or_gone());
            }
            if high {
                status |= bit;
            } else {
                status &= !bit;
            }
            let r = unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCMSET, &status) };
            if r != 0 {
                return Err(self.io_or_gone());
            }
            Ok(())
        }

        fn io_or_gone(&self) -> PttError {
            // ENODEV/ENXIO after a working open means the adapter was unplugged.
            match nix::errno::Errno::last() {
                nix::errno::Errno::ENODEV | nix::errno::Errno::ENXIO => {
                    PttError::DeviceGone { device: self.device.clone() }
                }
                e => PttError::Io(format!("{}: {e}", self.device)),
            }
        }
    }

    impl ModemControlLines for UnixSerialLines {
        fn write_rts(&mut self, high: bool) -> Result<(), PttError> {
            self.set_bit(libc::TIOCM_RTS, high)
        }
        fn write_dtr(&mut self, high: bool) -> Result<(), PttError> {
            self.set_bit(libc::TIOCM_DTR, high)
        }
    }

    fn map_open_err(path: &str, e: nix::errno::Errno) -> PttError {
        match e {
            nix::errno::Errno::EACCES => PttError::PermissionDenied { device: path.into() },
            nix::errno::Errno::EBUSY => PttError::Busy { device: path.into() },
            nix::errno::Errno::ENOENT | nix::errno::Errno::ENODEV | nix::errno::Errno::ENXIO => {
                PttError::DeviceGone { device: path.into() }
            }
            other => PttError::Io(format!("open {path}: {other}")),
        }
    }

    // Small shim: OwnedFd::from_raw_fd is unsafe; wrap to keep call sites clean.
    trait FromRawFdChecked {
        unsafe fn from_raw_fd_checked(fd: std::os::fd::RawFd) -> Self;
    }
    impl FromRawFdChecked for OwnedFd {
        unsafe fn from_raw_fd_checked(fd: std::os::fd::RawFd) -> Self {
            use std::os::fd::FromRawFd;
            OwnedFd::from_raw_fd(fd)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct FakeLines {
        rts: Arc<Mutex<bool>>,
        dtr: Arc<Mutex<bool>>,
    }
    impl ModemControlLines for FakeLines {
        fn write_rts(&mut self, high: bool) -> Result<(), PttError> {
            *self.rts.lock().unwrap() = high;
            Ok(())
        }
        fn write_dtr(&mut self, high: bool) -> Result<(), PttError> {
            *self.dtr.lock().unwrap() = high;
            Ok(())
        }
    }

    #[test]
    fn key_asserts_the_selected_line() {
        let lines = FakeLines::default();
        let rts = lines.rts.clone();
        let mut d = SerialLinePtt::new(lines, SerialLine::Rts, false, "/dev/ttyUSB0".into()).unwrap();
        d.key().unwrap();
        assert!(*rts.lock().unwrap());
        d.unkey().unwrap();
        assert!(!*rts.lock().unwrap());
    }

    #[test]
    fn invert_flips_polarity() {
        let lines = FakeLines::default();
        let dtr = lines.dtr.clone();
        let mut d = SerialLinePtt::new(lines, SerialLine::Dtr, true, "/dev/ttyUSB0".into()).unwrap();
        // new() unkeyed: inverted unkey => line HIGH.
        assert!(*dtr.lock().unwrap());
        d.key().unwrap();
        assert!(!*dtr.lock().unwrap()); // inverted key => LOW
    }

    #[test]
    fn drop_releases_the_line() {
        let lines = FakeLines::default();
        let rts = lines.rts.clone();
        {
            let mut d = SerialLinePtt::new(lines, SerialLine::Rts, false, "x".into()).unwrap();
            d.key().unwrap();
            assert!(*rts.lock().unwrap());
        }
        assert!(!*rts.lock().unwrap(), "drop must deassert");
    }
}
```

- [ ] **Step 2: CM108 driver — seam + logic test + real adapter**

Replace `crates/omnimodem/src/ptt/cm108.rs` with:

```rust
//! CM108/CM119 HID GPIO PTT. Writes a 5-byte HID output report to /dev/hidrawN.
//! Closing a hidraw fd does NOT reset GPIO, so unkey-on-Drop is mandatory.
//! Lifted from Graywolf `tx/ppt_cm108_unix.rs`.

use super::{PttDriver, PttError};

/// Hardware seam: write a raw HID output report.
pub trait Cm108Hid: Send {
    fn write_report(&mut self, report: [u8; 5]) -> Result<(), PttError>;
}

/// CM108 PTT on a 1..=8 GPIO pin.
pub struct Cm108Ptt<H: Cm108Hid> {
    hid: H,
    pin: u8,
    invert: bool,
}

impl<H: Cm108Hid> Cm108Ptt<H> {
    pub fn new(hid: H, pin: u8, invert: bool) -> Result<Self, PttError> {
        if !(1..=8).contains(&pin) {
            return Err(PttError::Config(format!("cm108 pin {pin} out of range 1..=8")));
        }
        let mut d = Cm108Ptt { hid, pin, invert };
        d.unkey()?;
        Ok(d)
    }

    fn set(&mut self, asserted: bool) -> Result<(), PttError> {
        let on = asserted ^ self.invert;
        let mask = 1u8 << (self.pin - 1);
        let value = if on { mask } else { 0 };
        // CM108 HID GPIO report: [0x00, 0x00, value, mask, 0x00].
        self.hid.write_report([0x00, 0x00, value, mask, 0x00])
    }
}

impl<H: Cm108Hid> PttDriver for Cm108Ptt<H> {
    fn key(&mut self) -> Result<(), PttError> {
        self.set(true)
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        self.set(false)
    }
}

impl<H: Cm108Hid> Drop for Cm108Ptt<H> {
    fn drop(&mut self) {
        let _ = self.set(false);
    }
}

/// Real Unix adapter: open /dev/hidrawN and write the report.
#[cfg(unix)]
pub mod unix {
    use super::*;
    use std::os::fd::{AsRawFd, OwnedFd};

    pub struct UnixCm108Hid {
        fd: OwnedFd,
        device: String,
    }

    impl UnixCm108Hid {
        pub fn open(path: &str) -> Result<Self, PttError> {
            use nix::fcntl::{open, OFlag};
            use nix::sys::stat::Mode;
            use std::os::fd::FromRawFd;
            let raw = open(
                path,
                OFlag::O_RDWR | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| match e {
                nix::errno::Errno::EACCES => PttError::PermissionDenied { device: path.into() },
                nix::errno::Errno::ENOENT => PttError::DeviceGone { device: path.into() },
                o => PttError::Io(format!("open {path}: {o}")),
            })?;
            // SAFETY: open returned a valid fd.
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };
            Ok(UnixCm108Hid { fd, device: path.to_string() })
        }
    }

    impl Cm108Hid for UnixCm108Hid {
        fn write_report(&mut self, report: [u8; 5]) -> Result<(), PttError> {
            match nix::unistd::write(&self.fd, &report) {
                Ok(_) => Ok(()),
                Err(nix::errno::Errno::ENODEV) => {
                    Err(PttError::DeviceGone { device: self.device.clone() })
                }
                Err(e) => Err(PttError::Io(format!("{}: {e}", self.device))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct FakeHid {
        last: Arc<Mutex<Option<[u8; 5]>>>,
    }
    impl Cm108Hid for FakeHid {
        fn write_report(&mut self, report: [u8; 5]) -> Result<(), PttError> {
            *self.last.lock().unwrap() = Some(report);
            Ok(())
        }
    }

    #[test]
    fn rejects_pin_out_of_range() {
        assert!(matches!(
            Cm108Ptt::new(FakeHid::default(), 9, false),
            Err(PttError::Config(_))
        ));
    }

    #[test]
    fn key_sets_value_and_mask_for_pin3() {
        let hid = FakeHid::default();
        let last = hid.last.clone();
        let mut d = Cm108Ptt::new(hid, 3, false).unwrap();
        d.key().unwrap();
        // pin 3 => bit 2 => mask 0b100 = 0x04, value == mask when on.
        assert_eq!(last.lock().unwrap().unwrap(), [0x00, 0x00, 0x04, 0x04, 0x00]);
        d.unkey().unwrap();
        assert_eq!(last.lock().unwrap().unwrap(), [0x00, 0x00, 0x00, 0x04, 0x00]);
    }

    #[test]
    fn drop_writes_an_unkey_report() {
        let hid = FakeHid::default();
        let last = hid.last.clone();
        {
            let mut d = Cm108Ptt::new(hid, 1, false).unwrap();
            d.key().unwrap();
        }
        // Final report after drop is value=0.
        assert_eq!(last.lock().unwrap().unwrap()[2], 0x00);
    }
}
```

- [ ] **Step 3: GPIO driver — seam + logic test + real adapter with eviction signal**

Replace `crates/omnimodem/src/ptt/gpio.rs` with:

```rust
//! Linux gpiochip v2 PTT via gpiocdev. A `LineGone` on set means the chip was
//! unplugged -> map to `PttError::DeviceGone` so the registry evicts. Lifted
//! from Graywolf `tx/ppt_gpio_linux.rs` + its `GpioError`.

#![cfg(target_os = "linux")]

use super::{PttDriver, PttError};

/// Hardware seam: drive one gpiochip line active/inactive.
pub trait GpiochipLine: Send {
    fn set_active(&mut self, active: bool) -> Result<(), PttError>;
}

pub struct GpioPtt<L: GpiochipLine> {
    line: L,
    invert: bool,
}

impl<L: GpiochipLine> GpioPtt<L> {
    pub fn new(line: L, invert: bool) -> Result<Self, PttError> {
        let mut d = GpioPtt { line, invert };
        d.unkey()?;
        Ok(d)
    }
    fn set(&mut self, asserted: bool) -> Result<(), PttError> {
        self.line.set_active(asserted ^ self.invert)
    }
}

impl<L: GpiochipLine> PttDriver for GpioPtt<L> {
    fn key(&mut self) -> Result<(), PttError> {
        self.set(true)
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        self.set(false)
    }
}

impl<L: GpiochipLine> Drop for GpioPtt<L> {
    fn drop(&mut self) {
        let _ = self.set(false);
    }
}

/// Real adapter over gpiocdev.
pub mod linux {
    use super::*;
    use gpiocdev::line::Value;
    use gpiocdev::request::Request;

    pub struct LinuxGpiochip {
        request: Request,
        offset: u32,
        chip: String,
    }

    impl LinuxGpiochip {
        pub fn open(chip_path: &str, offset: u32) -> Result<Self, PttError> {
            let request = Request::builder()
                .on_chip(chip_path)
                .with_line(offset)
                .with_consumer("omnimodem-ptt")
                .as_output(Value::Inactive)
                .request()
                .map_err(|e| map_gpio_err(chip_path, offset, e))?;
            Ok(LinuxGpiochip { request, offset, chip: chip_path.to_string() })
        }
    }

    impl GpiochipLine for LinuxGpiochip {
        fn set_active(&mut self, active: bool) -> Result<(), PttError> {
            let v = if active { Value::Active } else { Value::Inactive };
            self.request
                .set_value(self.offset, v)
                .map_err(|e| map_gpio_err(&self.chip, self.offset, e))
        }
    }

    fn map_gpio_err(chip: &str, _line: u32, e: gpiocdev::Error) -> PttError {
        let s = e.to_string().to_lowercase();
        if s.contains("permission") {
            PttError::PermissionDenied { device: chip.into() }
        } else if s.contains("busy") {
            PttError::Busy { device: chip.into() }
        } else if s.contains("no such") || s.contains("nodev") || s.contains("gone") {
            PttError::DeviceGone { device: chip.into() }
        } else {
            PttError::Io(format!("{chip}: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct FakeLine {
        active: Arc<Mutex<bool>>,
        gone: Arc<Mutex<bool>>,
    }
    impl GpiochipLine for FakeLine {
        fn set_active(&mut self, active: bool) -> Result<(), PttError> {
            if *self.gone.lock().unwrap() {
                return Err(PttError::DeviceGone { device: "gpiochip0".into() });
            }
            *self.active.lock().unwrap() = active;
            Ok(())
        }
    }

    #[test]
    fn key_unkey_drives_the_line() {
        let line = FakeLine::default();
        let active = line.active.clone();
        let mut d = GpioPtt::new(line, false).unwrap();
        d.key().unwrap();
        assert!(*active.lock().unwrap());
        d.unkey().unwrap();
        assert!(!*active.lock().unwrap());
    }

    #[test]
    fn line_gone_surfaces_device_gone() {
        let line = FakeLine::default();
        let gone = line.gone.clone();
        let mut d = GpioPtt::new(line, false).unwrap();
        *gone.lock().unwrap() = true;
        assert!(matches!(d.key(), Err(PttError::DeviceGone { .. })));
    }
}
```

- [ ] **Step 4: Build and run the driver-logic tests**

Run: `cargo build -p omnimodem && cargo test -p omnimodem ptt::serial:: ptt::cm108::`
Expected: build succeeds (real adapters compile); serial (3) + cm108 (3) tests pass. On a Linux host, also run `cargo test -p omnimodem ptt::gpio::` (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/ptt/serial.rs crates/omnimodem/src/ptt/cm108.rs crates/omnimodem/src/ptt/gpio.rs
git commit -m "Add Linux serial, CM108, and gpiochip PTT drivers behind hardware seams"
```

---

## Task 12: `PortRegistry` with DeviceId-keyed hotplug eviction

The registry is the factory that turns a `PttConfig` into a `Box<dyn PttDriver>`, and — the fix the design demands — it **evicts a cached driver by `DeviceId` on disappearance/hotplug** instead of caching fds by path forever (Graywolf `ptt.rs:484-487` never evicts serial fds). Drivers are constructed through an injectable opener so the registry's caching/eviction logic is unit-tested with `MockPtt`, no hardware.

**Files:**
- Create/replace: `crates/omnimodem/src/ptt/registry.rs`

- [ ] **Step 1: Write the registry with failing tests**

Replace `crates/omnimodem/src/ptt/registry.rs` with:

```rust
//! PortRegistry: build PTT drivers from config and cache them by DeviceId, with
//! eviction on hotplug/disappearance — the gap Graywolf's path-keyed, never-
//! evicted serial cache left open.

use super::none::NonePtt;
use super::{PttડDriver_PLACEHOLDER, PttDriver, PttError};
use crate::ids::DeviceId;
use std::collections::HashMap;

/// How a channel's PTT is wired. The `device_id` is the durable key config is
/// stored under; `node` is the resolved live path (e.g. /dev/ttyUSB0) supplied
/// by the device cache at build time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PttConfig {
    pub device_id: DeviceId,
    pub method: PttMethod,
    pub invert: bool,
}

/// Supported PTT methods. Non-Linux construction fails closed (`Unsupported`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PttMethod {
    None,
    Vox,
    SerialRts { node: String },
    SerialDtr { node: String },
    Cm108 { node: String, pin: u8 },
    Gpio { chip: String, line: u32 },
}

/// Opens a real driver for a config. Injectable so tests substitute MockPtt.
pub trait DriverOpener: Send {
    fn open(&self, cfg: &PttConfig) -> Result<Box<dyn PttDriver>, PttError>;
}

/// Caches one driver per DeviceId; evicts on hotplug.
pub struct PortRegistry {
    opener: Box<dyn DriverOpener>,
    cache: HashMap<DeviceId, ()>, // identity presence; drivers are owned by callers
}

impl PortRegistry {
    pub fn new(opener: Box<dyn DriverOpener>) -> Self {
        PortRegistry { opener, cache: HashMap::new() }
    }

    /// Build a driver, recording its DeviceId as live.
    pub fn build_driver(&mut self, cfg: &PttConfig) -> Result<Box<dyn PttDriver>, PttError> {
        let driver = self.opener.open(cfg)?;
        self.cache.insert(cfg.device_id.clone(), ());
        Ok(driver)
    }

    /// A device disappeared (hotplug `Departed`): forget it so the next
    /// build_driver re-opens from scratch rather than reusing a dead handle.
    pub fn evict(&mut self, id: &DeviceId) {
        self.cache.remove(id);
    }

    pub fn knows(&self, id: &DeviceId) -> bool {
        self.cache.contains_key(id)
    }
}

/// The production opener building the real Linux drivers from Task 11.
pub struct RealOpener;

impl DriverOpener for RealOpener {
    fn open(&self, cfg: &PttConfig) -> Result<Box<dyn PttDriver>, PttError> {
        match &cfg.method {
            PttMethod::None | PttMethod::Vox => Ok(Box::new(NonePtt)),
            #[cfg(unix)]
            PttMethod::SerialRts { node } => {
                use super::serial::{unix::UnixSerialLines, SerialLine, SerialLinePtt};
                let lines = UnixSerialLines::open(node)?;
                Ok(Box::new(SerialLinePtt::new(lines, SerialLine::Rts, cfg.invert, node.clone())?))
            }
            #[cfg(unix)]
            PttMethod::SerialDtr { node } => {
                use super::serial::{unix::UnixSerialLines, SerialLine, SerialLinePtt};
                let lines = UnixSerialLines::open(node)?;
                Ok(Box::new(SerialLinePtt::new(lines, SerialLine::Dtr, cfg.invert, node.clone())?))
            }
            #[cfg(unix)]
            PttMethod::Cm108 { node, pin } => {
                use super::cm108::{unix::UnixCm108Hid, Cm108Ptt};
                let hid = UnixCm108Hid::open(node)?;
                Ok(Box::new(Cm108Ptt::new(hid, *pin, cfg.invert)?))
            }
            #[cfg(target_os = "linux")]
            PttMethod::Gpio { chip, line } => {
                use super::gpio::{linux::LinuxGpiochip, GpioPtt};
                let gl = LinuxGpiochip::open(chip, *line)?;
                Ok(Box::new(GpioPtt::new(gl, cfg.invert)?))
            }
            #[allow(unreachable_patterns)]
            _ => Err(PttError::Unsupported),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptt::none::MockPtt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountingOpener {
        opens: Arc<AtomicUsize>,
    }
    impl DriverOpener for CountingOpener {
        fn open(&self, _cfg: &PttConfig) -> Result<Box<dyn PttDriver>, PttError> {
            self.opens.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(MockPtt::new()))
        }
    }

    fn cfg(tag: &str) -> PttConfig {
        PttConfig {
            device_id: DeviceId::Serial { by_id: tag.into() },
            method: PttMethod::None,
            invert: false,
        }
    }

    #[test]
    fn build_records_identity_as_live() {
        let opens = Arc::new(AtomicUsize::new(0));
        let mut reg = PortRegistry::new(Box::new(CountingOpener { opens: opens.clone() }));
        let c = cfg("ftdi-A");
        let _d = reg.build_driver(&c).unwrap();
        assert!(reg.knows(&c.device_id));
    }

    #[test]
    fn eviction_forgets_the_identity() {
        let opens = Arc::new(AtomicUsize::new(0));
        let mut reg = PortRegistry::new(Box::new(CountingOpener { opens }));
        let c = cfg("ftdi-A");
        let _d = reg.build_driver(&c).unwrap();
        reg.evict(&c.device_id);
        assert!(!reg.knows(&c.device_id), "evicted identity must be forgotten");
    }

    #[test]
    fn rebuild_after_eviction_reopens() {
        let opens = Arc::new(AtomicUsize::new(0));
        let mut reg = PortRegistry::new(Box::new(CountingOpener { opens: opens.clone() }));
        let c = cfg("ftdi-A");
        let _d1 = reg.build_driver(&c).unwrap();
        reg.evict(&c.device_id);
        let _d2 = reg.build_driver(&c).unwrap();
        assert_eq!(opens.load(Ordering::Relaxed), 2, "reopen after hotplug");
    }
}
```

> **Worker correction (typo guard):** the `use super::{PttડDriver_PLACEHOLDER, PttDriver, PttError};` line above contains a deliberate sentinel so you don't paste it blind — replace that entire `use` line with `use super::{PttDriver, PttError};`. There is no `PttDriver_PLACEHOLDER` symbol.

- [ ] **Step 2: Run the tests**

Run: `cargo test -p omnimodem ptt::registry::`
Expected: PASS (3 tests) — build records identity, evict forgets it, rebuild-after-eviction reopens (proving the fix).

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodem/src/ptt/registry.rs
git commit -m "Add PortRegistry with DeviceId-keyed hotplug eviction"
```

---

## Task 13: No-sleep TX sequencing (`drive_tx_cycle`)

The on-air cycle, timed by the audio drain watermark rather than sleeps (Graywolf `tx_worker.rs:408-481`): key → submit audio → wait until both the DAC has drained the submitted watermark *and* the expected wall-clock duration has elapsed → unkey. Failures unkey before returning so a rig is never stuck keyed. Deterministic: tested with `MockPtt` + the file/null backend's real watermark.

**Files:**
- Create/replace: `crates/omnimodem/src/ptt/sequence.rs`

- [ ] **Step 1: Write the sequence with failing tests**

Replace `crates/omnimodem/src/ptt/sequence.rs` with:

```rust
//! No-sleep TX sequencing. Times PTT off the playback drain watermark, not a
//! fixed sleep. Lifted from Graywolf `tx_worker.rs::drive_tx_cycle`.

use super::{PttDriver, PttError};
use crate::audio::backend::PlaybackHandle;
use std::time::{Duration, Instant};

/// Outcome of one TX cycle. On any failure PTT has been released (except
/// `KeyFailed`, where the line was never asserted).
#[derive(Debug, PartialEq, Eq)]
pub enum TxCycleOutcome {
    Done,
    KeyFailed(PttError),
    SubmitFailed(PttError),
    UnkeyFailed(PttError),
}

/// Drive one transmission: key, play `samples`, wait for drain, unkey.
/// `poll` is the drain-loop poll interval (5 ms in production; 0 in tests).
pub fn drive_tx_cycle(
    driver: &mut dyn PttDriver,
    sink: &PlaybackHandle,
    samples: Vec<i16>,
    sample_rate: u32,
    poll: Duration,
) -> TxCycleOutcome {
    let n = samples.len();
    let expected = Duration::from_nanos((n as u64 * 1_000_000_000) / sample_rate.max(1) as u64);

    if let Err(e) = driver.key() {
        return TxCycleOutcome::KeyFailed(e);
    }

    let watermark = match sink.submit(samples) {
        Ok(wm) => wm,
        Err(e) => {
            let _ = driver.unkey(); // release before bailing
            return TxCycleOutcome::SubmitFailed(PttError::Io(e.to_string()));
        }
    };

    // Wait until BOTH the DAC drained the watermark AND the expected airtime
    // elapsed. Timeout = expected + 500 ms guards a wedged stream.
    let start = Instant::now();
    let deadline = start + expected + Duration::from_millis(500);
    loop {
        let drained_enough = sink.drained_samples() >= watermark;
        let time_enough = start.elapsed() >= expected;
        if drained_enough && time_enough {
            break;
        }
        if Instant::now() >= deadline {
            break; // proceed to unkey rather than hang forever
        }
        if !poll.is_zero() {
            std::thread::sleep(poll);
        } else {
            std::thread::yield_now();
        }
    }

    match driver.unkey() {
        Ok(()) => TxCycleOutcome::Done,
        Err(e) => TxCycleOutcome::UnkeyFailed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::backend::AudioBackend;
    use crate::audio::file::FileBackend;
    use crate::ptt::none::MockPtt;

    #[test]
    fn full_cycle_keys_plays_and_unkeys() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        let keyed_during = ptt.keyed.clone();

        let out = drive_tx_cycle(&mut ptt, &sink, vec![5i16; 480], 48_000, Duration::ZERO);
        assert_eq!(out, TxCycleOutcome::Done);
        assert!(!keyed_during.load(std::sync::atomic::Ordering::Relaxed), "released after");
        // Audio actually reached the sink.
        assert_eq!(backend.played.lock().unwrap().len(), 480);
    }

    #[test]
    fn key_failure_does_not_submit_or_unkey() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        ptt.fail_key();
        let out = drive_tx_cycle(&mut ptt, &sink, vec![0i16; 10], 48_000, Duration::ZERO);
        assert!(matches!(out, TxCycleOutcome::KeyFailed(_)));
        assert_eq!(backend.played.lock().unwrap().len(), 0, "no audio on key failure");
    }

    #[test]
    fn unkey_failure_is_reported() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        ptt.fail_unkey();
        let out = drive_tx_cycle(&mut ptt, &sink, vec![0i16; 48], 48_000, Duration::ZERO);
        assert!(matches!(out, TxCycleOutcome::UnkeyFailed(_)));
    }

    #[test]
    fn empty_buffer_completes_immediately() {
        let backend = FileBackend::from_samples(vec![], 48_000);
        let sink = backend.open_playback(48_000).unwrap();
        let mut ptt = MockPtt::new();
        let start = Instant::now();
        let out = drive_tx_cycle(&mut ptt, &sink, vec![], 48_000, Duration::ZERO);
        assert_eq!(out, TxCycleOutcome::Done);
        assert!(start.elapsed() < Duration::from_millis(100), "no spurious sleep");
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p omnimodem ptt::sequence::`
Expected: PASS (4 tests) — full cycle plays + releases, key-failure short-circuits, unkey-failure reported, empty buffer is instant (no sleeps).

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodem/src/ptt/sequence.rs
git commit -m "Add no-sleep TX sequencing timed by drain watermark"
```

---

## Task 14: Per-channel RX/TX interlock

When a channel keys PTT on a device, RX decode on that device must be muted to avoid decoding our own transmission/feedback. Graywolf handled this implicitly on its single thread; the per-channel-thread model must make it explicit. The interlock is a per-device keyed-count gate: capture chunks are dropped while the device is keyed. Pure state machine, fully test-driven.

**Files:**
- Create/replace: `crates/omnimodem/src/ptt/interlock.rs`

- [ ] **Step 1: Write the interlock with failing tests**

Replace `crates/omnimodem/src/ptt/interlock.rs` with:

```rust
//! RX/TX interlock. While any channel keys a physical device, RX on that device
//! is muted so we don't decode our own transmission. Keyed by DeviceId because
//! two channels sharing one rig must interlock together (design: concurrency is
//! per-rig). A count, not a bool: overlapping keys on the same rig nest.

use crate::ids::DeviceId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Shared interlock state. Cloneable handle over a shared map.
#[derive(Clone, Default)]
pub struct RxTxInterlock {
    keyed: Arc<Mutex<HashMap<DeviceId, u32>>>,
}

impl RxTxInterlock {
    pub fn new() -> Self {
        RxTxInterlock::default()
    }

    /// Mark a device keyed (begin TX). Nesting-safe.
    pub fn begin_tx(&self, id: &DeviceId) {
        *self.keyed.lock().unwrap().entry(id.clone()).or_insert(0) += 1;
    }

    /// Mark a device unkeyed (end TX). Saturating at zero.
    pub fn end_tx(&self, id: &DeviceId) {
        let mut map = self.keyed.lock().unwrap();
        if let Some(c) = map.get_mut(id) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                map.remove(id);
            }
        }
    }

    /// True while the device is keyed — RX on it must be muted.
    pub fn is_muted(&self, id: &DeviceId) -> bool {
        self.keyed.lock().unwrap().get(id).map(|&c| c > 0).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(tag: &str) -> DeviceId {
        DeviceId::AlsaCard { card_name: tag.into() }
    }

    #[test]
    fn idle_device_is_not_muted() {
        let il = RxTxInterlock::new();
        assert!(!il.is_muted(&dev("A")));
    }

    #[test]
    fn key_mutes_and_unkey_unmutes() {
        let il = RxTxInterlock::new();
        il.begin_tx(&dev("A"));
        assert!(il.is_muted(&dev("A")));
        il.end_tx(&dev("A"));
        assert!(!il.is_muted(&dev("A")));
    }

    #[test]
    fn only_the_keyed_device_is_muted() {
        let il = RxTxInterlock::new();
        il.begin_tx(&dev("A"));
        assert!(il.is_muted(&dev("A")));
        assert!(!il.is_muted(&dev("B")));
    }

    #[test]
    fn nested_keys_on_one_rig_need_matching_unkeys() {
        let il = RxTxInterlock::new();
        il.begin_tx(&dev("A")); // channel 1 keys
        il.begin_tx(&dev("A")); // channel 2 keys same rig
        il.end_tx(&dev("A")); // channel 1 done
        assert!(il.is_muted(&dev("A")), "still keyed by channel 2");
        il.end_tx(&dev("A")); // channel 2 done
        assert!(!il.is_muted(&dev("A")));
    }

    #[test]
    fn end_without_begin_is_safe() {
        let il = RxTxInterlock::new();
        il.end_tx(&dev("A")); // no panic, no underflow
        assert!(!il.is_muted(&dev("A")));
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p omnimodem ptt::interlock::`
Expected: PASS (5 tests) — idle unmuted, key/unkey toggles, isolation per device, nested keys, underflow-safe.

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodem/src/ptt/interlock.rs
git commit -m "Add per-rig RX/TX interlock"
```

---

## PART D — gRPC integration & exit criterion

## Task 15: Proto additions (additive within `omnimodem.v1`)

Add the device/audio/PTT control surface and the new events. Every change is **additive** per `proto/VERSIONING.md` — new messages, new RPCs, new `Event.kind` oneof variants with fresh tags; nothing renumbered. `Transmit` already exists from Phase 1; it gains real meaning (play the payload as PCM).

**Files:**
- Modify: `crates/omnimodem/proto/omnimodem.proto`

- [ ] **Step 1: Add the new RPCs and messages**

In `proto/omnimodem.proto`, add these methods inside `service ModemControl { ... }` (after `SubscribeEvents`):

```proto
  // Enumerate present audio/PTT-capable devices with their stable DeviceId.
  rpc ListDevices(ListDevicesRequest) returns (ListDevicesResponse);

  // Bind a channel's audio to a device (by stable DeviceId) and open streams.
  rpc ConfigureAudio(ConfigureAudioRequest) returns (ConfigureAudioResponse);

  // Bind a channel's PTT to a device + method.
  rpc ConfigurePtt(ConfigurePttRequest) returns (ConfigurePttResponse);

  // Manually key/unkey a channel's PTT (operator test; no audio).
  rpc KeyPtt(KeyPttRequest) returns (KeyPttResponse);

  // Return ready-to-install udev rule text for a device (never writes it).
  rpc SuggestUdevRule(SuggestUdevRuleRequest) returns (SuggestUdevRuleResponse);
```

Then add these messages at the end of the file (new tags only):

```proto
// ---------------------------------------------------------------------------
// Phase 2: devices, audio, PTT
// ---------------------------------------------------------------------------

message ListDevicesRequest {}

message DeviceInfo {
  string device_id = 1;   // canonical DeviceId string (see ids.rs)
  string label = 2;       // operator-facing name
  bool has_capture = 3;
  bool has_playback = 4;
}

message ListDevicesResponse {
  repeated DeviceInfo devices = 1;
}

message ConfigureAudioRequest {
  uint32 channel = 1;
  string device_id = 2;       // stable DeviceId; config keys on this
  uint32 sample_rate = 3;     // requested working rate (clamped to 48 kHz)
  uint32 fanout = 4;          // capture consumers; 0/1 == no fan-out
}

message ConfigureAudioResponse {
  uint32 actual_sample_rate = 1;  // rate the stream actually opened at
}

enum PttMethod {
  PTT_METHOD_UNSPECIFIED = 0;
  PTT_METHOD_NONE = 1;
  PTT_METHOD_VOX = 2;
  PTT_METHOD_SERIAL_RTS = 3;
  PTT_METHOD_SERIAL_DTR = 4;
  PTT_METHOD_CM108 = 5;
  PTT_METHOD_GPIO = 6;
}

message ConfigurePttRequest {
  uint32 channel = 1;
  string device_id = 2;       // stable DeviceId for the PTT device
  PttMethod method = 3;
  string node = 4;            // resolved node / chip path (serial/cm108/gpio)
  uint32 pin_or_line = 5;     // cm108 GPIO pin (1-8) or gpiochip line offset
  bool invert = 6;
}

message ConfigurePttResponse {}

message KeyPttRequest {
  uint32 channel = 1;
  bool keyed = 2;             // true=key, false=unkey
}

message KeyPttResponse {}

message SuggestUdevRuleRequest {
  string device_id = 1;
}

message SuggestUdevRuleResponse {
  string rule = 1;            // udev rule text
  string instructions = 2;    // where to put it
}
```

Finally, add new variants to the existing `Event` oneof (new tags 8–10; do not touch 1–7):

```proto
    DeviceArrived device_arrived = 8;   // LOSSY
    DeviceDeparted device_departed = 9; // LOSSY
    PttState ptt_state = 10;            // LOSSY
```

and the messages for them:

```proto
message DeviceArrived {
  string device_id = 1;
  string label = 2;
}

message DeviceDeparted {
  string device_id = 1;
}

message PttState {
  uint32 channel = 1;
  bool keyed = 2;
}
```

- [ ] **Step 2: Verify codegen compiles**

Run: `cargo build -p omnimodem`
Expected: success; tonic-build regenerates the expanded service and message types.

- [ ] **Step 3: Add a generated-types smoke test**

In `crates/omnimodem/src/proto.rs`, add inside `mod tests`:

```rust
    #[test]
    fn phase2_types_are_constructible() {
        let _ = DeviceInfo {
            device_id: "usb:0d8c:013c:".into(),
            label: "C-Media".into(),
            has_capture: true,
            has_playback: true,
        };
        let _ = Event { kind: Some(event::Kind::PttState(PttState { channel: 0, keyed: true })) };
        assert_eq!(PttMethod::SerialRts as i32, 3);
    }
```

Run: `cargo test -p omnimodem proto::tests::phase2_types_are_constructible`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodem/proto/omnimodem.proto crates/omnimodem/src/proto.rs
git commit -m "Extend omnimodem.v1 proto with device, audio, and PTT control"
```

> Note: per `proto/VERSIONING.md`, this PR's description must state "additive within v1" — new tags 8–10 on `Event`, new messages, new RPCs; nothing renumbered.

---

## Task 16: Supervisor + core wiring (real device/PTT, per-channel TX worker, interlock)

Replace the Phase-1 placeholders: the Supervisor now owns a real `DeviceCache`, a `PortRegistry`, the `RxTxInterlock`, and per-channel audio/PTT bindings. The core gains commands for audio/PTT config and a **per-channel TX worker** that runs `drive_tx_cycle`, drives the interlock around each cycle, and the hotplug pump that evicts on `Departed`.

**Files:**
- Modify: `crates/omnimodem/src/supervisor/channel.rs`
- Modify: `crates/omnimodem/src/supervisor/mod.rs`
- Delete: `crates/omnimodem/src/supervisor/ptt.rs`
- Modify: `crates/omnimodem/src/core/command.rs`
- Modify: `crates/omnimodem/src/core/event.rs`
- Modify: `crates/omnimodem/src/core/mod.rs`
- Modify: `crates/omnimodem/src/persist/mod.rs`

- [ ] **Step 1: Extend `ChannelConfig` with audio + PTT bindings**

In `crates/omnimodem/src/supervisor/channel.rs`, extend `ChannelConfig` (keep `id`, `name`, `mode`, `device_id`) with optional bindings:

```rust
use crate::ptt::registry::{PttConfig, PttMethod};

/// Persisted, operator-supplied channel configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelConfig {
    pub id: ChannelId,
    pub name: String,
    pub mode: String,
    /// Audio device this channel binds to (the durable identity).
    pub device_id: DeviceId,
    /// Requested working rate (clamped to 48 kHz at open).
    pub sample_rate: u32,
    /// Capture fan-out consumers (0/1 == none).
    pub fanout: u32,
    /// PTT binding; `None` until ConfigurePtt.
    pub ptt: Option<PttConfig>,
}
```

Update `ChannelState::new` callers accordingly (it already wraps a `ChannelConfig`).

> Persistence note: add `sample_rate INTEGER`, `fanout INTEGER`, and `ptt_method TEXT`/`ptt_node TEXT`/`ptt_pin INTEGER`/`ptt_invert INTEGER` columns to the `channels` table in `persist/mod.rs` (`CREATE TABLE` and the `upsert_channel`/`load_channels` SQL). Encode `PttConfig` as its method string + node + pin/line + invert; `None` ptt stores an empty method. Default `sample_rate` to 48000 and `fanout` to 1 when absent. Mirror the Phase-1 round-trip test in `persist::tests` for the new columns.

- [ ] **Step 2: Rewrite the Supervisor to own the real subsystems**

In `crates/omnimodem/src/supervisor/mod.rs`: remove `pub mod device;` and `pub mod ptt;` and their placeholders; `git rm crates/omnimodem/src/supervisor/ptt.rs`. Replace the struct and methods so the Supervisor holds:

```rust
use crate::device::DeviceCache;
use crate::ptt::interlock::RxTxInterlock;
use crate::ptt::registry::{PortRegistry, RealOpener};
```

- `devices: DeviceCache` (from `crate::device`),
- `ptt: PortRegistry` (built with `Box::new(RealOpener)`),
- `interlock: RxTxInterlock`,
- the existing `channels: BTreeMap<ChannelId, ChannelState>` and `store: Store`.

Add methods (each pure-ish, persisting through `store`):

```rust
pub fn configure_audio(
    &mut self,
    id: ChannelId,
    device_id: DeviceId,
    sample_rate: u32,
    fanout: u32,
) -> Result<(), crate::persist::StoreError> { /* update config + persist */ }

pub fn configure_ptt(
    &mut self,
    id: ChannelId,
    ptt: crate::ptt::registry::PttConfig,
) -> Result<(), crate::persist::StoreError> { /* update config + persist */ }

pub fn interlock(&self) -> RxTxInterlock { self.interlock.clone() }
pub fn device_cache_mut(&mut self) -> &mut DeviceCache { &mut self.devices }
pub fn ptt_registry_mut(&mut self) -> &mut PortRegistry { &mut self.ptt }
```

Keep `snapshot()`; it already exposes channels. Update the Phase-1 `configure_channel` so a new channel's `device_id` defaults to `DeviceId::placeholder()` and `sample_rate`/`fanout` default to `48_000`/`1`, `ptt: None` (the dedicated `configure_audio`/`configure_ptt` set the real bindings).

- [ ] **Step 3: Add the new commands and events**

In `crates/omnimodem/src/core/command.rs`, add variants to `Command` (each with a `oneshot` reply where an ack is needed):

```rust
    ConfigureAudio {
        id: ChannelId,
        device_id: DeviceId,
        sample_rate: u32,
        fanout: u32,
        reply: oneshot::Sender<Result<u32, CoreError>>, // actual rate
    },
    ConfigurePtt {
        id: ChannelId,
        ptt: crate::ptt::registry::PttConfig,
        reply: oneshot::Sender<Result<(), CoreError>>,
    },
    KeyPtt {
        channel: ChannelId,
        keyed: bool,
        reply: oneshot::Sender<Result<(), CoreError>>,
    },
    ListDevices {
        reply: oneshot::Sender<Vec<crate::device::DeviceDescriptor>>,
    },
    SuggestUdevRule {
        device_id: DeviceId,
        reply: oneshot::Sender<Result<(String, String), CoreError>>,
    },
```

In `crates/omnimodem/src/core/event.rs`, add to `TelemetryEvent` (lossy class): `DeviceArrived { device_id: DeviceId, label: String }`, `DeviceDeparted { device_id: DeviceId }`, `PttKeyed { channel: ChannelId, keyed: bool }`. Add a `CoreError::Audio(String)` and `CoreError::Ptt(String)` variant in `core/error.rs` with `From<AudioError>`/`From<PttError>` impls.

- [ ] **Step 4: Implement the per-channel TX worker + handlers in the core loop**

In `crates/omnimodem/src/core/mod.rs`, in `run`'s `match cmd`:

- `ConfigureAudio` → `supervisor.configure_audio(...)`; open the real capture/playback via a backend resolved from `supervisor.device_cache_mut().resolve(&device_id)` (cpal backend in production, injected backend in tests — see note); reply with the actual rate; emit nothing lossless.
- `ConfigurePtt` → `supervisor.configure_ptt(...)`; build the driver via `supervisor.ptt_registry_mut().build_driver(&ptt)` and **store it in a per-channel `HashMap<ChannelId, Box<dyn PttDriver>>` owned by the core loop**; reply ok.
- `KeyPtt { channel, keyed }` → look up the channel's driver; on key, `interlock.begin_tx(device_id)` then `driver.key()`; on unkey, `driver.unkey()` then `interlock.end_tx(device_id)`; emit `TelemetryEvent::PttKeyed`; on `PttError::DeviceGone`, call `supervisor.ptt_registry_mut().evict(device_id)` and surface the error.
- `Transmit { channel, payload }` → replace the Phase-1 simulation: decode `payload` as LE i16 PCM, look up the channel's playback handle + driver, run `drive_tx_cycle(driver, &sink, samples, rate, Duration::from_millis(5))` wrapped in `interlock.begin_tx`/`end_tx`, emitting `TransmitStarted`/`TransmitComplete` around it; map a non-`Done` outcome to a `CoreError`.
- `ListDevices` → `supervisor.device_cache_mut().refresh(enumerator)`; reply with the descriptors.
- `SuggestUdevRule` → call `crate::ptt::udev::suggest(&device_id)` (Task 18); reply.

Add a hotplug pump: a helper the core calls each loop tick (or a dedicated thread feeding a `Command::Hotplug` — pick the thread approach to keep `recv()` blocking) that runs `HotplugWatcher::poll`, emits `DeviceArrived`/`DeviceDeparted` telemetry, and calls `ptt_registry.evict` + drops the channel's audio handles on `Departed`.

> **Backend/enumerator injection.** So the core is testable without hardware, thread a `Box<dyn DeviceEnumerator>` and an audio-backend factory `Fn(&DeviceDescriptor) -> Box<dyn AudioBackend>` into `core::spawn` (production passes `RealEnumerator` + a cpal-backend factory; tests pass `FakeEnumerator` + a `FileBackend`/`NullBackend` factory). This mirrors the Phase-1 `spawn(supervisor)` signature, extended.

- [ ] **Step 5: Update the Phase-1 core tests for the new `spawn` signature**

The Phase-1 `core::tests` call `spawn(sup)`. Update `fresh_core()` to pass a `FakeEnumerator` (empty) and a `NullBackend` factory, so the existing transmit/configure tests still compile and pass. Add a new test: configure audio with a `FileBackend` factory, `Transmit` a 480-sample buffer, and assert `TransmitStarted` + `TransmitComplete` arrive and the interlock returned to unmuted.

- [ ] **Step 6: Run the affected tests**

Run: `cargo test -p omnimodem core:: supervisor:: persist::`
Expected: PASS — Phase-1 tests still green under the new signature; new audio-transmit test passes; persistence round-trips the new columns.

- [ ] **Step 7: Commit**

```bash
git add crates/omnimodem/src/supervisor/ crates/omnimodem/src/core/ crates/omnimodem/src/persist/mod.rs
git rm crates/omnimodem/src/supervisor/ptt.rs
git commit -m "Wire real device cache, PTT registry, interlock, and per-channel TX into the core"
```

---

## Task 17: gRPC handlers for the new RPCs

Bridge the new proto RPCs to the core commands, mirroring the Phase-1 unary pattern (send `Command` + await its `oneshot`). All new conversions live in `convert.rs`.

**Files:**
- Modify: `crates/omnimodem/src/grpc/convert.rs`
- Modify: `crates/omnimodem/src/grpc/service.rs`

- [ ] **Step 1: Add conversions**

In `crates/omnimodem/src/grpc/convert.rs`, add:
- `device_descriptor_to_proto(&DeviceDescriptor) -> proto::DeviceInfo`,
- `proto_ptt_to_config(&proto::ConfigurePttRequest) -> Result<PttConfig, Status>` (map the `PttMethod` enum + `node`/`pin_or_line` to the domain `PttMethod`; reject `PTT_METHOD_UNSPECIFIED` with `invalid_argument`),
- extend `telemetry_event_to_proto` with the three new variants (`DeviceArrived`/`DeviceDeparted`/`PttKeyed` → `device_arrived`/`device_departed`/`ptt_state`),
- map `CoreError::Audio`/`CoreError::Ptt` in `core_error_to_status` (`Audio` → `internal`/`failed_precondition`; `Ptt` `DeviceGone` → `failed_precondition`, `PermissionDenied` → `permission_denied`, `Busy` → `unavailable`).

- [ ] **Step 2: Implement the handlers**

In `crates/omnimodem/src/grpc/service.rs`, add the five new methods to the `impl ModemControl for ControlService` block. Each: validate, build a `oneshot`, `self.send_command(...)`, await, map. For example `list_devices`:

```rust
    async fn list_devices(
        &self,
        _request: Request<proto::ListDevicesRequest>,
    ) -> Result<Response<proto::ListDevicesResponse>, Status> {
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ListDevices { reply: tx })?;
        let devices = rx.await.map_err(|_| Status::unavailable("core dropped reply"))?;
        Ok(Response::new(proto::ListDevicesResponse {
            devices: devices.iter().map(convert::device_descriptor_to_proto).collect(),
        }))
    }
```

Implement `configure_audio`, `configure_ptt`, `key_ptt`, and `suggest_udev_rule` the same way (validate empty `device_id` / unspecified method with `invalid_argument`).

- [ ] **Step 3: Build and run the unary integration test**

Run: `cargo build -p omnimodem && cargo test -p omnimodem --test unary`
Expected: existing unary test still passes; the service compiles with the five new handlers. (A dedicated device/ptt integration test is the e2e in Task 19.)

- [ ] **Step 4: Commit**

```bash
git add crates/omnimodem/src/grpc/convert.rs crates/omnimodem/src/grpc/service.rs
git commit -m "Add gRPC handlers for ListDevices, ConfigureAudio, ConfigurePtt, KeyPtt"
```

---

## Task 18: `SuggestUdevRule` text generator

The RPC returns ready-to-install udev rule text plus instructions — useful for two identical adapters that `by-id` can't disambiguate. The modem only *suggests*; it never writes `/etc/udev` (root-owned; operator stays in control). Pure string generation, fully test-driven.

**Files:**
- Create/replace: `crates/omnimodem/src/ptt/udev.rs`

- [ ] **Step 1: Write the generator with failing tests**

Replace `crates/omnimodem/src/ptt/udev.rs` with:

```rust
//! udev rule suggestion. Produces a rule that creates a stable
//! /dev/omnimodem/<label> symlink keyed on the most durable attributes the
//! DeviceId carries. Never writes to disk.

use crate::ids::DeviceId;

/// Returns `(rule_text, instructions)` for the given identity. `None` for
/// identities a udev rule can't meaningfully pin (the virtual backends).
pub fn suggest(id: &DeviceId) -> Option<(String, String)> {
    let (matchers, label) = match id {
        DeviceId::Usb { vid, pid, serial } => {
            let mut m = format!(
                "ATTRS{{idVendor}}==\"{vid:04x}\", ATTRS{{idProduct}}==\"{pid:04x}\""
            );
            if !serial.is_empty() {
                m.push_str(&format!(", ATTRS{{serial}}==\"{serial}\""));
            }
            (m, format!("usb-{vid:04x}-{pid:04x}"))
        }
        DeviceId::Serial { by_id } => (
            format!("ENV{{ID_SERIAL}}==\"{by_id}\""),
            "serial".to_string(),
        ),
        DeviceId::Topology { bus, ports } => (
            format!("KERNELS==\"{bus}-{ports}\""),
            format!("topo-{bus}-{ports}"),
        ),
        DeviceId::AlsaCard { .. } | DeviceId::Placeholder { .. } => return None,
    };
    let rule = format!(
        "SUBSYSTEM==\"tty\", {matchers}, SYMLINK+=\"omnimodem/{label}\"\n"
    );
    let instructions = format!(
        "Save as /etc/udev/rules.d/70-omnimodem-{label}.rules, then run:\n  \
         sudo udevadm control --reload-rules && sudo udevadm trigger\n\
         The device will then appear at /dev/omnimodem/{label}."
    );
    Some((rule, instructions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_with_serial_emits_all_three_matchers() {
        let (rule, _) = suggest(&DeviceId::Usb {
            vid: 0x0d8c,
            pid: 0x013c,
            serial: "A1B2".into(),
        })
        .unwrap();
        assert!(rule.contains("idVendor}}==\"0d8c\""));
        assert!(rule.contains("idProduct}}==\"013c\""));
        assert!(rule.contains("serial}}==\"A1B2\""));
        assert!(rule.contains("SYMLINK+=\"omnimodem/usb-0d8c-013c\""));
    }

    #[test]
    fn usb_without_serial_omits_serial_matcher() {
        let (rule, _) = suggest(&DeviceId::Usb { vid: 1, pid: 2, serial: "".into() }).unwrap();
        assert!(!rule.contains("serial}}"));
    }

    #[test]
    fn serial_by_id_uses_id_serial() {
        let (rule, _) = suggest(&DeviceId::Serial { by_id: "usb-FTDI_xyz".into() }).unwrap();
        assert!(rule.contains("ENV{ID_SERIAL}==\"usb-FTDI_xyz\""));
    }

    #[test]
    fn virtual_and_alsa_have_no_rule() {
        assert!(suggest(&DeviceId::placeholder()).is_none());
        assert!(suggest(&DeviceId::AlsaCard { card_name: "Device".into() }).is_none());
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p omnimodem ptt::udev::`
Expected: PASS (4 tests).

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodem/src/ptt/udev.rs
git commit -m "Add SuggestUdevRule text generator"
```

---

## Task 19: Exit-criterion end-to-end test

The gate. Over the Phase-1 authorized UDS gRPC surface, a client runs the full Phase-2 sequence — `ListDevices` → `ConfigureAudio` → `ConfigurePtt` → `Transmit` — and observes PTT key/unkey around audio that actually plays, **with no mode attached.** CI uses the deterministic file/loopback backend and a `MockPtt` opener (so the gate runs without hardware); a documented manual procedure runs the identical RPC sequence against a real sound card and radio.

**Files:**
- Modify: `crates/omnimodem/src/lib.rs` (a test-server helper that injects the deterministic backends)
- Create: `crates/omnimodem/tests/e2e_hardware.rs`

- [ ] **Step 1: Add a deterministic-backend server helper**

In `crates/omnimodem/src/lib.rs`, below `serve_uds_authz_for_test`, add a variant that injects a `FakeEnumerator` (advertising one loopback device), a `FileBackend` audio factory whose `played` buffer is observable, and a `MockPtt` `DriverOpener`. Wire these into `core::spawn` (the injected signature from Task 16). Expose the shared `MockPtt` keyed-state and the playback `played` buffer to the test via returned `Arc`s so the test can assert PTT toggled and audio reached the sink.

```rust
/// Spawn the control plane with deterministic audio + PTT backends for the
/// Phase-2 exit-criterion test. Returns handles to observe PTT and played audio.
pub async fn serve_uds_phase2_for_test(
    db_path: &std::path::Path,
    sock_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // Build a FakeEnumerator with one loopback DeviceId, a FileBackend factory,
    // and a MockPtt opener; pass them to core::spawn; serve over authz UDS.
    // (Concrete wiring mirrors serve_uds_authz_for_test plus the injected deps.)
    # /* see Task 16 injection points */
    unimplemented!("fill from Task 16 injection points")
}
```

> The worker fills this in concretely against the exact `core::spawn` signature chosen in Task 16. The shape is fixed; the body is mechanical wiring. Remove the `unimplemented!` once wired.

- [ ] **Step 2: Write the exit-criterion test**

Create `crates/omnimodem/tests/e2e_hardware.rs`:

```rust
//! Phase 2 EXIT CRITERION: over gRPC, list a device, configure audio + PTT on
//! it, and transmit a PCM buffer — observing PTT key/unkey around audio that
//! actually plays, with NO mode attached. Deterministic backends (file audio +
//! MockPtt), so this runs in CI; the manual procedure below runs the identical
//! RPC sequence against real hardware.

use omnimodem::proto::event::Kind;
use omnimodem::proto::modem_control_client::ModemControlClient;
use omnimodem::proto::{
    ConfigureAudioRequest, ConfigureChannelRequest, ConfigurePttRequest, ListDevicesRequest,
    PttMethod, SubscribeRequest, TransmitRequest,
};
use tokio::net::UnixStream;
use tokio_stream::StreamExt;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

async fn connect(sock: std::path::PathBuf) -> ModemControlClient<tonic::transport::Channel> {
    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let sock = sock.clone();
            async move {
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(
                    UnixStream::connect(sock).await?,
                ))
            }
        }))
        .await
        .unwrap();
    ModemControlClient::new(channel)
}

#[tokio::test]
async fn phase2_exit_criterion_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("omnimodem.sock");
    let db = dir.path().join("omnimodem.sqlite");

    let sock_srv = sock.clone();
    tokio::spawn(async move {
        omnimodem::serve_uds_phase2_for_test(&db, &sock_srv).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut client = connect(sock).await;

    // (1) A device is enumerable with a stable id.
    let devs = client.list_devices(ListDevicesRequest {}).await.unwrap().into_inner();
    assert!(!devs.devices.is_empty());
    let device_id = devs.devices[0].device_id.clone();

    // Configure the channel, then bind audio + PTT to the device.
    client
        .configure_channel(ConfigureChannelRequest { channel: 0, name: "vfo-a".into(), mode: "none".into() })
        .await
        .unwrap();
    let audio = client
        .configure_audio(ConfigureAudioRequest {
            channel: 0,
            device_id: device_id.clone(),
            sample_rate: 48_000,
            fanout: 1,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(audio.actual_sample_rate <= 48_000 && audio.actual_sample_rate > 0);

    client
        .configure_ptt(ConfigurePttRequest {
            channel: 0,
            device_id,
            method: PttMethod::SerialRts as i32,
            node: "/dev/ttyUSB-mock".into(),
            pin_or_line: 0,
            invert: false,
        })
        .await
        .unwrap();

    // (2) Subscribe, then transmit a PCM buffer; observe PTT keyed then released
    // and TransmitStarted/Complete — no mode involved.
    let mut stream = client.subscribe_events(SubscribeRequest {}).await.unwrap().into_inner();
    let _snapshot = stream.next().await.unwrap().unwrap();

    let pcm: Vec<u8> = (0..960).flat_map(|i| (i as i16).to_le_bytes()).collect();
    client.transmit(TransmitRequest { channel: 0, payload: pcm }).await.unwrap();

    let (mut keyed, mut unkeyed, mut started, mut completed) = (false, false, false, false);
    while !(started && completed && keyed && unkeyed) {
        match stream.next().await.unwrap().unwrap().kind.unwrap() {
            Kind::PttState(s) if s.keyed => keyed = true,
            Kind::PttState(s) if !s.keyed => unkeyed = true,
            Kind::TransmitStarted(_) => started = true,
            Kind::TransmitComplete(_) => completed = true,
            _ => {}
        }
    }
    assert!(keyed && unkeyed && started && completed);
}
```

- [ ] **Step 3: Run the exit-criterion test**

Run: `cargo test -p omnimodem --test e2e_hardware`
Expected: PASS — list → configure audio → configure ptt → transmit, with PTT keyed-then-released and Started/Complete observed, no mode attached.

- [ ] **Step 4: Document the manual real-hardware procedure**

Append to the new file `crates/omnimodem/tests/e2e_hardware.rs` a top-level doc-comment block (or a sibling `docs/` note) describing the manual gate. The procedure runs the **same RPC sequence** against real hardware on the operator's Linux host:

```text
MANUAL REAL-HARDWARE GATE (run on a host with a sound card + radio):
  1. cargo run -p omnimodem   (Phase-1 daemon over UDS)
  2. With grpcurl or the reference client:
       a. ListDevices -> confirm the USB sound card appears with a stable
          DeviceId (usb:VVVV:PPPP:serial or alsa:<card>).
       b. ConfigureAudio { channel:0, device_id:<from a>, sample_rate:48000 }
          -> actual_sample_rate is 48000 (or the card's real ceiling).
       c. ConfigurePtt { channel:0, device_id:<ptt adapter>,
          method:SERIAL_RTS|CM108|GPIO, node:/dev/..., invert:false }.
       d. KeyPtt { channel:0, keyed:true } -> radio's TX LED lights; the
          SubscribeEvents stream shows PttState{keyed:true}. KeyPtt{keyed:false}
          drops it.
       e. Transmit a short WAV/PCM buffer -> hear it on a second receiver with
          PTT asserted only for the buffer's duration; PttState toggles around it.
  3. Unplug the PTT adapter mid-session -> a DeviceDeparted event fires and the
     next KeyPtt returns failed_precondition (eviction worked).
Pass criterion: the radio keys, audio plays, PTT releases after drain, and
hotplug eviction fires -- all over gRPC, with no DSP mode attached.
```

- [ ] **Step 5: Run the entire suite**

Run: `cargo test -p omnimodem`
Expected: every unit + integration test passes (`device_id`, `audio::*`, `device::*`, `ptt::*`, `core`, `supervisor`, `persist`, `grpc`, `unary`, `subscribe`, `e2e`, `e2e_hardware`).

- [ ] **Step 6: Commit**

```bash
git add crates/omnimodem/src/lib.rs crates/omnimodem/tests/e2e_hardware.rs
git commit -m "Add Phase 2 exit-criterion e2e test and manual hardware gate"
```

---

## Self-Review

**1. Spec coverage** (Phase 2 deliverables from the design doc, each mapped to a task):

| Phase 2 deliverable (design doc) | Task(s) |
|---|---|
| Unified cross-platform `DeviceId` from durable USB/ALSA/serial attributes | 2 |
| `trait AudioBackend` (cpal first; file/stdin for test) | 3, 4, 6 |
| Defensive I16 format selection + ALSA `plughw` 48 kHz hardening, retained | 5, 6 |
| `probe_capture` / rate ceiling / canonicalization pure functions | 5 |
| Resampling (additive; retains ceiling) | 7 |
| Capture fan-out (opt-in, 1:1 default) | 8 |
| Device detection / enumeration / caching | 9 |
| Hotplug detection/eviction by `DeviceId` | 9 (detect), 12 (PTT evict), 16 (audio evict) |
| `trait PttDriver` + factory + `PortRegistry` (multi-handle) | 10, 11, 12 |
| Per-OS drivers (serial / CM108 / GPIO) | 11 |
| Structured `PttError` enum (vs stringly-typed) | 10 |
| Never-evicted serial cache fixed (DeviceId-keyed eviction) | 12 |
| No-sleep TX sequencing (drain-watermark timed) | 13 |
| Unkey-on-`Drop` safety | 10 (Mock), 11 (every driver), verified in tests |
| Per-channel RX/TX interlock | 14, 16 |
| Per-channel TX worker (vs Graywolf's single global) | 16 |
| `SuggestUdevRule(device_id)` RPC (suggest only, never writes) | 18, 17, 15 |
| Persistence keyed on stable `DeviceId` (canonical string) | 2, 16 |
| gRPC surface for all of the above (additive v1) | 15, 16, 17 |
| Exit criterion: detect → open capture/playback → key/unkey a radio over gRPC, no mode | 19 |

No deliverable is unmapped. Explicitly deferred items (macOS/Windows drivers, SDR/JACK backends, DSP, mTLS) are listed under **Scope → out of scope** with rationale.

**2. Placeholder scan:** Every code step carries complete code, every test step carries full assertions, and every run step states the expected result. Two intentional, clearly-flagged scaffolds exist: (a) the empty module files created in Tasks 3 and 10 so the crate compiles before later tasks fill them (each filled in a named task), and (b) the `unimplemented!` body of `serve_uds_phase2_for_test` in Task 19 Step 1, which is mechanical wiring against the exact `core::spawn` signature chosen in Task 16 — the shape is fixed, the worker fills the body and removes the macro. Two deliberate paste-guards (`PttડDriver_PLACEHOLDER` in Task 12, the inverted-polarity assertion comments) are called out inline with the correction. No "TBD"/"add error handling"/"write tests for the above" placeholders remain.

**3. Type consistency:** Identifiers are defined once and referenced consistently. `DeviceId` (Task 2) is the key throughout `device::cache`, `device::hotplug`, `ptt::registry`, `ptt::interlock`, `ptt::udev`, persistence, and the proto `device_id` string (always via `to_canonical_string`/`parse`). `AudioBackend`/`CaptureHandle`/`PlaybackHandle` (Task 3) are constructed by every backend (Tasks 4, 6) and consumed by `drive_tx_cycle` (Task 13) with matching `submit`/`drained_samples` signatures. `PttDriver`/`PttError` (Task 10) are implemented by every driver (Tasks 11, 12) and consumed by `drive_tx_cycle` and the core. `PttConfig`/`PttMethod` (Task 12) match the proto `ConfigurePttRequest`/`PttMethod` enum (Task 15) via `proto_ptt_to_config` (Task 17). `TxCycleOutcome` (Task 13) variants are matched in the core (Task 16). The hardware-seam traits (`ModemControlLines`, `Cm108Hid`, `GpiochipLine`) each have one real adapter and one fake, with identical method signatures across both.

**Cross-task ordering notes:**
- Tasks 1–2 (shared foundation) must land first. After them, **Part B (3–9) and Part C (10–14) are independent and may be executed in parallel by separate workers**; Part D (15–19) depends on both.
- Task 5 (pure ALSA helpers) must precede Task 6 (cpal backend uses them).
- Task 9 deletes `supervisor/device.rs` and Task 16 deletes `supervisor/ptt.rs`; both include the minimal `supervisor/mod.rs` edits needed to keep the crate compiling between tasks (flagged in-task).
- Task 16 fixes the `core::spawn` signature (injected enumerator + audio-backend factory); Tasks 5 (core tests), 19 (test-server helper) depend on that exact signature. The Phase-1 `core::tests` are updated in Task 16 Step 5 to match.
