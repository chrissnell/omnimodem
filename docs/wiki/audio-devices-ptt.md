# Audio, devices, PTT, KISS, authz

The "rock-solid hardware" half of the daemon: stable device identity, pluggable
audio, robust PTT, the KISS bridge, and the authorization layer. Source under
[`../../crates/omnimodemd/src/`](../../crates/omnimodemd/src/).

## Stable device identity — `DeviceId`

[`ids.rs`](../../crates/omnimodemd/src/ids.rs). One cross-platform identity that
survives renames and hotplug, and is the key everything (config, PTT registry,
hotplug diff) hangs off of. Variants, in rough precedence of durability:

| Variant | Fields | When |
|---|---|---|
| `Usb` | `vid, pid, serial` | USB device exposing a serial number — most durable. |
| `AlsaCard` | `card_name` | ALSA card by its stable *name* (not the volatile index). |
| `Topology` | `bus, ports` | Two identical adapters with no serial — disambiguate by USB port chain. |
| `Serial` | `by_id` | Wraps a `/dev/serial/by-id/<symlink>` (already stable by construction). |
| `RtlTcp` | `host, port` | An `rtl_tcp` SDR endpoint (local or remote); the endpoint *is* the audio device. |
| `Placeholder` | `tag` | Virtual backends (file/stdin/loopback) and test fixtures. |

Each variant round-trips through a canonical string (`usb:VVVV:PPPP:serial`,
`alsa:<name>`, `topo:<bus>-<ports>`, `serial:<by_id>`, `rtltcp:<host>:<port>`,
`virtual:<tag>`), which is what persistence stores. **Config keys on this, never a
`/dev` path** — that is why a renamed or re-enumerated device keeps its channel
binding. An `rtltcp:host:port` id needs no enumeration: bind it ad-hoc and the core
synthesizes a capture-only descriptor.

## Audio backends

`trait AudioBackend` ([`audio/backend.rs`](../../crates/omnimodemd/src/audio/backend.rs))
abstracts capture + playback so the DSP path is hardware-agnostic and CI can run
without hardware.

| Backend | File | Use |
|---|---|---|
| cpal (ALSA/CoreAudio/…) | `audio/cpal_backend.rs` | Real hardware; rebuilds the stream with backoff on error. |
| `rtl_tcp` SDR | `audio/rtlsdr.rs` | Receives off a (local/remote) RTL-SDR dongle over TCP; demods IQ → audio, RX-only. |
| File | `audio/file.rs` | Deterministic replay of a recorded corpus (also the test input). |
| stdin | `audio/stdin.rs` | Raw i16 PCM piped in. |
| Null | `audio/backend.rs` (`NullBackend`) | No-op fallback when a device can't be resolved. |

`CaptureHandle` / `PlaybackHandle` carry the stream's sample rate and cumulative
submitted/drained sample counts (the watermark the no-sleep TX cycle times off).

### `rtl_tcp` SDR backend

[`audio/rtlsdr.rs`](../../crates/omnimodemd/src/audio/rtlsdr.rs) (`RtlTcpBackend`).
Connects to a bare `rtl_tcp` server (no gqrx/`rtl_fm`/`socat`), reads the 12-byte
`RTL0` header, sends 5-byte `[opcode][u32 BE]` commands (center freq, rate, gain
mode/level, ppm), and streams raw u8 IQ. The capture thread runs the Plan-1 DSP
chain (`u8_iq_to_cplx` → `NbfmReceiver`: NCO channel-select → decimate → NBFM
discriminator → power squelch) to deliver mono i16 audio at the channel rate — so
every downstream mode (AFSK1200/APRS first) works unmodified. Playback is
`Unsupported` (dongles are receive-only).

- **`SdrControl`** — an `Arc`-of-atomics runtime cell (the RX analogue of
  `AudioGain`): NCO offset, hardware center, gain (auto/manual dB), squelch dBFS,
  ppm, demod mode. The core (writer, via gRPC in Plan 3) and the capture thread
  (reader) share it, so tune/gain/squelch changes reach a *running* capture with no
  respawn. Effective demod freq = hardware center + NCO offset.
- **RF waterfall** — the capture thread also runs `ComplexStft` + `full_spectrum_dbfs`
  + `SpectrumPlan::new_centered` and emits `SpectrumFrame{transmit:false}` with
  **absolute RF** `freq_start_hz`. The seam: `AudioBackend::attach_sdr_context` (a
  default-no-op trait method) injects the channel id, telemetry sender, and shared
  `SdrControl` from `core::configure_audio`, which the device factory (keyed only on
  identity) cannot supply. For an SDR channel the RX worker's audio-passband spectrum
  tap is held off, so there is exactly one waterfall producer per channel.

### The 48 kHz ceiling

[`audio/mod.rs`](../../crates/omnimodemd/src/audio/mod.rs) (`MAX_SAMPLE_RATE`) and
[`audio/alsa.rs`](../../crates/omnimodemd/src/audio/alsa.rs). Capture never opens
above 48 kHz. ALSA `plughw` will happily advertise rates the codec can't truly honor
(e.g. 192 kHz), silently desyncing bit timing and failing FCS on every frame.
Resampling ([`audio/resample.rs`](../../crates/omnimodemd/src/audio/resample.rs),
`RationalResampler`) is **additive** — it bridges the capped capture rate to the
mode's native rate; it does not replace the defensive rate/format selection.

### Capture fan-out

[`audio/fanout.rs`](../../crates/omnimodemd/src/audio/fanout.rs). One capture stream
can feed several demods (e.g. 1200 + 9600 on the same audio, or SDR slices). Opt-in
via `ConfigureAudio.fanout`; 1:1 is the default and bypasses it.

## Devices: enumeration, cache, hotplug

| Concern | File | Symbol |
|---|---|---|
| Enumerator trait + descriptor + fakes | `device/enumerate.rs` | `DeviceEnumerator`, `DeviceDescriptor`, `FakeEnumerator` |
| Production enumerator (cpal + nusb) | `device/mod.rs` | `RealEnumerator` |
| Cache: `DeviceId` → live device | `device/cache.rs` | `DeviceCache` |
| Hotplug diff (arrivals/departures) | `device/hotplug.rs` | `HotplugWatcher` |

`RealEnumerator` unifies what were two diverging identity paths in Graywolf
(cpal/ALSA vs nusb) into one `DeviceId`. `HotplugWatcher::poll` diffs successive
enumerations to emit `Arrived`/`Departed`; the core acts on those (see
[`grpc-edge.md`](grpc-edge.md#hotplug)).

## PTT

`trait PttDriver` + structured `PttError`
([`ptt/mod.rs`](../../crates/omnimodemd/src/ptt/mod.rs)). `PttError` distinguishes
device-gone vs permission-denied vs busy, so callers (and hotplug eviction) can
react specifically instead of parsing strings.

| Method (`PttMethod`) | Driver | File |
|---|---|---|
| `SERIAL_RTS` / `SERIAL_DTR` | serial line via TTY `ioctl(TIOCMSET)` | `ptt/serial.rs` |
| `CM108` | USB HID audio-codec GPIO (5-byte HID report, pins 1–8) | `ptt/cm108.rs` |
| `GPIO` | Linux gpiochip chardev v2 (`gpiocdev`) | `ptt/gpio.rs` |
| `NONE` / `VOX` | no-op (VOX keys off audio, no control line) | `ptt/none.rs` |

Cross-cutting PTT machinery:

| Concern | File | Symbol |
|---|---|---|
| Driver factory + registry + **hotplug eviction/reopen by `DeviceId`** | `ptt/registry.rs` | `PortRegistry`, `DriverOpener` |
| Keying sequence: `tx_delay` lead-in, audio, watermark drain, `tx_tail` hold; cancel-interruptible | `ptt/sequence.rs` | `drive_tx_cycle` |
| RX/TX interlock (per-rig, nesting-safe counter) | `ptt/interlock.rs` | `RxTxInterlock` |
| Exclusive TX lease (per-rig) | `ptt/lease.rs` | `TxLeaseRegistry` |
| udev rule text generation (never writes disk) | `ptt/udev.rs` | `suggest` |

Safety rules worth knowing (details in [`invariants.md`](invariants.md)): every
driver **unkeys on `Drop`** (a stuck transmitter is a licensing hazard); the
interlock mutes RX while keyed; the keying sequence times off the DAC watermark
rather than sleeping, and aborts promptly on cancel (mode change).

## KISS bridge

[`kiss/`](../../crates/omnimodemd/src/kiss/). `ConfigureKissListener` starts a
per-channel KISS-over-TCP listener (`kiss/listener.rs::KissRegistry`) so legacy TNC
apps (Direwolf, APRX, pat, Xastir) can drive a **packet** channel (AFSK 1200 AX.25).
`kiss/codec.rs` implements FEND/FESC framing: inbound KISS data frames become
`Transmit` commands, outbound decoded packets become KISS data frames. Only packet
modes are eligible.

## Authorization

[`authz/`](../../crates/omnimodemd/src/authz/). Opening the control socket means the
ability to key a transmitter under the operator's license, so authz is enforced even
locally.

- **UDS (default)** — `authz/uds.rs`: the socket file mode is hardened and each
  request's peer credentials (`SO_PEERCRED`) must show a uid equal to the daemon's.
- **Routable mTLS** — `authz/tls.rs`: when `OMNIMODEM_ROUTABLE_ADDR` is set, the
  daemon binds TCP and **requires** server cert/key + client CA; it fails closed if
  the material is missing, and clients must present a valid client certificate.
- **Transport select + validation** — `authz/mod.rs`: `Transport`,
  `serve_uds`/`serve_routable`, and `validate_transport` (which warns/fails before
  binding).
