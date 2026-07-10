# Omnimodem TUI Client — Design

> Status: **Design / RFC.** Targets the existing `omnimodem.v1` gRPC API.
> Scope: configuration + digital-mode **TX**. RX (decode display) is deferred, but
> the layout reserves its place so it drops in without rework.

A terminal client for `omnimodem`, built with [Bubble Tea](https://github.com/charmbracelet/bubbletea).
It lets an operator enumerate audio/PTT hardware, bind a channel, pick a digital
mode, and send a message — hearing the modulated signal on the output device.

The bar is **usable, not exhaustive**: enough of what real operators expect from
WSJT-X / fldigi / JS8Call that someone would actually reach for it, without
chasing every feature those mature apps carry. §5 is a research spike that sets
that bar; §6 is the layout it produces.

## 1. Goals & non-goals

**Goals (MVP):**
1. Connect to a local `omnimodem` over its gRPC control plane.
2. Enumerate audio + PTT devices; select RX/TX audio and a PTT device + method.
3. Bind a channel (`ConfigureChannel` + `ConfigureAudio` + `ConfigurePtt`).
4. Show live audio levels (dBFS) and PTT state from the event stream.
5. Select a digital mode and compose + transmit a message.
6. Surface TX progress (started/complete), TX lease, and clock-sync for FT8.
7. **Feel like a real digital-mode app** — macros, a persistent status bar, an
   activity pane, and a hard TX abort (see §5).

**Non-goals (deferred / out of scope):**
- RX decode display (`RxFrame` rendering) — comes after TX is solid; its panes are
  scaffolded now and sit empty until the decode path lands.
- Rig CAT/VFO control — omnimodem is audio + PTT; the proto has no rig frequency, so
  "tuning" means the audio offset within the passband, not a VFO.
- DX cluster / PSKReporter / online spotting, contest logging, ADIF, maps.

> **Update (after #20):** runtime gain (`SetAudioGain`) and split RX/TX device
> binding (`ConfigureAudio.tx_device_id`) both landed, so the levels panel and
> separate RX/TX device selection are in scope — see [§6.3](#63-configuration-per-channel).

## 2. Where it lives

`omnimodem` is a Rust workspace; the TUI is Go. Proposal: a self-contained Go
module at **`clients/omnimodem-tui/`** in this repo, generating bindings from
`proto/omnimodem.proto` via `protoc-gen-go` + `protoc-gen-go-grpc`. One repo keeps
the proto and its first reference frontend versioned together. (Alternative: a
separate repo — see [§8](#8-open-questions).)

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

Components: `bubbles/list` (devices, modes, activity roster),
`textinput`/`textarea` (composer & PTT fields), `table` (dashboard, band activity),
`progress` (dBFS meter, FT8 slot timer), `key`/`help` (keymap + macro bar),
`lipgloss` for layout.

## 5. UI/UX research spike — lessons from the established apps

What do operators actually expect from a digital-mode app, and how is it organized?
A survey of the dominant packages (and the proof that terminal clients are real):

| App | Modes | Core layout | Standout UX |
|---|---|---|---|
| **WSJT-X** | FT8/FT4/JT65/Q65/WSPR | mode buttons; **Band Activity** + **Rx Frequency** decode panes; **Tx1–Tx6** message panel; separate **waterfall**; Pwr slider | **auto-sequencing** QSO state machine; double-click a decode to start a QSO; big **Halt Tx**; **Tx watchdog** timeout; sending `73` opens the **log** window |
| **fldigi** | PSK31/RTTY/CW/Olivia/Hell/… | click-to-tune **waterfall**; **RX text** pane over **TX text** pane; **macro bar** (F1–F12 × 4 banks = 48 macros); inline log fields | macro quick-keys (CQ / my-call / brag / 73); already-sent TX text colorized; pick a signal off the waterfall |
| **JS8Call** | JS8 (keyboard chat over FT8) | **Band Activity** + **Call Activity** (heard-station roster) + directed-message pane + **compose box** + waterfall | select a callsign → **directed message** (callsign auto-prefixed); click a station to reply; **heartbeat** automation |
| **twpsk / psk31lx** | PSK31 | ncurses RX/TX split + tuning indicator | proof that terminal-native digital clients exist and get used |

**Distilled "table-stakes for usable"** — the patterns common to all of them:

1. **Spectrum/waterfall awareness** — see *where* signals are and tune to them. The
   single most iconic element of every GUI app.
2. **Split RX-over-TX text** — the conversation transcript for keyboard modes.
3. **Macros / quick-send** — one keystroke for canned messages (CQ, my call, RST,
   73). fldigi's F-keys; the biggest ergonomic multiplier and cheap to build.
4. **Activity / station roster** — recently-heard stations, click to reply
   (JS8Call Call Activity, WSJT-X Band Activity).
5. **Auto-sequence + one-key abort** — for the rigid FT8 exchange (Tx1–Tx6), plus a
   prominent **Halt Tx** and a **watchdog** that stops runaway transmits.
6. **At-a-glance status** — mode, frequency/offset, PTT, clock-sync, signal level,
   always visible in one bar.
7. **Lightweight logging** — record call / RST / grid / time; `73` triggers it.
8. **Signal report (RST/SNR)** front-and-center — the unit of a QSO.

**Adopt (in scope — these exercise omnimodem usefully and read as a real app):**
- **Macro bar** of quick-send messages wired to `Transmit` (CQ, my-call, RST, 73,
  brag); user-editable text, function-key bound.
- **Persistent status bar**: channel, mode + offset, PTT state, clock-sync, live
  RX/TX dBFS — driven by the event stream.
- **Activity/roster pane + chat transcript** laid out now; transcript shows TX today
  and gains inbound RX lines for free when decode lands; roster populates from
  `RxFrame` later.
- **FT8 auto-sequence ladder** (Tx1–Tx6) with **Halt Tx** and a **Tx watchdog**.
- **Mini QSO log** — local append-only (call/RST/grid/UTC); `73`/RR73 prompts a log.
- **Big TX-active banner + single abort key** everywhere TX can run.

**Deliberately skip (scope discipline — "exercise omnimodem", not clone WSJT-X):**
- Rig CAT control (no rig frequency in the proto), DX cluster / PSKReporter,
  contest/ADIF logging, mapping/GridTracker, multi-decoder super-waterfalls.

**Gaps this surfaced** (carried into [§8](#8-open-questions)):
- **No spectrum/FFT in the API.** `AudioLevel` is a single dBFS scalar — enough for a
  level meter, *not* a waterfall. A real spectrum strip needs a new FFT/spectrum
  event from the daemon, or we ship a level-meter-only view. This is the one place
  the "usable" bar bumps the protocol.
- **Tuning model.** With no VFO in the proto, "tuning" is the audio offset where the
  mode sits in the passband (e.g. PSK31 center, FT8 sub-band) — worth confirming the
  mental model matches operator expectation.
- **RX-dependent features inert until decode lands** — roster, inbound chat, RST-from-SNR.

## 6. Screens

### 6.1 Connection
Pick/confirm socket path or routable addr → connect → show daemon version & state.

### 6.2 Dashboard (home)
Table of channels: id, name, mode, bound device, PTT state, live dBFS bar, TX-lease
holder. Live from the event stream. Keys to jump into Config or Operate.

### 6.3 Configuration (per channel)
- **Device enumeration:** `ListDevices` → split into capture-capable (RX) and
  playback-capable (TX) views from `DeviceInfo{has_capture,has_playback}`. A typical
  USB rig interface is one device exposing both (pick it once); since #20 a split rig
  can bind a separate playback card via `tx_device_id`. The screen offers an **RX
  capture device**, an **optional separate TX playback device** (defaults to RX), and
  a **PTT device**. Hotplug (`DeviceArrived`/`Departed`) updates the lists live.
- **Audio bind:** `ConfigureAudio(channel, device_id, sample_rate=48000, fanout,
  tx_device_id?, tx_sample_rate?)`; show the returned `actual_sample_rate`.
- **PTT bind:** `ConfigurePtt(channel, device_id, method, node, pin_or_line, invert)`
  with a method picker (NONE/VOX/SERIAL_RTS/SERIAL_DTR/CM108/GPIO); pin/line/invert
  gated by method. `KeyPtt` gives a manual key/unkey test.
- **Permissions helper:** when a bind needs udev access (CM108/serial/GPIO), offer
  `SuggestUdevRule(device_id)` and show the rule + install instructions (daemon never
  writes it).
- **Levels:** live dBFS meter from `AudioLevel`, with **RX/TX gain sliders** wired to
  `SetAudioGain(channel, rx_gain, tx_gain)` (linear, 1.0 = unity; landed in #20).

### 6.4 Operate (per channel) — the main screen

Mode-selected compose surface (see [§7](#7-interaction-model-chat-vs-structured-vs-packet))
over shared TX plumbing, wrapped in the always-on chrome the research calls for: a
**status bar**, an **activity pane**, and a **macro bar**.

**Ragchew layout (PSK31 / RTTY / CW / AFSK1200-converse):**

```
┌ omnimodem · ch1 ▸ PSK31 @ 1000Hz ······· clk ✓ · PTT ▢ · RX −18 TX −− dBFS ┐
│ Activity (heard)      │ Transcript                                          │
│ ▸ W1AW   −12  2m       │ 12:03 ‹ CQ CQ de W1AW W1AW K                        │
│   K0PIR  −08  5m       │ 12:04 › W1AW de NW5W NW5W                           │
│   …  (RX-era)          │ 12:04 ‹ NW5W de W1AW ur 599 …  (RX-era)            │
│                        │ ▁▁▂▃▅▇ level / spectrum strip                       │
├────────────────────────┴───────────────────────────────────────────────────┤
│ › compose…________________________________________________________  [↵ send]│
├ macros ──────────────────────────────────────────────────────────────────── │
│ F1 CQ  F2 Call  F3 RST  F4 73  F5 Brag  …            [T] key  [Esc] HALT TX  │
└──────────────────────────────────────────────────────────────────────────── ┘
```

**Structured layout (FT8):**

```
┌ omnimodem · ch1 ▸ FT8 · slot ███▁▁ 07/15s · clk ✓ +0.02s · TX −6 dBFS ──────┐
│ Band activity          │ QSO sequencer                                        │
│ 00:15 −10 CQ W1AW FN31  │  DX [W1AW]  grid [FN31]   my [NW5W EM10]  RST −10/+02│
│ 00:30 −08 K0PIR EM12    │  ┌ auto-sequence ──────────────────────────────────┐│
│ …                       │  │ ▸ Tx1  W1AW NW5W EM10                            ││
│                         │  │   Tx2  W1AW NW5W −10                             ││
│                         │  │   Tx3  W1AW NW5W R−08                            ││
│                         │  │   Tx4  W1AW NW5W RR73                            ││
│                         │  │   Tx5  W1AW NW5W 73                              ││
│                         │  └──────────────────────────────────────────────────┘│
│                         │  auto ☑   [↵] Enable Tx   [H] HALT TX   73→log       │
└─────────────────────────┴──────────────────────────────────────────────────── ┘
```

(Wireframes are illustrative; double-lines = focus. `‹`=RX, `›`=TX.)

- **Mode select:** picks the surface. Mode params (CW wpm/tone, RTTY baud/shift,
  PSK31 center) need a wire convention — separate protocol-expansion plan
  ([§8](#8-open-questions)); MVP uses sensible defaults.
- **TX flow:** `AcquireTxLease` → `Transmit(channel, payload)` → watch
  `TransmitStarted`/`TransmitComplete` (TX banner + live dBFS; operator hears the
  modulation) → `ReleaseTxLease`. **Halt** aborts; a **watchdog** stops TX if it
  runs past a configured ceiling. PTT keying during TX follows the daemon contract
  ([§8](#8-open-questions)).
- **FT8 specifics:** the slot clock and `ClockOffset{synchronized,offset_s}` ride in
  the status bar; warn before TX if unsynchronized. Auto-sequence advances Tx1→…→73;
  reaching 73/RR73 prompts a log entry.

## 7. Interaction model: chat vs structured vs packet

**The keyboard modes are chat; FT8 isn't; AFSK1200 can be either.** Driving the whole
UI as one chat window would fit PSK31/RTTY/CW (and AFSK1200 in converse use) but
mis-model FT8 and AX.25/KISS packet. Hence the mode-selected surfaces above:

| Shape | Modes | Surface | Why |
|---|---|---|---|
| **Chat / ragchew** | PSK31, RTTY, CW, **AFSK1200\*** | transcript + compose + macros | keyboard-to-keyboard free text (*AFSK rides HDLC frames today; raw async TTY needs a modem change) |
| **Structured QSO** | FT8 | auto-sequence ladder + slot clock | rigid timed protocol, ~13-char payloads, not free chat |
| **Packet / KISS** | AFSK1200 (AX.25) | `ConfigureKissListener`, not this screen | Direwolf/APRS interop, framed AX.25 |

AFSK1200 is just Bell-202 modulation; omnimodem's modulator HDLC-frames any bytes
(`Afsk1200Mod` → preamble + `hdlc_frame` + FCS), which is *not* AX.25-specific, so
the chat surface backs it today via raw `Transmit`. Truly raw async AFSK (no HDLC)
would need a transparent-framing path in the modem ([§8](#8-open-questions)).

## 8. Mapping to the six platform capabilities

| # | Requirement | RPC / event |
|---|---|---|
| 1 | Enumerate audio & PTT ports | `ListDevices`; `DeviceArrived/Departed` |
| 2 | Configure RX+TX audio + PTT | `ConfigureChannel`, `ConfigureAudio`, `ConfigurePtt` |
| 3 | Expose audio levels | `AudioLevel` events; `GetMetrics`/`ChannelMetrics` |
| 4 | Adjust RX/TX levels | `SetAudioGain(rx_gain, tx_gain)` — landed in #20 |
| 5 | Send + modulate (TX) | `AcquireTxLease`, `Transmit`, `KeyPtt`, `TransmitStarted/Complete` |
| 6 | RX decode back to client | `RxFrame` events — **deferred** |

## 9. Open questions

Resolved by #20: ~~gain control~~ and ~~split RX/TX devices~~. Remaining forks:

1. **Spectrum/waterfall feed (new, from §5).** The "usable" bar wants signal-position
   awareness, but `AudioLevel` is a single dBFS scalar. Add an FFT/spectrum event to
   the proto (small, additive) for a real waterfall, or ship a level-meter-only strip
   for MVP? The wireframes assume the latter as a placeholder.
2. **Mode parameters over the wire — its own protocol-expansion plan.**
   `ConfigureChannel.mode` is a bare string; Rust `ModeConfig` carries params (CW
   wpm/tone, RTTY baud/shift, PSK31 center_hz). Structured proto config, a mode-string
   grammar, or defaults-only for MVP? Tracked separately; TUI builds against defaults.
3. **TX keying contract.** Does `Transmit` auto-assert PTT for the burst, or must the
   client `KeyPtt(true)` around it? `KeyPtt` is documented "no audio" — confirm
   keying ownership during a real transmit.
4. **Transparent (raw async) AFSK1200.** Chat works today through HDLC framing; raw
   Bell-202 async (no HDLC) needs a transparent-framing path in the modem. Is
   HDLC-framed AFSK chat enough for MVP?
5. **Client home.** `clients/omnimodem-tui/` in this repo (proposed) vs a separate repo.
6. **Remote operation in MVP.** Local UDS only, or include the mTLS/`--addr` path from
   the start?

## 10. Suggested build order

1. gRPC plumbing: generate Go bindings, UDS dial, `GetState` + `SubscribeEvents`
   bridge; dashboard + **status bar** rendering from live state.
2. Configuration screen: device enumeration → `ConfigureChannel`/`ConfigureAudio`/
   `ConfigurePtt`, manual `KeyPtt` test, dBFS meter + gain sliders, udev helper.
3. Operate screen — ragchew first: transcript + **compose** + **macro bar** → lease →
   `Transmit` → TX banner/progress; **Halt** + watchdog; activity pane scaffolded.
4. Operate screen — FT8: auto-sequence ladder, slot clock + clock-sync gate, mini-log.
5. Fast-follow: spectrum strip (pending the feed decision), then RX decode display
   lighting up the transcript + roster.

---

### Sources (UI/UX research spike)

- [WSJT-X User Guide](https://wsjt.sourceforge.io/wsjtx-doc/wsjtx-main-2.6.1.html) — main window, Band Activity / Rx Frequency, Tx1–Tx6, auto-sequence, Halt Tx, watchdog, 73→log.
- [Beginners' Guide to Fldigi (W1HKJ)](http://www.w1hkj.com/beginners.html) and [fldigi beginners wiki](https://sourceforge.net/p/fldigi/wiki/beginners/) — waterfall, RX/TX text panes, F1–F12 macro banks.
- [JS8Call — overview for new users (M0IAX)](https://m0iax.com/2018/11/13/js8call-an-overview-for-new-users/) and [JS8Call User Guide](http://js8call.com/downloads/JS8Call_User_Guide.pdf) — Band/Call Activity, directed messages, heartbeat.
- [twpsk (Debian)](https://packages.debian.org/sid/twpsk) and psk31lx — terminal/ncurses digital-mode precedent.
