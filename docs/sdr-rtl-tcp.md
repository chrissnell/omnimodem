# RTL-SDR (`rtl_tcp`) input — operator's guide

omnimodem can take its receive audio straight from an **RTL-SDR dongle** served over
the network by `rtl_tcp`, instead of a soundcard. The daemon connects to the
`rtl_tcp` server, tunes the dongle, demodulates the RF to audio in software (the
"gqrx" wideband + NCO model), and feeds that audio to any mode — so you can decode
APRS off 144.390 MHz, listen to an AM airband channel, or work HF with a
direct-sampling dongle, all without a radio.

RX only: RTL dongles cannot transmit, so an SDR-bound channel is receive-only.

## 1. Start an `rtl_tcp` server on the dongle's host

`rtl_tcp` ships with `librtlsdr` (`rtl-sdr` / `rtl-sdr-blog` packages). Run it on the
machine the dongle is plugged into:

```sh
# Local dongle, listening only on localhost:
rtl_tcp

# Remote dongle (e.g. a Pi at the antenna): listen on all interfaces so the
# omnimodem host can reach it. Port 1234 is the rtl_tcp default.
rtl_tcp -a 0.0.0.0 -p 1234
```

`rtl_tcp -a 0.0.0.0` exposes the dongle to your LAN — only do this on a trusted
network (there is no authentication). Leave the sample rate / gain to omnimodem;
it re-commands the dongle on connect.

## 2. Register or bind the endpoint

An `rtl_tcp` endpoint is addressed as `rtltcp:<host>:<port>`. You can either **bind
it ad-hoc** on a channel, or **register it** in the daemon config so it shows up in
`ListDevices`.

### Ad-hoc (no config)

Point a channel's audio device at the endpoint via `ConfigureAudio` (in the TUI:
the Configure screen's device field; over gRPC: `ConfigureAudioRequest.rx_device =
"rtltcp:192.168.1.50:1234"`). No enumeration or config entry is needed.

### Registered (surfaced in `ListDevices`)

Add lines to the daemon config file (`$OMNIMODEM_RUNTIME_DIR/omnimodem.conf`, see
[`running.md`](running.md)) so remote dongles a hardware scan can't find still appear
for selection:

```text
# rtl_tcp <host:port> [optional label...]
rtl_tcp 192.168.1.50:1234 Rooftop R820T
rtl_tcp 127.0.0.1:1234
```

Malformed lines are skipped with a warning; registration is a convenience, not a
requirement.

## 3. Tune, watch the waterfall, pick a mode

Once a channel is bound to the dongle:

- **Tune** — set the absolute demod frequency (TUI tuning view, or `SetSdrTune`
  `freq_hz`). Small moves within the captured band retune instantly and losslessly
  (only the software NCO moves); a large move re-centers the dongle hardware. The
  daemon reports the frequency it actually landed on.
- **Waterfall** — a wideband, RF-referenced spectrum (`SpectrumFrame`) streams while
  the channel is bound. In the TUI you can click/step the cursor across it to tune;
  the frequency axis is real RF (hardware center ± the captured band).
- **Demod mode** — choose per channel via `ConfigureSdr.demod_mode`:
  - `DEMOD_NBFM` — narrowband FM. The default; the right choice for APRS/AFSK1200
    and 2 m/70 cm voice. Audio is flat (no de-emphasis) so AFSK twist is preserved.
  - `DEMOD_AM` — airband and other AM.
  - `DEMOD_WFM` — wideband FM (broadcast). Not de-emphasized yet.
  - `DEMOD_USB` / `DEMOD_LSB` — SSB (upper/lower sideband).

## 4. Gain, squelch, ppm, and dongle extras

All via `ConfigureSdr` / `SetSdrGain` (TUI: the SDR settings on the channel):

- **Gain** — `auto=true` engages the dongle's hardware AGC; otherwise a manual value
  is snapped to the tuner's discrete gain table (query it with `GetSdrCaps`). Start
  with auto, then set manual gain if a strong nearby signal is desensing the front end.
- **Squelch** — a power squelch (dBFS open threshold, with hysteresis). Set it to mute
  the channel between transmissions; a value `<= -200` disables it (always open).
- **ppm** — frequency correction for the dongle's crystal error. Find your dongle's
  ppm once (e.g. against a known carrier) and set it so tuning is accurate.
- **Bias-tee** — powers an inline LNA / active antenna over the coax. R820-class
  dongles only; check `GetSdrCaps.bias_tee_supported`. **Do not enable it into a
  passive antenna or a radio** — it puts DC on the coax.
- **Direct sampling (HF)** — bypasses the tuner to reach HF (below ~24 MHz, up to the
  ~28.8 MHz ADC Nyquist). Enable it for HF work on a direct-sampling-capable dongle
  (RTL-SDR Blog V3 and similar); the reported tunable range widens down to DC while
  it's on.

## 5. Robustness (what happens when things go wrong)

The `rtl_tcp` input is built to run unattended:

- **Auto-reconnect.** If the `rtl_tcp` link drops — the server restarts, the Pi
  reboots, the network blips — the daemon reconnects on its own with exponential
  backoff (100 ms → up to 5 s) and **re-applies your whole tune**: center/offset,
  gain mode and level, ppm, sample rate, bias-tee, and direct-sampling. A transient
  outage never tears down the channel or loses where you were listening; RX just
  resumes. Reconnect attempts and drops are logged (`RUST_LOG=info`).
- **Overrun handling.** If the decoder can't keep up with the incoming audio (a very
  busy machine, a slow mode), the capture **drops the oldest queued-but-undelivered
  audio and keeps reading** rather than stalling the dongle — so latency stays bounded
  and you stay near real-time instead of falling further and further behind. Dropped
  audio is counted and logged (on the first drop, then throttled to one warning per 64
  dropped chunks) so overruns are visible, not silent.
- **Multiple clients (last-writer-wins).** The tune/gain/mode state is shared per
  channel, so if two clients (say two TUI windows, or a script and the TUI) both
  control the same SDR channel, the **most recent** `SetSdrTune` / `SetSdrGain` /
  `ConfigureSdr` wins — there is no locking or ownership. Every change broadcasts an
  `SdrState` event, and a client sees the current state the moment it subscribes, so
  all clients converge on the effective tune/gain/demod-mode/squelch. If you don't
  want two operators fighting over one dongle, coordinate out of band or give each a
  separate channel/dongle.

## See also

- [`grpc-api.md`](grpc-api.md) — the SDR control RPCs (`SetSdrTune`, `SetSdrGain`,
  `ConfigureSdr`, `GetSdrCaps`) and the `SdrState` event, field by field.
- [`running.md`](running.md) — the daemon config file that registers endpoints.
- [`wiki/audio-devices-ptt.md`](wiki/audio-devices-ptt.md) — how the SDR backend fits
  the audio subsystem (for developers).
- [`design/2026-07-06-rtl-tcp-sdr-input-design.md`](design/2026-07-06-rtl-tcp-sdr-input-design.md)
  — the tuning model and design rationale.
