# Native (local USB) RTL-SDR Support — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: subagent-driven-development / executing-plans, strict TDD, frequent commits. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Discover and drive locally-attached RTL-SDR dongles directly over USB
(pure-Rust `nusb`, no C deps), alongside the existing `rtl_tcp` path, so a user
can plug in a dongle, start omnimodem, and select it like a sound card.

**Architecture:** Extract an internal `SdrTransport` trait so the entire IQ→audio
DSP chain and all four SDR gRPC RPCs are reused verbatim; add a second transport
(`RtlUsbTransport`) and a second `AudioBackend` (`RtlUsbBackend`) behind a new
`DeviceId::Rtl`. Discovery/hotplug reuse the existing `RealEnumerator` /
`HotplugWatcher` poll-diff spine.

**Tech stack:** Rust, `nusb` 0.1 (already a workspace dep), the existing
`omnimodem-dsp` crate, tonic/prost gRPC.

**Spec:** `docs/design/2026-07-12-native-rtl-sdr-design.md`.

**Build/test:** `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0 cargo test -p omnimodem`.
Never run `cargo fmt`. Commit as `chrissnell` only, no AI attribution.

---

## Scope & phasing

Three phases → **11 discrete chunks** (each a PR-sized unit → one Multica
sub-issue). Phase 4 (ham-SDR hardware) is a **separate future spec** and is not
planned here.

### Dependency graph (order of work)

```
P1-A  SdrTransport seam ─────────────┐
P1-B  DeviceId::Rtl ──┬──────────────┤
                      └─▶ P1-C  Discovery + hotplug + needs-setup
                                        │
P1-A + P1-B ─▶ P2-A  USB open/claim ─▶ P2-B  RTL2832U init ─▶ P2-C  Tuner ─▶ P2-D  Params/caps ─▶ P2-E  Streaming + backend
                                                                                        (also needs P1-C) ─┘
                                                                                                            │
                                                        P2-E ─┬─▶ P3-A  USB error/removal handling
                                                              ├─▶ P3-B  Cross-platform (udev/WinUSB/macOS)  (also needs P1-C)
                                                              │
                                            P3-A + P3-B ─────▶ P3-C  Docs + wiki + bring-up checklist
```

- **Start immediately (no deps):** P1-A, P1-B (parallel).
- **Strictly serial chain:** P2-A → P2-B → P2-C → P2-D → P2-E (hardware bring-up
  order — each layer needs the one below).
- **Parallel after P2-E:** P3-A and P3-B.
- **Last:** P3-C.

## File structure

| File | Responsibility | Chunk |
|---|---|---|
| `crates/omnimodem/src/audio/sdr/mod.rs` | `SdrTransport` trait; module root for SDR transports | P1-A |
| `crates/omnimodem/src/audio/sdr/rtl_tcp.rs` | `RtlTcpTransport` (moved from `rtlsdr.rs`, behavior-identical) | P1-A |
| `crates/omnimodem/src/audio/sdr/pipeline.rs` | The shared capture-thread body (NCO/demod/decimate/squelch/waterfall/deliver) parameterized over `SdrTransport` | P1-A |
| `crates/omnimodem/src/audio/sdr/usb.rs` | `RtlUsbTransport` + `RtlUsbBackend` | P2-A..E |
| `crates/omnimodem/src/audio/sdr/usb_regs.rs` | RTL2832U + tuner register constants/sequences (ported from librtlsdr) | P2-B, P2-C |
| `crates/omnimodem/src/ids.rs` | `DeviceId::Rtl` variant + parse/canonical | P1-B |
| `crates/omnimodem/src/device/enumerate.rs` | nusb RTL scan; `needs_setup` on `DeviceDescriptor` | P1-C, P3-B |
| `crates/omnimodem/src/lib.rs` | factory arm `DeviceId::Rtl => RtlUsbBackend` | P2-E |
| `crates/omnimodem/src/config.rs` | manual `rtl:` device registration | P1-C |
| `docs/wiki/*`, `docs/running.md`, `packaging/` | operator docs, udev rule, Zadig guide | P3-B, P3-C |

**Note on the refactor:** `audio/rtlsdr.rs` is renamed/split into the `audio/sdr/`
module. `RtlTcpBackend` keeps its public name and behavior; only its internals move
behind the trait. The existing fake-`rtl_tcp` test suite is the regression gate and
must stay green with zero test-logic changes (only import-path updates).

---

## Phase 1 — Discovery + seam

### P1-A — Extract the `SdrTransport` seam (rtl_tcp behavior-identical)

**Files:**
- Create: `crates/omnimodem/src/audio/sdr/mod.rs`, `.../sdr/rtl_tcp.rs`, `.../sdr/pipeline.rs`
- Modify: `crates/omnimodem/src/audio/mod.rs` (module wiring), delete `audio/rtlsdr.rs` after move
- Test: existing tests in the moved module (fake `rtl_tcp` server) + one new trait test

**Depends on:** none.

- [ ] **Step 1: Define the trait (write it, then a compile-only test).** In `sdr/mod.rs`:

```rust
/// Everything the capture pipeline needs from "the dongle", regardless of
/// whether it is reached over TCP or USB. Internal to the audio module.
pub(crate) trait SdrTransport: Send {
    /// Fill `buf` with raw interleaved u8 IQ; block until data or error.
    /// Returns the number of bytes written (may be < buf.len()).
    fn read_iq(&mut self, buf: &mut [u8]) -> Result<usize, AudioError>;
    /// Apply current hardware params from the control snapshot: center freq,
    /// sample rate, gain mode/level, ppm, bias-tee, direct-sampling.
    fn apply_hardware(&mut self, control: &SdrControl) -> Result<(), AudioError>;
    /// Tuner capabilities discovered at open (for GetSdrCaps).
    fn caps(&self) -> TunerCaps;
    /// A closure that unblocks a thread parked in `read_iq`, so the backend's
    /// stop hook is honored promptly.
    fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send>;
}
```

- [ ] **Step 2: Move the rtl_tcp wire code into `RtlTcpTransport`.** Move
  `connect_and_handshake`, `send_initial_commands`, `apply_hardware`, `RtlCmd`,
  the socket read, and the reconnect supervisor from `rtlsdr.rs` into
  `sdr/rtl_tcp.rs` as `struct RtlTcpTransport` implementing `SdrTransport`.
  `read_iq` wraps the socket read + reconnect; `apply_hardware` sends the
  existing `RtlCmd` sequence; `caps()` returns the header-derived `TunerCaps`;
  `shutdown_handle` shuts the current socket down.

- [ ] **Step 3: Move the DSP body into `pipeline.rs`.** Extract the capture-thread
  body (build RX chain from `SdrControl`, `read_iq`, channelize/demod/decimate,
  `deliver_audio`, `emit_waterfall`) into
  `fn run_capture(transport: Box<dyn SdrTransport>, ctx: CaptureCtx)` where
  `CaptureCtx` carries channel id, telemetry sender, control, tx, stop. Both
  backends spawn this same function.

- [ ] **Step 4: Re-express `RtlTcpBackend` in terms of the seam.** `open_capture`
  constructs an `RtlTcpTransport` (initial connect stays synchronous so a bad
  address fails fast + caps publish), then spawns `run_capture`. Public API and
  `device_id()` unchanged.

- [ ] **Step 5: Update imports and run the full existing suite.**

Run: `CARGO_TARGET_DIR=/tmp/omni-target cargo test -p omnimodem`
Expected: **all pre-existing rtl_tcp tests PASS unchanged** (header parse, command
round-trip, u8→Cplx, reconnect, overrun/drop-oldest). This is the behavior-identical gate.

- [ ] **Step 6: Add a trait-level test.** A `FakeTransport` yielding a scripted IQ
  slice drives `run_capture` and asserts the delivered audio matches the existing
  NBFM-tone expectation — proving the pipeline is transport-agnostic.

- [ ] **Step 7: Commit.**

```bash
git add crates/omnimodem/src/audio
git commit -m "refactor: extract SdrTransport seam; move rtl_tcp behind it"
```

### P1-B — `DeviceId::Rtl` variant

**Files:**
- Modify: `crates/omnimodem/src/ids.rs`
- Test: `crates/omnimodem/src/ids.rs` (tests module)

**Depends on:** none.

- [ ] **Step 1: Write failing round-trip tests.** In the `ids.rs` tests module:

```rust
#[test]
fn rtl_serial_and_topo_roundtrip() {
    roundtrip(DeviceId::Rtl { key: RtlKey::Serial("00000001".into()) });
    roundtrip(DeviceId::Rtl { key: RtlKey::Topo { bus: 1, ports: "1.4".into() } });
    assert_eq!(
        DeviceId::Rtl { key: RtlKey::Serial("abc".into()) }.to_canonical_string(),
        "rtl:serial:abc"
    );
    assert_eq!(DeviceId::parse("rtl:topo:2-1.3"),
        Some(DeviceId::Rtl { key: RtlKey::Topo { bus: 2, ports: "1.3".into() } }));
    assert_eq!(DeviceId::parse("rtl:bogus:x"), None);
}
```

- [ ] **Step 2: Run to confirm it fails.** Run:
  `cargo test -p omnimodem ids::tests::rtl_serial_and_topo_roundtrip` → FAIL (no variant).

- [ ] **Step 3: Add the variant + parse/canonical.**

```rust
pub enum RtlKey { Serial(String), Topo { bus: u8, ports: String } }
// in enum DeviceId: Rtl { key: RtlKey },
// to_canonical_string:
DeviceId::Rtl { key: RtlKey::Serial(s) }     => format!("rtl:serial:{s}"),
DeviceId::Rtl { key: RtlKey::Topo { bus, ports } } => format!("rtl:topo:{bus}-{ports}"),
// parse, scheme "rtl": split body on first ':' into subkind + rest;
//   "serial" => Serial(rest); "topo" => split rest on first '-' => bus,ports; else None.
```

- [ ] **Step 4: Run tests to PASS.** Run: `cargo test -p omnimodem ids::` → PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/omnimodem/src/ids.rs
git commit -m "feat: add DeviceId::Rtl (rtl:serial / rtl:topo) identity"
```

### P1-C — Discovery, hotplug, `needs_setup`, config binding

**Files:**
- Modify: `crates/omnimodem/src/device/enumerate.rs` (nusb RTL scan + `needs_setup` field)
- Modify: `crates/omnimodem/src/config.rs` (accept `rtl:` in device config)
- Test: `enumerate.rs` tests (via a mockable USB-list seam)

**Depends on:** P1-B.

- [ ] **Step 1: Add `needs_setup` to `DeviceDescriptor`.** Add
  `pub needs_setup: bool` (default `false` for all existing constructors —
  update the cpal path to set it `false`). Compile the workspace.

- [ ] **Step 2: Write a failing discovery test.** Introduce a `UsbLister` seam
  (`trait UsbLister { fn list(&self) -> Vec<UsbDev>; }`, `UsbDev { vid, pid, serial, bus, ports, product, claimable }`) so the RTL scan is testable without hardware. Test:

```rust
#[test]
fn rtl_dongle_becomes_a_capture_only_descriptor() {
    let lister = FakeUsbLister(vec![UsbDev {
        vid: 0x0bda, pid: 0x2838, serial: Some("00000001".into()),
        bus: 1, ports: "4".into(), product: "RTL2838UHIDIR".into(), claimable: true,
    }]);
    let devs = scan_rtl(&lister);
    assert_eq!(devs[0].id, DeviceId::Rtl { key: RtlKey::Serial("00000001".into()) });
    assert!(devs[0].has_capture && !devs[0].has_playback && !devs[0].needs_setup);
}

#[test]
fn duplicate_serials_fall_back_to_topology() {
    let dup = |bus, ports: &str| UsbDev { vid:0x0bda, pid:0x2832, serial: Some("00000001".into()),
        bus, ports: ports.into(), product: "RTL".into(), claimable: true };
    let devs = scan_rtl(&FakeUsbLister(vec![dup(1,"1"), dup(1,"2")]));
    assert!(matches!(devs[0].id, DeviceId::Rtl { key: RtlKey::Topo { .. } }));
    assert!(matches!(devs[1].id, DeviceId::Rtl { key: RtlKey::Topo { .. } }));
}

#[test]
fn unclaimable_dongle_is_reported_with_needs_setup() {
    let d = UsbDev { vid:0x0bda, pid:0x2838, serial: None, bus:1, ports:"4".into(),
        product:"RTL".into(), claimable:false };
    let devs = scan_rtl(&FakeUsbLister(vec![d]));
    assert!(devs[0].needs_setup && devs[0].has_capture);
}
```

- [ ] **Step 3: Run to confirm failure.** Run:
  `cargo test -p omnimodem enumerate::tests::rtl` → FAIL.

- [ ] **Step 4: Implement `scan_rtl`.** Match known RTL VID/PIDs
  (`0x0bda:0x2832`, `0x0bda:0x2838`, plus the documented clone list); prefer
  `RtlKey::Serial` when the serial is present and unique among the scan, else
  `RtlKey::Topo { bus, ports }`; set `needs_setup = !claimable`. Wire a real
  `NusbLister` (implements `UsbLister` via `nusb::list_devices()`) into
  `RealEnumerator::enumerate` so RTL descriptors are appended after cpal devices.

- [ ] **Step 5: Run tests to PASS.** Run: `cargo test -p omnimodem enumerate::` → PASS.

- [ ] **Step 6: Config binding.** In `config.rs`, accept `rtl:...` device ids in
  the manual device list (parse via `DeviceId::parse`); add a round-trip test
  mirroring the existing `rtltcp:` config test.

- [ ] **Step 7: Hotplug is automatic — assert it.** Add a `FakeEnumerator`-based
  test that adding/removing an `rtl:` descriptor yields the expected
  `HotplugEvent::Arrived` / `Departed` (no new hotplug code; proves the diff spine
  covers USB).

- [ ] **Step 8: Commit.**

```bash
git add crates/omnimodem/src/device crates/omnimodem/src/config.rs
git commit -m "feat: discover local RTL dongles via nusb; needs_setup + hotplug + config"
```

---

## Phase 2 — Native RX

> Register/tuner sequences (P2-B, P2-C, P2-D) are **ported from librtlsdr**
> (`librtlsdr.c`, `tuner_r82xx.c`) — the byte-level source of truth. The plan
> gives the structure, endpoints, and test shape; literal register tables are
> transcribed from that reference at implementation time and unit-tested against
> the reference's expected sequences. This is a transcription task, not invention.

### P2-A — `RtlUsbTransport`: open, claim, kernel-driver detach

**Files:**
- Create: `crates/omnimodem/src/audio/sdr/usb.rs`
- Test: `usb.rs` tests (mockable device seam)

**Depends on:** P1-A, P1-B.

- [ ] **Step 1: Device open seam.** Define `struct RtlUsbTransport { dev: nusb::Interface, iface: u8, tuner: TunerKind, caps: TunerCaps }` and a `open(key: &RtlKey) -> Result<Self, AudioError>` that: lists via nusb, matches the `RtlKey`, opens the device, on Linux **detaches the kernel driver** (`dvb_usb_rtl28xxu`) and claims interface 0, on macOS/Windows claims directly. Map claim failure to a distinct `AudioError::UsbClaim` (feeds `needs_setup`).

- [ ] **Step 2: Control-transfer helpers + test.** Implement `read_reg`/`write_reg`
  (RTL2832U control transfers: `bmRequestType`/`bRequest` per librtlsdr's
  `rtlsdr_read_reg`/`write_reg`) against a `FakeUsb` seam. Test asserts the exact
  `ControlIn`/`ControlOut` setup packets for a known register write.

- [ ] **Step 3: Run + commit.** Run: `cargo test -p omnimodem sdr::usb` → PASS.

```bash
git commit -am "feat(rtl-usb): open/claim + control-transfer register I/O"
```

### P2-B — RTL2832U demod init

**Files:** Create `.../sdr/usb_regs.rs`; modify `.../sdr/usb.rs`. Test: `usb.rs`.

**Depends on:** P2-A.

- [ ] **Step 1:** Transcribe the RTL2832U init sequence (demod reset, USB block
  init, IF/AGC setup) from librtlsdr `rtlsdr_init_baseband` into
  `fn init_baseband(&mut self)` as an ordered list of `write_reg` calls in
  `usb_regs.rs`.
- [ ] **Step 2:** Unit-test that `init_baseband` emits the reference sequence
  (assert the ordered `(block, addr, val)` list against the transcribed table).
- [ ] **Step 3:** Implement `set_sample_rate` (RTL2832U resampler ratio math from
  librtlsdr `rtlsdr_set_sample_rate`) + test the ratio for 240 kHz and 2.4 MHz.
- [ ] **Step 4:** Run + commit `feat(rtl-usb): RTL2832U baseband init + sample rate`.

### P2-C — R820T/R828D tuner probe, init, tune, gain

**Files:** modify `.../sdr/usb_regs.rs`, `.../sdr/usb.rs`. Test: `usb.rs`.

**Depends on:** P2-B.

- [ ] **Step 1:** Tuner probe: read the tuner i2c id; map to
  `TunerKind::{R820T, R828D}` (others → `AudioError::UnsupportedTuner`). Test the
  id→kind mapping.
- [ ] **Step 2:** Transcribe R82xx init array + `set_freq` (LO/PLL from
  `tuner_r82xx.c`) into `usb_regs.rs`; `set_tuner_freq(hz)`. Test the PLL divider
  math for a known frequency (e.g. 144.39 MHz) against the reference.
- [ ] **Step 3:** `set_gain_mode(auto)` + `set_tuner_gain(tenths_db)` snapping to
  the R82xx gain table (reuse existing `snap_gain` + `tuner_gains_db`). Test snap.
- [ ] **Step 4:** Run + commit `feat(rtl-usb): R820T/R828D tuner init, tune, gain`.

### P2-D — Params, `apply_hardware`, `caps()`

**Files:** modify `.../sdr/usb.rs`. Test: `usb.rs`.

**Depends on:** P2-C.

- [ ] **Step 1:** `set_freq_correction(ppm)`, `set_bias_tee(on)`,
  `set_direct_sampling(mode)` (librtlsdr equivalents). Unit-test the ppm→register
  conversion.
- [ ] **Step 2:** Implement `SdrTransport::apply_hardware` mapping the `SdrControl`
  snapshot onto the setters, **in the same order** `RtlTcpTransport` sends them
  (rate, ppm, direct-sampling, bias-tee, gain mode/level, center freq). Test that a
  given `SdrControl` produces the expected ordered setter calls (mock seam).
- [ ] **Step 3:** Implement `caps()` from the probed tuner using the existing
  `tuner_freq_range` / `tuner_gains_db` / `supported_sample_rates` so `GetSdrCaps`
  answers identically to the rtl_tcp path. Test caps for `R820T`.
- [ ] **Step 4:** Run + commit `feat(rtl-usb): params, apply_hardware, caps`.

### P2-E — Bulk IQ streaming + `RtlUsbBackend` + factory wiring (end-to-end)

**Files:** modify `.../sdr/usb.rs`, `crates/omnimodem/src/lib.rs`. Test: `usb.rs`.

**Depends on:** P2-D, P1-C.

- [ ] **Step 1:** Implement `SdrTransport::read_iq` via a queued bulk IN transfer
  (`nusb` async transfer queue on the RTL bulk endpoint 0x81), returning raw u8 IQ
  bytes. `shutdown_handle` cancels the transfer queue. Reset the endpoint before
  first read (librtlsdr `rtlsdr_reset_buffer`).
- [ ] **Step 2:** `RtlUsbBackend` implements `AudioBackend`: `open_capture` opens
  `RtlUsbTransport`, calls `apply_hardware` from control, spawns the shared
  `run_capture` (P1-A). `device_id()` returns the `DeviceId::Rtl`. `open_playback`
  returns `AudioError::Unsupported` (RX-only). `attach_sdr_context` stores channel/
  telemetry/control exactly like `RtlTcpBackend`.
- [ ] **Step 3:** Factory arm in `lib.rs`:

```rust
if let ids::DeviceId::Rtl { key } = &desc.id {
    return Box::new(audio::sdr::usb::RtlUsbBackend::new(key.clone()));
}
```

- [ ] **Step 4:** Integration test: a `FakeUsb` scripted with an FM-modulated
  AFSK1200 APRS burst → `RtlUsbBackend` → AFSK1200 mode → correct AX.25 frame
  (reuses the mode conformance harness, mirroring the rtl_tcp integration test).
- [ ] **Step 5:** Run full suite + commit `feat(rtl-usb): bulk IQ streaming + backend + factory (end-to-end RX)`.

---

## Phase 3 — Cross-platform hardening

### P3-A — USB error, removal, and overrun handling

**Files:** modify `.../sdr/usb.rs`, `.../sdr/pipeline.rs`. Test: `usb.rs`, `pipeline.rs`.

**Depends on:** P2-E.

- [ ] **Step 1:** A mid-capture unplug (bulk transfer error / `NO_DEVICE`) is a
  **terminal stop** (not a reconnect like rtl_tcp): `read_iq` returns
  `AudioError::UsbLost`, `run_capture` exits, the channel unbinds, and hotplug
  reports `Departed`. Test with a `FakeUsb` that errors after N transfers.
- [ ] **Step 2:** Confirm `deliver_audio`'s drop-oldest overrun path already
  applies to the USB source (shared pipeline) — add a test that a slow consumer
  increments `dropped_chunks` without stalling the transfer queue.
- [ ] **Step 3:** Run + commit `feat(rtl-usb): terminal removal handling + overrun coverage`.

### P3-B — Cross-platform claim (Linux udev / macOS / Windows WinUSB)

**Files:** create `packaging/udev/99-omnimodem-rtlsdr.rules`; modify
`device/enumerate.rs` (WinUSB-unbound → `needs_setup`); docs.

**Depends on:** P2-E, P1-C.

- [ ] **Step 1:** Ship `packaging/udev/99-omnimodem-rtlsdr.rules`
  (`SUBSYSTEM=="usb", ATTR{idVendor}=="0bda", ATTR{idProduct}=="2832"|"2838", MODE="0660", TAG+="uaccess"`)
  and document install + DVB-module blacklist in `docs/running.md`.
- [ ] **Step 2:** Windows: detect a present-but-unbound (no WinUSB) RTL device in
  the nusb scan and set `needs_setup = true` so `ListDevices` surfaces it; document
  the one-time **Zadig** step. Test the mapping via `FakeUsbLister { claimable:false }`.
- [ ] **Step 3:** macOS: validate direct claim; document that no per-device setup
  is required. (Validation is manual/hardware; capture the checklist in docs.)
- [ ] **Step 4:** Commit `feat(rtl-usb): udev rule, WinUSB needs-setup surfacing, cross-platform docs`.

### P3-C — Operator docs, wiki, bring-up checklist

**Files:** create `docs/sdr-rtl-usb.md`; modify `docs/wiki/code-map.md`,
`docs/wiki/audio-devices-ptt.md`, `docs/running.md`.

**Depends on:** P3-A, P3-B.

- [ ] **Step 1:** Write `docs/sdr-rtl-usb.md`: selecting an auto-detected vs manual
  vs remote dongle, per-OS setup, and the hardware bring-up checklist (plug in →
  appears in `ListDevices` → bind `rtl:...` → tune 144.390 → decode APRS).
- [ ] **Step 2:** Update `docs/wiki/code-map.md` with the new `audio/sdr/*` module
  rows and the `RtlUsbBackend`/`SdrTransport` entries (the wiki is how future agents
  find this code — required by project policy when code lands).
- [ ] **Step 3:** Commit `docs: native RTL-USB operator guide + wiki code-map`.

---

## Self-review

- **Spec coverage:** goals 1–5 → P1-C (discover), P1-C/P2-E/config (3 modes),
  P2-B..E (native RX), P1-A + P2-D (reuse control surface), P3-B (cross-platform).
  Non-goals (TX, other tuners, other hardware, removing rtl_tcp) respected — P2-C
  rejects non-R82xx tuners; rtl_tcp path only refactored, never removed.
- **Placeholder scan:** no "TBD/handle appropriately"; the only externally-sourced
  content is the librtlsdr register tables, explicitly scoped as a transcription
  task with a unit-test gate (P2 preamble), not a vague placeholder.
- **Type consistency:** `SdrTransport` (`read_iq`/`apply_hardware`/`caps`/
  `shutdown_handle`), `RtlKey::{Serial,Topo}`, `DeviceDescriptor.needs_setup`,
  `run_capture`, `scan_rtl`, `RtlUsbBackend`, `RtlUsbTransport` used consistently
  across P1-A → P3-C.
```
