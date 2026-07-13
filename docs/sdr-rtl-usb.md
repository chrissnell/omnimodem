# Native (local USB) RTL-SDR input — operator's guide

omnimodem can drive an **RTL-SDR dongle plugged straight into this machine** — no
`rtl_tcp`, no `librtlsdr`, no C dependency. The daemon discovers the dongle on the
USB bus, claims its interface directly (pure-Rust `nusb`), initializes the RTL2832U
+ R820T/R828D tuner itself, and streams raw IQ, demodulating to audio in software
exactly like the `rtl_tcp` path. Bind a channel to the dongle and you can decode
APRS off 144.390 MHz, listen to airband AM, or work HF with a direct-sampling dongle
— all without a radio.

RX only: RTL dongles cannot transmit, so an SDR-bound channel is receive-only.

This is the *local USB* path. For a dongle served over the network by `rtl_tcp`
(remote Pi at the antenna, or a dongle on another host), see
[`sdr-rtl-tcp.md`](sdr-rtl-tcp.md). The tuning model, waterfall, demod modes, and
gain/squelch/ppm controls in section 3–4 below are **identical** between the two —
only discovery and per-OS setup differ.

## 1. Three ways to select a dongle

| Path | How it's addressed | When to use |
|---|---|---|
| **Auto-detected local USB** | `rtl:serial:<serial>` / `rtl:topo:<bus>-<ports>` — discovered, shown in `ListDevices` | A dongle plugged into this machine (this guide). |
| **Manual local USB** | the same `rtl:...` id, bound ad-hoc via `ConfigureAudio` | You already know the id and want to skip the picker. |
| **Remote / networked** | `rtltcp:<host>:<port>` — see [`sdr-rtl-tcp.md`](sdr-rtl-tcp.md) | The dongle is on another host, or reached over the LAN. |

### Auto-detected (the common case)

A dongle plugged into this machine is discovered automatically and appears in
`ListDevices` as an **`rtl:` device**. No config entry, no `rtl_tcp` server. Pick it
in the TUI's Configure screen and bind — done. Hotplug is live: plug the dongle in
while the daemon runs and it appears; unplug it and it departs.

The id is **stable across reboots and USB ports**: a dongle with a unique USB serial
is keyed `rtl:serial:<serial>` (survives moving to a different port); a dongle with
no serial, or a duplicate serial that collides with another dongle, falls back to
its bus topology `rtl:topo:<bus>-<ports>` (stable as long as it stays in the same
port). Either way the channel binding follows the identity, not a `/dev` path.

> **`needs_setup`.** If a dongle is present but the OS hasn't been prepared to let
> the daemon claim it, it still appears in `ListDevices` with **`needs_setup`** set,
> so the TUI can tell you *what* to fix instead of the dongle silently failing to
> open. See section 2 for the one-time per-OS step that clears it.

### Manual (bind by id, no picker)

Any `rtl:` id can be bound ad-hoc via `ConfigureAudio`
(`ConfigureAudioRequest.rx_device = "rtl:serial:00000001"`) — the same way an
`rtltcp:` endpoint is bound. Useful in scripts, or when you already know the id.

### Remote

A dongle on another host is **not** a local USB device — run `rtl_tcp` on that host
and bind `rtltcp:<host>:<port>`. That path (including registering endpoints in the
daemon config so they show up in `ListDevices`) is documented in
[`sdr-rtl-tcp.md`](sdr-rtl-tcp.md).

## 2. Per-OS setup (one time)

Each OS needs a one-time permission or driver step before the daemon can claim the
raw USB interface. Until it's done, the dongle shows up with `needs_setup` set. The
full commands live in [`running.md`](running.md#native-local-usb-rtl-sdr-dongles);
the short version:

- **Linux** — install the bundled udev rule for permissions, and blacklist the
  kernel DVB-T driver (`dvb_usb_rtl28xxu`) so it doesn't grab the dongle first:

  ```sh
  sudo cp packaging/udev/99-omnimodem-rtlsdr.rules /etc/udev/rules.d/
  sudo udevadm control --reload-rules && sudo udevadm trigger
  # /etc/modprobe.d/blacklist-omnimodem-rtlsdr.conf:  blacklist dvb_usb_rtl28xxu
  ```

  Then unplug/re-plug (and `sudo rmmod dvb_usb_rtl28xxu`, or reboot). A `needs_setup`
  or claim error on Linux is almost always the DVB driver.

- **Windows** — bind a generic **WinUSB** driver to the dongle once with
  [Zadig](https://zadig.akeo.ie/) (*Options → List All Devices*, pick the RTL
  `Bulk-In, Interface (Interface 0)`, install **WinUSB**). Until then the device is
  listed with `needs_setup`. Zadig is per physical port — repeat it if you move the
  dongle to a different port.

- **macOS** — nothing to do. No in-kernel DVB driver competes for the dongle, so the
  daemon claims interface 0 on plug-in and it's immediately bindable. If a claim ever
  fails, close any other SDR app (SDR++, CubicSDR, GQRX/SoapySDR) that may hold it.

## 3. Tune, watch the waterfall, pick a mode

Once a channel is bound to the dongle, everything below behaves exactly as the
`rtl_tcp` path — the DSP and control surface are shared.

- **Tune** — set the absolute demod frequency (TUI tuning view, or `SetSdrTune`
  `freq_hz`). Small moves within the captured band retune instantly and losslessly
  (only the software NCO moves); a large move re-centers the dongle hardware. The
  daemon reports the frequency it actually landed on.
- **Waterfall** — a wideband, RF-referenced spectrum (`SpectrumFrame`) streams while
  the channel is bound; the frequency axis is real RF (hardware center ± the captured
  band). In the TUI you can step the cursor across it to tune.
- **Demod mode** — choose per channel via `ConfigureSdr.demod_mode`:
  - `DEMOD_NBFM` — narrowband FM. The default; the right choice for APRS/AFSK1200 and
    2 m/70 cm voice. Audio is flat (no de-emphasis) so AFSK twist is preserved.
  - `DEMOD_AM` — airband and other AM.
  - `DEMOD_WFM` — wideband FM (broadcast). Not de-emphasized yet.
  - `DEMOD_USB` / `DEMOD_LSB` — SSB (upper/lower sideband).

## 4. Gain, squelch, ppm, and dongle extras

All via `ConfigureSdr` / `SetSdrGain` (TUI: the SDR settings on the channel) —
identical to the `rtl_tcp` path:

- **Gain** — `auto=true` engages the tuner's AGC; a manual value is snapped to the
  tuner's discrete gain table (query it with `GetSdrCaps`). Start with auto, set
  manual gain if a strong nearby signal is desensing the front end.
- **Squelch** — a power squelch (dBFS open threshold, with hysteresis). A value
  `<= -200` disables it (always open).
- **ppm** — crystal-error correction. Find your dongle's ppm once (against a known
  carrier) and set it so tuning is accurate.
- **Bias-tee** — powers an inline LNA / active antenna over the coax. R820-class
  dongles only; check `GetSdrCaps.bias_tee_supported`. **Do not enable it into a
  passive antenna or a radio** — it puts DC on the coax.
- **Direct sampling (HF)** — bypasses the tuner to reach HF (below ~24 MHz, up to the
  ~28.8 MHz ADC Nyquist). Enable it for HF on a direct-sampling-capable dongle
  (RTL-SDR Blog V3 and similar); the reported tunable range widens down to DC.

## 5. Robustness — mid-capture removal and overrun

The native USB path shares the pipeline's drop-oldest overrun handling with
`rtl_tcp`, but differs on one point: **removal is terminal, not reconnecting.**

- **Unplug = terminal stop.** A locally-attached dongle has no network to blip — if
  it's pulled mid-capture, the USB transfer fails (`NO_DEVICE`) and the capture stops
  cleanly: the channel unbinds and hotplug reports the device `Departed`. There is
  no auto-reconnect (unlike `rtl_tcp`, where a link drop is expected and retried) —
  re-plug the dongle and re-bind the channel. The removal is logged.
- **Overrun = drop-oldest.** If the decoder can't keep up (a busy machine, a slow
  mode), the capture drops the oldest queued-but-undelivered audio and keeps reading
  rather than stalling the transfer queue, so latency stays bounded. Dropped audio is
  counted and logged (first drop, then throttled).
- **Multiple clients (last-writer-wins).** Tune/gain/mode state is shared per channel;
  the most recent `SetSdrTune` / `SetSdrGain` / `ConfigureSdr` wins, and every change
  broadcasts an `SdrState` event so all clients converge. Coordinate out of band if
  you don't want two operators fighting over one dongle.

## 6. Bring-up checklist (plug in → decode APRS)

A concrete first-light sequence for an auto-detected local dongle. If any step
stalls, the *Fix* column is the usual cause.

| # | Step | Expect | If not — fix |
|---|---|---|---|
| 1 | Do the one-time per-OS setup (section 2) | — | Skip only if already done for this port. |
| 2 | Plug the dongle into this machine | Kernel/OS enumerates it | Try another port/cable; avoid unpowered hubs. |
| 3 | `ListDevices` (TUI Configure screen, or gRPC) | An `rtl:serial:...` / `rtl:topo:...` entry appears | Not listed → not enumerated (step 2); listed with **`needs_setup`** → redo section 2 for this OS/port. |
| 4 | Bind a channel to that `rtl:` device | Channel opens; waterfall starts streaming | Claim/`needs_setup` error → Linux: DVB driver still bound (`rmmod dvb_usb_rtl28xxu`); Windows: WinUSB not installed (Zadig); macOS: another SDR app holds it. |
| 5 | Set demod mode `DEMOD_NBFM`, tune **144.390 MHz** (US 2 m APRS) | Waterfall centers on 144.390; NBFM audio | Wrong region → use your local APRS frequency. Off-frequency → set the dongle's **ppm**. |
| 6 | Set the mode to **AFSK1200 / APRS** and watch for decodes | Packets decode as stations key up | No decodes on a live channel → nudge **gain** (start `auto`, then manual if a strong signal desenses); confirm antenna + squelch (`<= -200` to force open). |

Once step 6 decodes, the dongle is fully working — switch modes/frequencies freely;
tune stays put across mode changes.

## See also

- [`running.md`](running.md#native-local-usb-rtl-sdr-dongles) — the exact per-OS
  setup commands (udev rule, DVB blacklist, Zadig) and daemon environment knobs.
- [`sdr-rtl-tcp.md`](sdr-rtl-tcp.md) — the networked `rtl_tcp` path; same tuning /
  waterfall / demod model, different discovery.
- [`grpc-api.md`](grpc-api.md) — the SDR control RPCs (`SetSdrTune`, `SetSdrGain`,
  `ConfigureSdr`, `GetSdrCaps`) and the `SdrState` event, field by field.
- [`wiki/audio-devices-ptt.md`](wiki/audio-devices-ptt.md) — how the native USB
  backend fits the audio subsystem (for developers).
- [`design/2026-07-12-native-rtl-sdr-design.md`](design/2026-07-12-native-rtl-sdr-design.md)
  — discovery, USB claim, and the RTL2832U/R82xx bring-up design rationale.
