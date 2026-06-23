# Omnimodem TUI Client — Design

> Status: **Design / RFC.** Targets the existing `omnimodem.v1` gRPC API.
> Scope: configuration + digital-mode **TX**. RX (decode display) is deferred.

A terminal client for `omnimodemd`, built with [Bubble Tea](https://github.com/charmbracelet/bubbletea).
It lets an operator enumerate audio/PTT hardware, bind a channel, pick a digital
mode, and send a message — hearing the modulated signal on the output device.

## 1. Goals & non-goals

**Goals (MVP):**
1. Connect to a local `omnimodemd` over its gRPC control plane.
2. Enumerate audio + PTT devices; select RX/TX audio and a PTT device + method.
3. Bind a channel (`ConfigureChannel` + `ConfigureAudio` + `ConfigurePtt`).
4. Show live audio levels (dBFS meter) and PTT state from the event stream.
5. Select a digital mode and compose + transmit a message.
6. Surface TX progress (started/complete), TX lease, and clock-sync for FT8.

**Non-goals (deferred):**
- RX decode display (`RxFrame` rendering) — comes after TX is solid.
- KISS/packet operation — that path is the in-daemon KISS listener, not typed messages.

> **Update (after #20):** runtime gain (`SetAudioGain`) and split RX/TX device
> binding (`ConfigureAudio.tx_device_id`) both landed, so the levels panel and
> separate RX/TX device selection are now in scope for MVP — see [§5.3](#53-configuration-per-channel).

## 2. Where it lives

`omnimodemd` is a Rust workspace; the TUI is Go. Proposal: a self-contained Go
module at **`clients/omnimodem-tui/`** in this repo, generating bindings from
`proto/omnimodem.proto` via `protoc-gen-go` + `protoc-gen-go-grpc`. One repo keeps
the proto and its first reference frontend versioned together. (Alternative: a
separate repo — see [§7](#7-open-questions).)

## 3. Transport & connection

- **Default:** UDS at the daemon's runtime socket (`…/omnimodem.sock`). Go dials it
  with a custom `unix` dialer; the daemon authorizes via socket mode (0600) +
  `SO_PEERCRED`, so no app-level auth is needed locally.
- **Remote (optional):** TCP + mTLS when the daemon is started with
  `OMNIMODEM_ROUTABLE_ADDR`. The TUI takes a `--addr` / `--tls-{ca,cert,key}` set.
- On connect: call `GetState` for the initial snapshot, then open `SubscribeEvents`
  (whose first message is also a snapshot — we reconcile to it).

## 4. Architecture (Elm/Bubble Tea)

Single root `Model` with a `screen` enum and shared sub-state. The gRPC event
stream is bridged into Bubble Tea the idiomatic way:

```
SubscribeEvents (goroutine) ──> buffered chan Event ──> waitForEvent (tea.Cmd)
                                                          └─> returns eventMsg, re-issued each Update
```

- **LOSSY** events (AudioLevel, PttState, Status, Device{Arrived,Departed},
  ClockOffset, ChannelMetrics) → coalesce, keep latest; never block the UI.
- **LOSSLESS** events (RxFrame) → buffered, but ignored in the TX-only MVP.
- All mutating RPCs run as `tea.Cmd`s returning result/err msgs — the Update loop
  stays non-blocking. State of record = the snapshot + event deltas, not polling.

Components: `bubbles/list` (devices, modes), `textinput`/`textarea` (composer &
PTT fields), `table` (channel dashboard), `progress` (dBFS meter, FT8 slot timer),
`help`, `lipgloss` for layout.

## 5. Screens

### 5.1 Connection
Pick/confirm socket path or routable addr → connect → show daemon version & state.

### 5.2 Dashboard (home)
Table of channels: id, name, mode, bound device, PTT state, live dBFS bar, TX-lease
holder. Live-updated from the event stream. Keys to jump into Config or Compose.

### 5.3 Configuration (per channel)
- **Device enumeration:** `ListDevices` → split into capture-capable (RX) and
  playback-capable (TX) views from `DeviceInfo{has_capture,has_playback}`. A typical
  USB rig interface is one device exposing both (pick it once for RX+TX); since #20,
  a split rig can bind a separate playback card via `tx_device_id`. So the screen
  offers an **RX capture device**, an **optional separate TX playback device**
  (defaults to the RX device), and a **PTT device**. Device hotplug
  (`DeviceArrived`/`Departed`) updates the lists live.
- **Audio bind:** `ConfigureAudio(channel, device_id, sample_rate=48000, fanout,
  tx_device_id?, tx_sample_rate?)`; show the returned `actual_sample_rate`.
- **PTT bind:** `ConfigurePtt(channel, device_id, method, node, pin_or_line, invert)`
  with a method picker (NONE/VOX/SERIAL_RTS/SERIAL_DTR/CM108/GPIO) and the
  pin/line/invert fields gated by method. `KeyPtt` gives a manual key/unkey test.
- **Permissions helper:** on a bind that needs udev access (CM108/serial/GPIO),
  offer `SuggestUdevRule(device_id)` and display the rule text + install instructions
  (the daemon never writes it).
- **Levels:** live dBFS meter driven by `AudioLevel` events, with **RX/TX gain
  sliders** wired to `SetAudioGain(channel, rx_gain, tx_gain)` (linear, 1.0 =
  unity) — landed in #20, so the operator sets levels by eye against the live meter.

### 5.4 Compose & TX (per channel)

**Not every mode is a chat.** These modes fall into three interaction shapes, and
the compose surface should match the mode rather than force one universal text box
(see [§5.5](#55-interaction-model-chat-vs-structured-vs-packet)):

- **Ragchew / keyboard modes — PSK31, RTTY, CW.** Genuinely conversational,
  back-and-forth free text. Surface = a **chat transcript**: a scrollback viewport
  of interleaved RX/TX lines with timestamps + a compose line at the bottom. (RX
  lines are deferred, so the MVP transcript is TX-only, but the layout is built to
  drop decoded RX in unchanged once that path lands — the chat UI is the payoff.)
- **Structured QSO — FT8.** *Not* a chat. A rigid, clock-driven exchange ladder
  (CQ → grid → signal report → R-report → RRR → 73) in 15 s slots, ~13-char
  payloads. Surface = a **QSO sequencer**: my call/grid, target call, the standard
  message ladder as selectable steps, the slot clock, and a constrained free-text
  escape hatch. A chat box would be the wrong metaphor here.
- **AFSK1200 — packet *or* chat.** AFSK is just Bell-202 modulation, not a
  protocol. In omnimodem today the modulator HDLC-frames whatever bytes you hand
  it (`Afsk1200Mod::modulate` → preamble flags + `hdlc_frame(payload)` + FCS) and
  this is *not* AX.25-specific — AX.25 is merely the usual payload. So AFSK1200 can
  back the **chat transcript** too: each typed line rides as an HDLC-framed payload
  via the raw `Transmit` path, no daemon change. The **AX.25/KISS** path
  (`ConfigureKissListener`) is the separate, interop-oriented use for
  Direwolf/APRS. Caveat: *truly* raw async-serial AFSK (no HDLC at all, classic
  Bell-202 TTY) would need a transparent-framing path added to the modem — see
  [§7](#7-open-questions).

- **Mode select:** picks the surface above. Mode params (CW wpm/tone, RTTY
  baud/shift, PSK31 center) need a wire convention — escalated to a separate
  protocol-expansion plan ([§7](#7-open-questions)); MVP uses sensible defaults.
- **TX flow:**
  1. `AcquireTxLease(channel)` — refuse/queue if another channel holds the rig.
  2. `Transmit(channel, payload)` → `transmit_id`.
  3. Watch `TransmitStarted` → `TransmitComplete` for that id; show a progress/state
     line and the live TX dBFS meter. Operator hears modulation on the output device.
  4. `ReleaseTxLease(channel)` when done.
  - PTT keying during TX follows the daemon contract — confirm in [§7](#7-open-questions).
- **FT8 specifics:** show the 15 s slot timer and a clock-sync indicator from
  `ClockOffset{synchronized, offset_s}` (windowed modes need an accurate clock);
  warn before TX if unsynchronized.

### 5.5 Interaction model: chat vs structured vs packet

Answering "are these chat-centric?" directly: **the keyboard modes are; FT8 isn't;
AFSK1200 can be either.** Driving the whole UI as one chat window would fit
PSK31/RTTY/CW (and AFSK1200 in converse use) but mis-model FT8 and AX.25/KISS
packet. So the Compose surface is **mode-selected** (and, for AFSK1200,
use-selected), sharing the same TX plumbing underneath:

| Shape | Modes | Surface | Why |
|---|---|---|---|
| **Chat / ragchew** | PSK31, RTTY, CW, **AFSK1200\*** | Scrollback transcript + compose line | Keyboard-to-keyboard free text (*AFSK lines ride HDLC frames today; raw async TTY needs a modem change) |
| **Structured QSO** | FT8 | Exchange-ladder sequencer + slot clock | Rigid timed protocol, ~13-char payloads, not free chat |
| **Packet / KISS** | AFSK1200 (AX.25) | `ConfigureKissListener`, not this screen | Direwolf/APRS interop, framed AX.25 |

The chat transcript is the right long-term home for the deferred RX path: today it
shows your TX lines; when decode lands, RX frames render as inbound lines in the
same view, and PSK31/RTTY/CW become true two-way chat with no layout change.

## 6. Mapping to the six platform capabilities

| # | Requirement | RPC / event |
|---|---|---|
| 1 | Enumerate audio & PTT ports | `ListDevices`; `DeviceArrived/Departed` |
| 2 | Configure RX+TX audio + PTT | `ConfigureChannel`, `ConfigureAudio`, `ConfigurePtt` |
| 3 | Expose audio levels | `AudioLevel` events; `GetMetrics`/`ChannelMetrics` |
| 4 | Adjust RX/TX levels | `SetAudioGain(rx_gain, tx_gain)` — landed in #20 |
| 5 | Send + modulate (TX) | `AcquireTxLease`, `Transmit`, `KeyPtt`, `TransmitStarted/Complete` |
| 6 | RX decode back to client | `RxFrame` events — **deferred** |

## 7. Open questions

Resolved by #20: ~~gain control~~ (`SetAudioGain` landed) and ~~split RX/TX
devices~~ (`ConfigureAudio.tx_device_id` landed). Remaining forks:

1. **Mode parameters over the wire — needs its own protocol-expansion plan.**
   `ConfigureChannel.mode` is a bare `string` ("none" in Phase 1), but Rust
   `ModeConfig` carries params (CW wpm/tone, RTTY baud/shift, PSK31 center_hz).
   Options: (a) structured mode config in the proto, (b) a mode-string grammar
   (e.g. `cw:wpm=20,tone=700`), or (c) defaults-only for MVP. Per the issue
   thread this is a protocol change tracked separately; the TUI builds against
   defaults until that plan lands.
2. **TX keying contract.** Does `Transmit` auto-assert PTT for the burst, or must
   the client `KeyPtt(true)` around it? `KeyPtt` is documented as an operator test
   "no audio" — confirm the keying ownership during a real transmit.
3. **Transparent (raw async) AFSK1200.** AFSK1200 chat works today *through HDLC
   framing* (`Afsk1200Mod` only accepts `FramePayload::Packet` and always wraps in
   HDLC). If we want classic raw Bell-202 async-serial chat (no HDLC at all), the
   modem needs a transparent-framing path (e.g. `Text` payload support for AFSK or
   a raw mode). MVP: do HDLC-framed AFSK chat suffice, or is raw async a
   requirement? — a modem-side change, not just TUI.
4. **Client home.** `clients/omnimodem-tui/` in this repo (proposed) vs a separate
   repo.
5. **Remote operation in MVP.** Local UDS only, or include the mTLS/`--addr` remote
   path from the start?

## 8. Suggested build order

1. gRPC plumbing: generate Go bindings, UDS dial, `GetState` + `SubscribeEvents`
   bridge, dashboard rendering from live state.
2. Configuration screen: device enumeration → `ConfigureChannel`/`ConfigureAudio`/
   `ConfigurePtt`, manual `KeyPtt` test, live dBFS meter, udev helper.
3. Compose & TX: mode select (defaults), composer, lease → `Transmit` → progress;
   FT8 slot timer + clock-sync gate.
4. Fast-follow: `SetAudioGain` sliders (when merged), then RX decode display.
