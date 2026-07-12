# Omnimodem — Native (Local USB) RTL-SDR Support

**Date:** 2026-07-12
**Status:** Design approved in principle (issue GRA-338); phased delivery below. No
code yet — this doc is the source of truth for what lands and in what order.

## Summary & goals

Add a **native, locally-attached RTL-SDR source** so omnimodem can discover and drive
a USB RTL-SDR dongle **directly**, with **no `rtl_tcp` sidecar** and **no C
dependency**. Today a local dongle only works if the operator installs and runs
`rtl_tcp` separately and binds `rtltcp:127.0.0.1:1234`; this feature lets a user plug
in a dongle, start omnimodem, and have it **appear in device discovery** exactly like a
sound card — selectable and configurable over gRPC.

The USB transport is implemented in **pure Rust on `nusb`** — already a workspace
dependency (`device/enumerate.rs`) — so no `librtlsdr`, no FFI, no build-system or
runtime C library, and the cross-platform story (Linux / macOS / Windows) stays a
single code path.

Goals:

1. **Discover** locally-attached RTL-SDR dongles and surface them in `ListDevices`
   and hotplug, identically to how audio devices appear.
2. **Three selection modes over gRPC**, all via the existing `ConfigureAudio` binding:
   (a) an **auto-detected** dongle, (b) a **manually configured** dongle pinned in the
   config file, (c) a **remote** `rtl_tcp` endpoint (unchanged).
3. **Native RX**: initialize the RTL2832U + R820T/R828D, tune, set gain/rate/ppm, and
   stream raw IQ over USB into the **existing** IQ→audio DSP chain.
4. **Reuse the entire SDR control surface** — `SetSdrTune`, `SetSdrGain`,
   `ConfigureSdr`, `GetSdrCaps`, the RF waterfall, squelch — with zero API change.
5. **Cross-platform**: Linux, macOS, and Windows, with each platform's USB-claim
   reality documented rather than papered over.

## Non-goals

- **TX via SDR** — RTL dongles are RX-only; unchanged from the `rtl_tcp` design.
- **Tuners beyond R820T/R828D** — E4000 / FC0012 / FC0013 / R828D variants past the
  common case are deferred. The common cheap dongle (RTL2832U + R820T/R828D) is the
  target.
- **Non-RTL SDR hardware** (Airspy, HackRF, SDRplay, ham-radio transceivers). This is
  an explicit **fast-follow** (see Phase 4 and "Forward compatibility"), scoped in its
  own brainstorm; this doc only draws the seam so that work doesn't require re-plumbing.
- **Removing or changing `rtl_tcp`.** The remote path stays exactly as shipped; native
  is added alongside it.

## Background

### What already exists

The `rtl_tcp` SDR source (`crates/omnimodem/src/audio/rtlsdr.rs`, design
`docs/design/2026-07-06-rtl-tcp-sdr-input-design.md`) already implements the full
IQ→audio pipeline behind the `AudioBackend` trait: complex NCO channel-select,
NBFM/AM/WFM/SSB/RawMag demodulation, decimation to audio, power squelch, the
RF-referenced wideband waterfall, and the `SdrControl` cell that all four SDR gRPC
RPCs drive. None of that is `rtl_tcp`-specific in principle — it operates on raw u8 IQ
and a shared control snapshot.

What *is* `rtl_tcp`-specific: the TCP connect + 12-byte header handshake, the `RtlCmd`
5-byte command encoding written to a `TcpStream`, the socket IQ read loop, and the
reconnect supervisor. This is the layer native support replaces.

### What `nusb` gives us and what it costs per-OS

`nusb` is a pure-Rust USB library (Linux usbfs, macOS IOKit, Windows WinUSB) supporting
control transfers and bulk endpoints — everything an RTL2832U needs (register writes
via control transfers, raw IQ via a bulk IN endpoint). It is already the workspace's
USB enumeration path. Using it keeps the "self-contained modem, no external glue"
posture and the pure-`cargo` CI.

The catch is **claiming** the device from user space, which differs by OS (see
"Cross-platform" below) — this is inherent to user-space USB, not a `nusb` limitation.

## Architecture

### The seam: extract an `SdrTransport`, reuse everything above it

`audio/rtlsdr.rs` currently interleaves the wire protocol with the DSP. We factor out a
small **internal `SdrTransport` trait** that captures exactly what the capture thread
needs from "the dongle", regardless of whether it is reached over TCP or USB:

```rust
// Internal to the audio module; not part of the gRPC surface.
trait SdrTransport: Send {
    /// Fill `buf` with raw interleaved u8 IQ; blocks until data or error.
    fn read_iq(&mut self, buf: &mut [u8]) -> Result<usize, AudioError>;
    /// Apply the current hardware parameters from the control snapshot
    /// (center freq, sample rate, gain mode/level, ppm, bias-tee,
    /// direct-sampling). Called at connect and on control changes.
    fn apply_hardware(&mut self, control: &SdrControl) -> Result<(), AudioError>;
    /// Capabilities discovered at open (tuner type, ranges, gain table).
    fn caps(&self) -> TunerCaps;
    /// Unblock a thread parked in `read_iq` so `stop` is honored promptly.
    fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send>;
}
```

Two implementations:

- **`RtlTcpTransport`** — the existing code, behavior-identical: `RtlCmd` → `TcpStream`,
  socket reads, reconnect/backoff. The current `connect_and_handshake`,
  `send_initial_commands`, `apply_hardware`, and socket read become this impl.
- **`RtlUsbTransport`** — `nusb` control transfers for RTL2832U + tuner register writes,
  a bulk-endpoint read for raw IQ, and device open/claim (with per-OS kernel-driver
  handling). No network, no reconnect supervisor (a local USB drop is a hard error /
  hotplug-departure, not a retry loop).

Everything above the transport is **reused verbatim**: the complex NCO channelizer, the
demodulators, decimation, squelch, `deliver_audio`, the RF waterfall tap, the whole
`SdrControl` cell, and all four gRPC RPCs. The capture thread's structure (build RX
chain from control, read IQ, channelize/demod/decimate, deliver audio, emit waterfall)
is unchanged; only its source of IQ and its "apply hardware" call go through the trait.

`RtlUsbBackend` becomes a second `AudioBackend` impl next to `RtlTcpBackend`, each
holding its transport.

```
                          ┌────────────────── AudioBackend seam ──────────────────┐
DeviceId::RtlTcp ─▶ RtlTcpBackend ─┐                                               │
                                   ├─▶ capture thread ─▶ SdrTransport::read_iq ────┤
DeviceId::Rtl    ─▶ RtlUsbBackend ─┘        │                                      │
                                            ▼                                      │
                          NCO channel-select ▶ demod ▶ decimate ▶ squelch ▶ i16 ──┤▶ modem
                                            │                                      │
                                            └▶ RF waterfall (SpectrumFrame) ───────┘
                          SdrControl (shared) ◀── SetSdrTune / SetSdrGain / ConfigureSdr / GetSdrCaps
```

### RtlUsbTransport internals

- **Open & claim**: match the dongle by identity (below), open via `nusb`, detach any
  conflicting kernel driver where applicable, and claim the interface.
- **Init**: the RTL2832U demod reset + tuner probe/init. Probe for R820T/R828D; run its
  register init sequence; program IF. This is the bulk of the real work and is ported
  from the well-known librtlsdr register sequences (pure Rust, no linkage) — optionally
  leaning on an existing pure-Rust reference (e.g. `rtlsdr_rs`) for the tables.
- **Control**: `set_center_freq`, `set_sample_rate`, `set_gain_mode`/`set_tuner_gain`,
  `set_freq_correction` (ppm), `set_bias_tee`, `set_direct_sampling` — each a small
  control-transfer sequence. `apply_hardware` maps the `SdrControl` snapshot onto these,
  mirroring the fields `RtlTcpTransport` already sends.
- **Streaming**: submit bulk IN transfers and hand the returned u8 IQ to `read_iq`.
  `nusb`'s async transfer queue backs this; the capture thread sees the same u8 stream
  the socket read produced.
- **Caps**: `TunerCaps` built from the probed tuner (reusing the existing
  `tuner_freq_range` / `tuner_gains_db` / `supported_sample_rates` tables), so
  `GetSdrCaps` answers identically whether the tuner was learned from a USB probe or an
  `rtl_tcp` header.

## Device identity & the three selection modes

All three modes bind through the **existing** `ConfigureAudio{ device_id }` seam — no
new selection RPC.

| Mode | device_id | Source |
|---|---|---|
| (a) auto-detected | `rtl:<key>` | Discovered by enumeration; user picks it from `ListDevices`. |
| (b) manually configured | `rtl:<key>` | Pinned in the daemon config file; same variant. |
| (c) remote | `rtltcp:<host>:<port>` | Unchanged. |

A new `DeviceId::Rtl` variant with canonical scheme `rtl:<key>` is added to `ids.rs`
(parse + `to_canonical_string`, with round-trip tests alongside the existing variants).

**Identity `<key>` — the subtle part.** Cheap dongles frequently ship with a **blank or
duplicated serial** (`00000001`), so serial alone is not a safe key. Identity resolves
as:

1. **USB serial string** when present and unique among attached RTL dongles →
   `rtl:serial:<s>`. Stable across USB ports and reboots.
2. Otherwise **USB bus topology** (bus + port chain) → `rtl:topo:<bus>-<ports>`. This is
   exactly the disambiguation `DeviceId::Topology` already performs for sound cards, so
   "the dongle in this physical port" stays stable even with a junk serial.

Manual config may pin either form. Discovery prefers the serial form when it is unique.
The `rtl:` scheme namespaces these sub-forms so parsing stays unambiguous.

## Discovery & hotplug — reuses the existing spine

`RealEnumerator::enumerate` (`device/mod.rs`, `device/enumerate.rs`) already bridges
cpal + nusb. We add an nusb scan for known RTL2832U VID/PIDs (0x0bda:0x2832,
0x0bda:0x2838, plus the small set of documented clone IDs) that emits, per present
dongle:

```rust
DeviceDescriptor {
    id: DeviceId::Rtl { .. },   // serial or topology key per above
    label: <USB product string, e.g. "RTL2838UHIDIR">,
    has_capture: true,
    has_playback: false,        // RX-only
}
```

Because `HotplugWatcher` (`device/hotplug.rs`) detects change by **diffing enumeration
snapshots**, arrival/departure and appearance in `ListDevices` come for free — the
dongle shows up in and disappears from the config menu exactly like an audio device,
which is the target experience. No new hotplug mechanism is introduced; the poll-diff
model already in place covers USB add/remove.

A dongle that is **present but not claimable** (e.g. Windows without WinUSB bound, or
Linux without permission) is still reported by `enumerate` with a distinct
"needs driver setup / permission" flag in `DeviceDescriptor`, so a frontend can **guide
the user** instead of silently omitting the device. (Adds one optional field to
`DeviceDescriptor`; additive.)

## Cross-platform reality

`nusb` compiles to one code path across OSes, but claiming an RTL dongle from user space
differs, and the design does not oversell "just fire it up":

- **Linux** — the kernel binds `dvb_usb_rtl28xxu` (and `rtl2832`); the transport detaches
  it at open (`nusb` detach-and-claim) and we ship a udev rule granting the runtime user
  access to the RTL VID/PIDs. After the one-time udev rule (or running as root / in a
  privileged container), discovery and use are fully automatic. Blacklisting the DVB
  modules is the common alternative and will be documented.
- **macOS** — no competing Apple kernel driver claims the RTL2832U, so `nusb` (IOKit)
  claims it directly. No per-device setup. This is the cleanest platform.
- **Windows** — user-space USB requires the **WinUSB driver bound to the device**, a
  one-time step the user performs with **Zadig**. `nusb` (WinUSB) cannot bypass this; it
  is how *every* Windows SDR application works (SDR#, SDR++, gqrx all require Zadig).
  Discovery and selection work once WinUSB is bound; the "needs driver setup" state above
  surfaces an un-bound dongle so the UI can point the user at the one-time Zadig step.

This per-OS behavior is Phase 3 work and is documented in the operator handbook.

## gRPC

**No new RPCs.** Selection is `ConfigureAudio{ channel, device_id: "rtl:<key>" }`, the
same binding mechanism used for sound cards and `rtltcp:`. All RF control already ships
and is transport-agnostic:

- `SetSdrTune`, `SetSdrGain`, `ConfigureSdr`, `GetSdrCaps` — reused as-is; the daemon
  routes them to the bound channel's `SdrControl` regardless of transport.
- `GetSdrCaps` is populated from the native tuner probe (via `TunerCaps`) instead of the
  `rtl_tcp` connect header — same message, same fields.
- The RF waterfall (`SpectrumFrame`) and `SdrState` events are unchanged.

The only additive proto-adjacent change is the optional `DeviceDescriptor`
"needs-setup" flag surfaced through `ListDevices`; it is back-compatible.

## Phased delivery

Per the project convention, deferred work is documented and planned here and executed
before the feature is called "done."

| Phase | Scope | "Done" gate |
|---|---|---|
| **1 — Discovery + seam** | `SdrTransport` trait extraction with `RtlTcpTransport` behavior-identical; `DeviceId::Rtl` (`rtl:serial:*` / `rtl:topo:*`) parse/canonical + tests; nusb RTL scan in `RealEnumerator`; `ListDevices`/hotplug/config wiring; `needs-setup` descriptor flag. No IQ over USB yet. | `rtl_tcp` path unchanged (existing fake-server tests green); a plugged dongle appears in `ListDevices` and hotplug with a stable id. |
| **2 — Native RX** | `RtlUsbTransport`: open/claim, RTL2832U + R820T/R828D init, set freq/rate/gain/ppm, bulk IQ read → existing DSP. `GetSdrCaps` from the probe. | End-to-end with **no `rtl_tcp` running**: bind `rtl:<key>`, tune 144.390, decode APRS off a real dongle; waterfall + click-tune work in the TUI. |
| **3 — Cross-platform hardening** | Linux kernel-driver detach + udev rule + docs; macOS validation; Windows WinUSB detection + "needs-setup" surfacing + Zadig docs; USB error/overrun/removal handling (hard-stop + hotplug-departure, not reconnect). | Works on Linux and macOS out of the flow; Windows works after documented Zadig step; a mid-capture unplug reports cleanly and the channel unbinds. |
| **4 — Ham-SDR fast-follow (separate spec)** | Generalize the `SdrTransport` / device-identity abstraction toward popular ham SDR hardware. Scoped in its own brainstorm. | Out of scope here; Phase 1's seam is drawn so this needs no re-plumbing. |

## Forward compatibility (fast-follow)

The user wants popular ham-community SDR hardware next. The `SdrTransport` seam is drawn
so that a future device implements the same trait (raw IQ + apply-hardware + caps) and
reuses the whole DSP + gRPC surface. Where a future radio is not RTL2832U-based, only a
new transport and a new `DeviceId` scheme are added — the channelizer, demods, waterfall,
and control RPCs are unchanged. Radios with different sample formats (e.g. 16-bit IQ)
are accommodated by widening the transport's IQ contract at that time; today's u8 path is
the RTL case and stays the default.

## Testing

- **`SdrTransport` refactor safety**: the existing in-process **fake `rtl_tcp` server**
  suite (header parse, command round-trip, u8→Cplx, reconnect, overrun/drop-oldest) runs
  unchanged against `RtlTcpTransport` to prove the extraction is behavior-preserving.
- **DeviceId**: round-trip + parse tests for `rtl:serial:*` and `rtl:topo:*`, including
  rejection of malformed keys, alongside the existing variant tests.
- **Enumeration**: `FakeEnumerator`-style tests that a synthetic RTL descriptor produces
  the right `Arrived`/`Departed` hotplug events and the right `ListDevices` entry,
  including the `needs-setup` flag path.
- **`RtlUsbTransport`**: the register/tuner-init sequences and control encodings are
  unit-tested against expected byte sequences (no hardware); the `nusb` open/claim path
  is covered by a thin, mockable device seam so logic is testable without a dongle.
- **Integration (hardware-in-the-loop, manual/CI-optional)**: a real dongle tuned to a
  known signal decodes end-to-end; kept out of the default `cargo test` gate since it
  needs hardware, documented as a bring-up checklist.
- **DSP**: unchanged — already covered by the `rtl_tcp` design's DSP tests, which the
  native path reuses.

## Decisions record

- **Native pure-Rust `nusb`, no C deps** — approved (user strongly preferred no C
  dependency; `nusb` already in-tree).
- **Discovery-first UX** — approved; dongles appear in `ListDevices`/hotplug like audio
  devices; three selection modes (auto / manual / remote) via existing `ConfigureAudio`.
- **`SdrTransport` extraction, `rtl_tcp` behavior-identical** — approved; proven by the
  existing fake-server test suite.
- **Scope = RTL2832U + R820T/R828D** — approved; other tuners deferred.
- **macOS + Windows required** — approved; Windows carries a documented one-time Zadig
  prerequisite (inherent to user-space USB, matches all Windows SDR apps), surfaced via a
  "needs-setup" device state rather than engineered away.
- **Ham-SDR support is a fast-follow with its own spec** — approved; seam drawn to avoid
  re-plumbing.
