# Omnimodem — RTL-SDR (`rtl_tcp`) Audio Input, Tuning & Waterfall

**Date:** 2026-07-06
**Status:** Design approved in principle; phased delivery below. No code yet — this
doc is the source of truth for what lands and in what order.

## Summary & goals

Add an **`rtl_tcp` audio input source** so omnimodem can receive directly from an
RTL-SDR dongle — local or remote — with **no external glue** (no gqrx, `rtl_fm`,
or `socat`). omnimodem connects to an `rtl_tcp` server over the network, tunes the
dongle, demodulates the raw IQ down to audio, and feeds that audio to any existing
mode (AFSK1200 for APRS first, but the source is mode-agnostic).

Because omnimodem is a **reusable, gRPC-driven modem** that other software drives,
all SDR control (tuning, gain, demod mode, squelch, device capabilities) is exposed
as a first-class gRPC surface, and a **wideband waterfall** is streamed so any
frontend — including our TUI — can offer click/step tuning.

Goals:

1. **Receive from a bare `rtl_tcp` server**, local or remote, over TCP.
2. **Runtime tuning** (change frequency without restarting the channel) exposed over gRPC.
3. **Selectable demod mode** (NBFM first; AM/WFM/SSB designed in from day one).
4. **Wideband waterfall** as a tuning aid, reusing the existing `SpectrumFrame` stream.
5. **Complete, documented, reusable SDR control API** — every capability another
   frontend would reasonably want is either shipped or explicitly phased and tracked.

## Non-goals

- TX via SDR (RTL dongles are RX-only). SDR PTT/transmit is out of scope.
- SoapySDR / HackRF / Airspy device support. The wire protocol here is specifically
  `rtl_tcp`. The backend is written so a future SoapySDR backend can reuse the same
  gRPC surface, but only `rtl_tcp` is implemented.
- DSP-quality WFM stereo, RDS, or decoding of non-amateur signals.

## Background: what `rtl_tcp` is, and how remote devices connect

`rtl_tcp` (ships with `librtlsdr`) is a tiny TCP server that exposes a USB RTL-SDR
dongle over the network. Someone runs, on the machine with the dongle:

```
rtl_tcp -a 0.0.0.0 -p 1234        # listen on all interfaces, port 1234
```

A client (omnimodem) then:

1. Opens a TCP connection to `host:1234`.
2. Reads a **12-byte header**: magic `"RTL0"` + tuner type (u32 BE) + tuner gain
   count (u32 BE). This tells us the tuner chip (R820T, E4000, …) and its gain table.
3. Streams **raw 8-bit unsigned IQ**: interleaved `I0 Q0 I1 Q1 …`, each byte
   0–255 centered at 127.5. This is *not* audio — it is complex baseband the client
   must demodulate.
4. Sends **5-byte commands back** on the same socket to control the dongle:
   `0x01` set center frequency (Hz), `0x02` set sample rate, `0x03` set gain mode
   (auto/manual), `0x04` set tuner gain (tenths of dB), `0x05` set freq correction
   (ppm), plus bias-tee / direct-sampling / AGC commands. Each is a 1-byte opcode +
   4-byte big-endian argument.

**How omnimodem connects to a remote RTL device — concretely.** The `rtl_tcp`
endpoint *is* the audio device. Its `device_id` string is `rtltcp:<host>:<port>`
(e.g. `rtltcp:192.168.1.50:1234`), and it is bound to a channel exactly like a
sound card:

```
ConfigureAudio{ channel: 0, device_id: "rtltcp:192.168.1.50:1234", sample_rate: 48000 }
```

The daemon parses that `device_id`, constructs an `RtlTcpBackend { host, port }`,
opens the TCP connection on capture start, and the channel now receives off the
remote dongle. A local dongle is just `rtltcp:127.0.0.1:1234`. RF parameters
(frequency, gain, demod mode, …) are set separately via the SDR control RPCs
below — `ConfigureAudio` only establishes the transport binding.

`ListDevices` additionally surfaces any `rtl_tcp` endpoints listed in the daemon
config file so a frontend can present them for selection, but a user can always
bind an ad-hoc `rtltcp:host:port` directly without pre-registration.

## Architecture

### The IQ → audio pipeline lives inside the backend

The new source implements the **existing `AudioBackend` trait** and hands the modem
ordinary mono `i16` audio at ≤48 kHz — exactly like the cpal soundcard backend.
Nothing downstream of the audio seam changes; every existing mode, the resampler,
the audio-passband spectrum tap, gain control, etc. all work unmodified. The entire
IQ→audio chain is internal to the backend:

```
rtl_tcp socket ──▶ u8 IQ ──▶ complex baseband (Cplx)
   │                              │
   │                         ┌────┴─────────────────────────────┐
   │                         ▼                                   ▼
   │              [A] wideband IQ FFT               [B] channel select (NCO offset)
   │                    → RF waterfall                     → low-pass + decimate
   │                    (SpectrumFrame, RF Hz)             → demod (NFM/AM/WFM/SSB)
   │                                                       → squelch gate
   │                                                       → resample to mode rate
   └───────────────────────────────────────────────────── → i16 mono AudioChunk ─▶ modem
```

### The "gqrx model" and what an NCO is

An RTL dongle **cannot retune instantly** (each hardware retune has settling
latency and can miss packets), but it captures a **wide** slice of spectrum at once
— e.g. 240 kHz, or up to ~2.4 MHz. So we do what gqrx / SDR# do: tune the hardware
to a **center frequency** once, then select the exact signal we want *inside* that
captured band digitally, which is instant and lossless.

That digital channel-selection is done with an **NCO** — a *Numerically Controlled
Oscillator*. Plainly: it is a software sine/cosine generator running at a chosen
frequency. To "move" a signal that sits at, say, +30 kHz above the dongle's center
down to 0 Hz (so we can filter and demodulate it), we multiply the incoming IQ by
the NCO's wave at −30 kHz. That shift-to-zero is called *mixing* or
*down-conversion*. omnimodem's DSP crate already has this block
(`crates/dsp/src/frontend/nco.rs`, `DownConverter` with a `retune()` method) — today
it takes a *real* input; we add a complex-input variant (`push_cplx`) for IQ. The
"waterfall cursor" the user drags is simply retuning this NCO — no packet loss, no
hardware round-trip.

**Effective demod frequency = hardware center + NCO offset.** A frontend just asks
for an absolute frequency via `SetSdrTune{freq_hz}`; the daemon decides whether the
target is already inside the captured band (retune only the NCO — instant) or
outside it (retune the hardware center, then re-zero the NCO). This split is hidden
from callers.

### Demod modes (selectable, first-class)

Demodulation happens on the NCO-selected, decimated complex channel. The demod mode
is an SDR-source parameter (`ConfigureSdr.demod_mode`), settable at runtime:

| Mode | Method | Phase |
|---|---|---|
| **NBFM** | Quadrature/discriminator (`arg(x·conj(x_prev))`) + de-emphasis + audio LPF | **v1 (Phase A)** |
| **AM** | Envelope (`|x|`) + DC block | Phase B |
| **WFM** | Wideband quadrature demod (wider channel filter, no de-emphasis by default) | Phase B |
| **USB / LSB (SSB)** | Weaver / phasing SSB (real part of frequency-shifted channel) | Phase B |

The FM discriminator already exists in `crates/dsp/src/frontend/detector.rs`; AM/WFM/SSB
are added as sibling demodulators behind a small `enum DemodMode` dispatch. The gRPC
enum ships complete in v1 so external clients can code against it; unimplemented modes
return `UNIMPLEMENTED` until their phase lands.

### Squelch (v1)

A **power squelch** gates audio when the selected channel's power sits below a
threshold, so an idle APRS frequency feeds silence to the demod instead of hiss (and
DCD/metrics stay clean). Threshold is in dBFS via `ConfigureSdr.squelch_db`; a
sentinel disables it. Implemented as a simple windowed-power comparator on the
post-decimation channel, with hysteresis to avoid chatter.

### Wideband waterfall (the tuning aid)

The tuning waterfall is a **complex FFT of the raw IQ** across the whole captured
band — this is what lets the operator *see* signals and tune onto them. It reuses the
existing `SpectrumFrame` event and `SubscribeEvents` stream verbatim; the only change
is semantic: `freq_start_hz` / `freq_step_hz` are populated with **absolute RF Hz**
(bin[0] = hardware_center − rate/2) instead of audio-passband Hz. Existing renderers,
including the TUI waterfall, already label their X axis from those two fields, so they
display an RF-referenced waterfall with no change.

Because the current spectrum tap computes a *real, half* spectrum from audio, we add
an **IQ (complex, full) spectrum tap** in the RX path when the bound source is an SDR.
`ConfigureSpectrum` gains an implicit "this channel is RF" behavior driven by the
source type; its span-window fields (`freq_lo_hz`/`freq_hi_hz`) become an RF zoom
window. No breaking proto change — additive semantics only.

## gRPC API additions

All additions are **additive and back-compatible** (new RPCs + new messages; existing
messages unchanged). New RPCs on `ModemControl`:

```protobuf
// Point-and-shoot: absolute demod frequency. Daemon splits into hardware
// center + NCO offset automatically.
rpc SetSdrTune(SetSdrTuneRequest) returns (SetSdrTuneResponse);
message SetSdrTuneRequest  { uint32 channel = 1; double freq_hz = 2; }
message SetSdrTuneResponse { double actual_freq_hz = 1; double center_hz = 2; double offset_hz = 3; }

// Gain: auto AGC, or manual from the tuner's discrete gain table.
rpc SetSdrGain(SetSdrGainRequest) returns (SetSdrGainResponse);
message SetSdrGainRequest  { uint32 channel = 1; bool auto = 2; float gain_db = 3; }
message SetSdrGainResponse { float actual_gain_db = 1; }

// Source-wide SDR config: capture rate/bandwidth, demod mode, squelch,
// ppm correction, bias-tee, direct-sampling. All optional (sentinels = "unchanged").
rpc ConfigureSdr(ConfigureSdrRequest) returns (ConfigureSdrResponse);
enum DemodMode { DEMOD_NBFM = 0; DEMOD_AM = 1; DEMOD_WFM = 2; DEMOD_USB = 3; DEMOD_LSB = 4; }
message ConfigureSdrRequest {
  uint32    channel        = 1;
  uint32    capture_rate   = 2;  // dongle sample rate (Hz); 0 = unchanged
  DemodMode demod_mode     = 3;
  float     squelch_db     = 4;  // dBFS; <= -200 disables squelch
  int32     ppm            = 5;  // freq correction; sentinel keeps current
  bool      bias_tee       = 6;  // Phase C
  bool      direct_sampling= 7;  // Phase C
}
message ConfigureSdrResponse { uint32 actual_capture_rate = 1; }

// Capabilities: what this tuner/endpoint can do (for building UIs + validation).
rpc GetSdrCaps(GetSdrCapsRequest) returns (GetSdrCapsResponse);
message GetSdrCapsRequest  { uint32 channel = 1; }
message GetSdrCapsResponse {
  string        tuner        = 1;   // "R820T", "E4000", ...
  double        freq_min_hz  = 2;
  double        freq_max_hz  = 3;
  repeated uint32 sample_rates = 4; // valid capture rates
  repeated float  gains_db     = 5; // discrete tuner gain table
  bool          bias_tee_supported       = 6;
  bool          direct_sampling_supported = 7;
}
```

Tuning/gain state is also reported via `SubscribeEvents` (a small `SdrState` event)
so late-joining frontends and multiple clients stay in sync — matching how gain and
spectrum are already broadcast.

## TUI (`clients/omnimodem-tui`)

Extend the existing waterfall view into a tuning view when the bound source is an SDR:

- **RF frequency readout** (large, current effective demod freq).
- **Step tuning** with arrow keys (configurable step: 1 k / 5 k / 12.5 k / 25 k),
  **direct entry** (type a frequency), and a **movable demod-channel cursor** drawn
  over the RF waterfall showing where within the captured band we're listening.
- **Gain** control (auto toggle + manual step through the tuner's gain table from
  `GetSdrCaps`), **ppm**, **demod-mode** picker, and a **squelch** level with an
  open/closed indicator and a live signal-level meter.
- Reuses the existing `SpectrumFrame` renderer; adds the cursor overlay + control bar.

## Phased delivery

Per the decision on the issue: anything deferred **must be documented and planned
here, and executed before the project is called "done."** No phase is optional; the
phases exist only to sequence the work and land value early.

| Phase | Scope | "Done" gate |
|---|---|---|
| **A — Core rx + NBFM + tuning + waterfall** | `RtlTcpBackend` (connect/header/IQ read), complex NCO channel-select, NBFM demod, decimation to audio, power squelch, RF waterfall tap, `SetSdrTune` / `SetSdrGain` / `ConfigureSdr` / `GetSdrCaps`, TUI tuning view. End-to-end: bind `rtltcp:host:port`, tune 144.390, decode APRS. | APRS decodes off a real/remote dongle; waterfall + click-tune work in TUI. |
| **B — Demod-mode breadth** | AM, WFM, SSB (USB/LSB) demodulators behind the shipped `DemodMode` enum. | Each mode selectable at runtime and audibly/objectively correct on a known signal. |
| **C — Dongle extras** | ppm correction wired end-to-end, bias-tee, direct-sampling (HF), config-file device registration in `ListDevices`. | Each exposed control verifiably changes dongle behavior. |
| **D — Hardening** | Auto-reconnect on dropped `rtl_tcp` link (mirrors the rigctld PTT driver), buffer/overrun handling, multi-client tune arbitration, docs + handbook page. | Survives server restarts; documented for end users and API consumers. |

The `DemodMode` enum, all four control RPCs, and `GetSdrCaps` ship in Phase A so the
**API surface is stable from the first release**; later phases fill in behavior, and
unimplemented controls return `UNIMPLEMENTED` rather than changing the contract.

## Testing

- **Backend unit tests** with a **fake `rtl_tcp` server** (in-process TCP listener
  that emits the 12-byte header and a scripted IQ stream): header parse, command
  encoding round-trip, u8→Cplx conversion, reconnect.
- **DSP unit tests**: complex NCO shifts a synthetic tone to DC; NBFM demod recovers a
  known modulating tone; squelch opens/closes at threshold with hysteresis; decimation
  image rejection.
- **Integration**: synthetic IQ containing an FM-modulated AFSK1200 APRS burst →
  `RtlTcpBackend` → AFSK1200 mode → correct AX.25 frame. Reuses the existing mode
  conformance harness.
- **Spectrum**: complex FFT tap produces RF-referenced `freq_start_hz`/`freq_step_hz`;
  a tone at a known RF frequency lands in the expected bin.
- **TUI**: model-level tests for tune step/entry, cursor↔frequency mapping, and
  gain-table stepping.

## Decisions record

- **gqrx wideband + NCO model** — approved (instant fine-tune, better UX; reuses the
  existing retunable downconverter).
- **Demod-mode selector in the gRPC surface from day one** — approved; NBFM implemented
  first, AM/WFM/SSB phased (Phase B) but enum ships complete in Phase A.
- **Phasing** — approved; deferral allowed only if documented + planned here (it is) and
  executed before "done."
- **Squelch in v1** — approved (power squelch, Phase A).
