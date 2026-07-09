# Omnimodem gRPC API reference

This is the integration surface. A single service, `omnimodem.v1.ModemControl`,
defined in [`../proto/omnimodem.proto`](../proto/omnimodem.proto). The `.proto`
file is authoritative for wire details (field tags, types); this document explains
what each RPC does, how the pieces fit, and how to drive a full session.

> **Stability.** The package is versioned `omnimodem.v1` and is **additive-only**
> within a major (new fields/RPCs/enum values only; tags are never reused or
> renumbered). A breaking change means a new package (`omnimodem.v2`) served
> alongside `v1`. Policy: [`../proto/VERSIONING.md`](../proto/VERSIONING.md).

## Model

The daemon owns a set of **channels**. A channel is a logical pipeline:
`audio → demod → frames + metrics` on receive, and `frame → mod → audio + PTT` on
transmit. Each channel has an id (`uint32`, chosen by the client), a **mode**, a
bound **audio device** (capture, optionally split playback), and a bound **PTT**.
Channel config is persisted (SQLite, keyed on stable device identity), so it
survives daemon restarts and device renames/hotplug.

You operate the daemon by:

1. **Command-and-control** — unary RPCs that configure and act (configure a
   channel/audio/PTT, transmit, query state, manage leases, etc.). Each is
   validated and acknowledged.
2. **Event subscription** — one server-streaming RPC, `SubscribeEvents`, that
   replays a **state snapshot first**, then streams live events (decoded frames,
   telemetry, device/PTT changes, waterfall lines, RSID hits).

### Two event delivery classes

Backpressure policy is deliberate and load-bearing (see
[`wiki/invariants.md`](wiki/invariants.md)):

- **Lossless — decoded RX frames** (`RxFrame`). A decoded frame is never silently
  dropped. A subscriber that falls too far behind is disconnected (with a
  resource-exhausted status) rather than losing a frame. Reconnect and the
  snapshot brings you current.
- **Lossy — telemetry** (`AudioLevel`, `Status`, `ChannelMetrics`, `PttState`,
  `SpectrumFrame`, `ClockOffset`, `DeviceArrived`/`Departed`, `RsidDetected`, …).
  Only the latest value matters; a slow subscriber skips intermediate values and
  keeps streaming.

Design a client accordingly: treat `RxFrame` as a reliable log, and treat
telemetry as a best-effort gauge.

## Transport & authorization

Opening the control socket means the ability to key a transmitter under the
operator's license, so authorization is enforced **even locally**.

- **Default: Unix-domain socket.** The daemon binds
  `${OMNIMODEM_RUNTIME_DIR}/omnimodem.sock` (default
  `<tempdir>/omnimodem/omnimodem.sock`, i.e. `/tmp/omnimodem/omnimodem.sock` on
  Linux). The socket file mode is hardened and peer identity is checked via
  `SO_PEERCRED` (peer uid must match the daemon's uid).
- **Routable: mTLS.** Set `OMNIMODEM_ROUTABLE_ADDR` to bind a TCP endpoint
  instead. This **requires** TLS material (server cert/key + client CA); the
  daemon fails closed if it is missing. Clients must present a valid client
  certificate.

A client connects with a standard gRPC channel over the UDS or the TCP endpoint;
no extra handshake beyond the transport's auth. The crate version is surfaced in
handshake metadata.

## RPC reference

### Channel configuration

#### `ConfigureChannel(ConfigureChannelRequest) → ConfigureChannelResponse`
Create or update a virtual channel. **Idempotent on `channel` id** — reusing an id
updates that channel in place. Fields:

- `channel` — logical id.
- `name` — operator-facing label.
- `mode` — mode selector. A bare label (`"cw"`, `"ft8"`, `"psk31"`) resolves with
  default parameters; a parametric string (`"cw:wpm=25,tone=600"`) overrides
  individual params.
- `mode_params` — optional **typed** per-mode parameters (a `oneof` selecting the
  mode and its values). When present it is authoritative and `mode` is ignored.
  Prefer this for parametric modes; see [Mode parameters](#mode-parameters).
- `rsid_tx` / `rsid_rx` — enable prepending the mode's RSID burst before each TX,
  and/or running the RSID detector over received audio (surfacing `RsidDetected`).
  Both default off. proto3 scalars have no presence, so **resend the current
  values** whenever you reconfigure a channel.

#### `ConfigureAudio(ConfigureAudioRequest) → ConfigureAudioResponse`
Bind a channel's audio to a device (by stable `device_id`) and open the streams.
`device_id` is the capture (RX) device — config keys on this. `sample_rate` is the
requested RX working rate (clamped to a 48 kHz ceiling — see invariants). `fanout`
lets one capture stream feed several demods (0/1 = no fan-out). Optional split
playback: set `tx_device_id` (empty = same as capture) and `tx_sample_rate`
(0 = same as `sample_rate`). The response echoes the rates the streams **actually**
opened at.

#### `ConfigurePtt(ConfigurePttRequest) → ConfigurePttResponse`
Bind a channel's PTT to a device + method. `method` is a `PttMethod`
(`NONE`, `VOX`, `SERIAL_RTS`, `SERIAL_DTR`, `CM108`, `GPIO`). `node` is the
resolved node/chip path (serial/cm108/gpio); `pin_or_line` is the CM108 GPIO pin
(1–8) or the gpiochip line offset; `invert` flips the sense. `tx_delay_ms` /
`tx_tail_ms` are per-channel keying timing: a keyed-but-silent lead-in before audio
(lets the rig's PTT close) and a hold after audio drains before releasing. A full
`ConfigurePtt` **replaces** the binding, so always send the current values (0 is a
valid explicit setting).

#### `KeyPtt(KeyPttRequest) → KeyPttResponse`
Manually key/unkey a channel's PTT with no audio (operator test). `keyed` = true to
key, false to unkey.

#### `SetAudioGain(SetAudioGainRequest) → SetAudioGainResponse`
Adjust a channel's RX input gain and TX output gain at runtime. Linear multipliers
(`1.0` = unity). Takes effect on the running workers without a respawn.

#### `ConfigureSpectrum(ConfigureSpectrumRequest) → ConfigureSpectrumResponse`
Enable/disable and size a channel's waterfall (spectrum) stream. **Default OFF**, so
the FFT costs nothing when no one is watching. `enable=false` stops the producer
(other fields ignored). `bin_count` is the display width (0 = server default 256);
`fft_size` is the FFT length (0 = default 2048, rounded to a power of two); `rate_hz`
is target lines/sec (0 = default 15); `freq_lo_hz`/`freq_hi_hz` window the passband
(0 = 0 Hz / Nyquist). The response echoes the **actual** clamped params, which match
the `SpectrumFrame` fields you'll then receive. Idempotent per channel.

### State & metrics

#### `GetState(GetStateRequest) → ModemState`
Snapshot the current channels. Each `ChannelInfo` carries the channel's id, name,
mode, RX/TX/PTT device ids, PTT method, RSID flags, running flag, and PTT timing.
This is the same shape delivered as `Event.snapshot` on subscribe.

#### `GetMetrics(GetMetricsRequest) → GetMetricsResponse`
Snapshot per-channel metrics. `channel = 0` means all channels. Each
`ChannelMetrics` has `good_frames`, `bad_frames`, `snr_db`, `dbfs`,
`afc_offset_hz`, `dcd` (data-carrier detect), and `last_decoder` (which ensemble
member/decoder produced the last frame). The same message also streams as a lossy
`ChannelMetrics` event.

#### `SubscribeEvents(SubscribeRequest) → stream Event`
Subscribe to the event stream. **The first message is always `Event.snapshot`**
(a `ModemState` reflecting subscription-time state); live events follow. Ordering
is at-least-once: a change applied between subscribe and snapshot may appear in
both — treat the snapshot as authoritative and tolerate a duplicate follow-up. The
`Event` `oneof` `kind` is one of:

| Event | Class | Meaning |
|---|---|---|
| `snapshot` | — | Always first. Full `ModemState`. |
| `channel_configured` | lossy | A channel's config was applied. |
| `transmit_started` / `transmit_complete` | lossless-adjacent | TX lifecycle for a `transmit_id`. |
| `rx_frame` | **lossless** | A decoded frame (text/packet in `data`, or a raster `image`). |
| `audio_level` | lossy | Per-channel input dBFS. |
| `status` | lossy | Per-channel counters (e.g. `tx_frames`). |
| `device_arrived` / `device_departed` | lossy | Hotplug. |
| `ptt_state` | lossy | PTT keyed/unkeyed. |
| `clock_offset` | lossy | Host clock discipline (NTP offset, error, synchronized). |
| `channel_metrics` | lossy | Same as `GetMetrics`, pushed. |
| `spectrum_frame` | lossy | One waterfall line (see below). |
| `rsid_detected` | lossy | An RSID burst was identified (tag, mode, freq, extended). |

### Transmit

#### `Transmit(TransmitRequest) → TransmitResponse`
Enqueue a frame for transmission on a channel. `payload` is opaque frame bytes.
Returns a monotonically increasing `transmit_id` **once the frame is accepted onto
the channel's TX queue — not when it leaves the air**. Watch for `TransmitStarted`
then `TransmitComplete` events carrying that id to follow the on-air lifecycle.

#### `TransmitImage(TransmitImageRequest) → TransmitResponse`
Transmit a raster image over the channel's configured picture-capable mode
(MFSK / THOR / IFKP / FSQ; the daemon builds the in-band header + pixel-FSK and
keys the rig). `rgb` is row-major interleaved RGB (`R,G,B,…`, `width*height*3`
bytes). `color` sends three planes vs grayscale luma. `txspp` is MFSK
samples-per-pixel (8 default / 4 / 2; 0 ⇒ 8). Same acceptance/`transmit_id`
contract as `Transmit`. MFSK carries an explicit W×H so any size works;
THOR/IFKP/FSQ select from a fixed size table, so `(width, height[, color])` must
match one of the mode's sizes. This is the symmetric partner of the typed
`RxFrame.image` on receive.

### TX arbitration (leases)

#### `AcquireTxLease(TxLeaseRequest) → TxLeaseResponse` / `ReleaseTxLease(...)`
Acquire/release a channel's **exclusive TX lease** on its bound rig, for sessions
that can't tolerate interleaving (contest/Winlink). The lease is **per-rig, not
per-channel** — two channels sharing one physical rig cannot both transmit.
`granted=false` means another channel holds it; `held_by` names the current holder
(0 if none). The cooperative TX queue works without a lease; the lease is an
optional escalation to exclusivity.

### Devices

#### `ListDevices(ListDevicesRequest) → ListDevicesResponse`
Enumerate present audio/PTT-capable devices. Each `DeviceInfo` has the canonical,
stable `device_id`, an operator-facing `label`, and `has_capture`/`has_playback`
flags. Use these `device_id`s with `ConfigureAudio`/`ConfigurePtt` — they are
durable across renames and hotplug.

#### `SuggestUdevRule(SuggestUdevRuleRequest) → SuggestUdevRuleResponse`
Return ready-to-install udev rule text for a device (creating a stable
`/dev/omnimodem/<label>` symlink). The daemon **only suggests** — it never writes
`/etc/udev`. Response carries the `rule` text and `instructions` for where to put
it.

### Legacy TNC bridge

#### `ConfigureKissListener(ConfigureKissListenerRequest) → ConfigureKissListenerResponse`
Start/stop an in-daemon KISS-over-TCP listener for a **packet** channel (AFSK 1200
AX.25), so legacy TNC apps (Direwolf, APRX, pat, Xastir) speak KISS to omnimodem.
`bind_addr` is `host:port` (`:0` picks a port); `enable=true` starts/replaces,
`false` stops (bind_addr ignored). The response reports the actual `bound_addr` and
whether a listener is `active`.

## Mode parameters

For parametric modes, prefer the typed `ModeParams` `oneof` on `ConfigureChannel`
over the parametric string. Each variant selects the mode and supplies its params.
Summary (see the `.proto` for exact fields and the
[mode catalog](wiki/mode-catalog.md) for what each mode is):

| `ModeParams` variant | Mode family | Key fields |
|---|---|---|
| `cw` | CW / Morse | `wpm`, `tone_hz` |
| `rtty` | RTTY | `baud`, `shift_hz`, `center_hz`, `reverse` |
| `psk31` | PSK31 (legacy) | `center_hz` |
| `psk` | PSK family (psk31/63/125/250/500/1000 + QPSK) | `submode`, `center_hz` |
| `olivia` | Olivia | `tones`, `bandwidth_hz` |
| `contestia` | Contestia | `tones`, `bandwidth_hz` |
| `mfsk` | MFSK family | `submode`, `center_hz` |
| `dominoex` | DominoEX family | `submode`, `center_hz` |
| `thor` | THOR family | `submode`, `center_hz` |
| `ifkp` | IFKP | `speed`, `center_hz` |
| `fsq` | FSQ / FSQCALL | `speed`, `center_hz`, `mycall`, `directed` |
| `mt63` | MT63 family | `submode`, `center_hz` |
| `hell` | Hellschreiber family (raster RX) | `submode`, `center_hz` |
| `throb` | Throb family | `submode`, `center_hz` |
| `navtex` | NAVTEX / SITOR-B | `submode`, `center_hz` |
| `wefax` | WEFAX (raster RX) | `submode`, `center_hz` |
| `afsk1200` | AX.25 1200 AFSK packet | `tx` |

Modes not listed here (FT8, FT4, JT65, JT9, JT4, WSPR, FST4, MSK144, JS8, SSTV) take
no parameters beyond the bare mode label, or are selected by label alone (e.g. JT4's
submodes are `jt4a`…`jt4g`). Typed params
are converted to the equivalent internal mode string in the daemon, then resolved
by the mode registry — so `mode: "cw:wpm=25,tone=600"` and the `cw` `ModeParams`
variant are equivalent.

## Rasters: receiving and sending images

Facsimile/raster modes (Hell, WEFAX, SSTV) and the picture sub-protocols populate
the typed `RxFrame.image` field and leave `data` empty; a receiver picks whichever
is set. An `Image` is `width` (pixels per row), `pixels` (row-major 8-bit samples),
and `channels` (1 = grayscale luma, 3 = RGB interleaved; 0 on the wire ⇒ 1). Raster
modes append incrementally — each successive `width*channels`-byte row is one
on-air column — so a receiver renders a growing image line by line. To transmit,
use `TransmitImage` (see above).

## Waterfall (`SpectrumFrame`)

One `SpectrumFrame` is one waterfall line: a magnitude spectrum over the channel's
passband. Map each `uint8` in `bins` to a shade (low→high frequency, `len ==
bin_count`), label the X axis from `freq_start_hz` + `freq_step_hz`, and map the
dB range with `db_floor` (bin value 0) and `db_ceiling` (bin value 255).
`timestamp_ns` is the window's leading edge; `transmit` distinguishes TX spectrum
from RX. Enable/size the stream with `ConfigureSpectrum` first; it is lossy, so a
lagging renderer simply drops lines.

## A worked session

```
# 1. Discover hardware
ListDevices {}                          → devices[]: pick a device_id

# 2. Create a channel and bind it
ConfigureChannel { channel: 0, name: "HF FT8", mode: "ft8" }
ConfigureAudio   { channel: 0, device_id: "<id>", sample_rate: 48000 }
ConfigurePtt     { channel: 0, device_id: "<id>", method: PTT_METHOD_CM108,
                   pin_or_line: 3, tx_delay_ms: 50, tx_tail_ms: 20 }

# 3. Subscribe (snapshot first, then live)
SubscribeEvents {}                      → Event.snapshot { channels: [...] }
                                        → Event.rx_frame / audio_level / clock_offset / ...

# 4. Optionally watch the waterfall
ConfigureSpectrum { channel: 0, enable: true, bin_count: 256 }
                                        → Event.spectrum_frame ...

# 5. Transmit (FT8 aligns to the next slot internally)
AcquireTxLease { channel: 0 }           → granted: true      (optional; exclusive)
Transmit { channel: 0, payload: <opaque frame bytes> }
                                        → TransmitResponse { transmit_id: N }
                                        → Event.transmit_started { transmit_id: N }
                                        → Event.transmit_complete { transmit_id: N }
ReleaseTxLease { channel: 0 }
```

## See also

- [`../proto/omnimodem.proto`](../proto/omnimodem.proto) — the authoritative schema.
- [`../proto/VERSIONING.md`](../proto/VERSIONING.md) — stability policy.
- [`wiki/mode-catalog.md`](wiki/mode-catalog.md) — what every mode is, and its submodes.
- [`wiki/system-topology.md`](wiki/system-topology.md) — processes, sockets, env vars.
- [`wiki/tui-client.md`](wiki/tui-client.md) — the reference client, RPC by RPC.
- [`wiki/invariants.md`](wiki/invariants.md) — the backpressure and authz rules in depth.
