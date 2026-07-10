# Waterfall / Spectrum API — Design

> Status: **Design / RFC**, ready to implement. Daemon-side (`omnimodem` + `dsp`)
> additions that let any frontend (the TUI, a GUI, a web client) draw a real
> waterfall. Additive within `omnimodem.v1` (see `proto/VERSIONING.md`).

A waterfall is a **time series of spectra**: each tick the receiver computes one
FFT line over the audio passband and the display scrolls it. So the API needs one
new streamed event carrying a single spectrum line, plus a small RPC to turn it on
and size it. Everything heavy already exists in the codebase — this is mostly
wiring a wire contract to the existing STFT.

## 1. What already exists (reuse, don't rebuild)

- **`dsp::frontend::stft::Stft`** — Hann-windowed, hopped real-FFT
  (`new(nfft, hop)` → `feed(&[Sample]) -> Vec<Vec<Complex<f32>>>`), with
  `bin_hz(bin, rate)` and `window_sum()` for amplitude normalization. Its own doc
  says *"Powers the waterfall and noncoherent tone detection."* This is the engine.
- **RX worker** (`core/rx_worker.rs`) — already pulls capture chunks, resamples to
  the working rate, applies `rx_gain` (#20), computes dBFS, and feeds the demod. It
  owns exactly the post-gain `Vec<Sample>` the FFT should see. **This is the tap
  point** — no new capture or fan-out consumer required.
- **Telemetry broadcast** — `TelemetryEvent` is the LOSSY class
  (`core/event.rs`): lag drops intermediates, which is precisely right for a
  waterfall (a skipped line is invisible). `grpc/convert.rs` maps `TelemetryEvent`
  → proto `Event` oneof.
- **Control-RPC pattern** — `ConfigureAudio` / `SetAudioGain` / `ConfigureKissListener`
  already model "configure a per-channel feature, echo back the actual params."
  `ConfigureSpectrum` follows the same shape.

So the new surface is: **one event + one RPC + one producer hook + the convert glue.**

## 2. Proto additions (`proto/omnimodem.proto`, additive)

### 2.1 The spectrum line event

```proto
// One waterfall line: a magnitude spectrum over the channel's audio passband.
// LOSSY class — a lagging subscriber drops lines (a missed line is invisible).
message SpectrumFrame {
  uint32 channel       = 1;
  uint64 timestamp_ns  = 2;   // wall-clock of the window's leading edge (0 if unknown)
  float  freq_start_hz = 3;   // center frequency of bin[0]
  float  freq_step_hz  = 4;   // Hz per output bin
  float  db_floor      = 5;   // dBFS mapped to bin value 0
  float  db_ceiling    = 6;   // dBFS mapped to bin value 255
  bytes  bins          = 7;   // uint8 quantized dBFS, len == bin_count, low→high freq
}
```

Add to the `Event` oneof (next free tag is **13**; 1–12 are taken):

```proto
    SpectrumFrame spectrum_frame = 13;  // LOSSY class
```

**Why `bytes` of uint8, not `repeated float`.** A waterfall renders *intensity*,
not precise dB — color/braille shading wants 0..255 already. uint8 over a known
[`db_floor`,`db_ceiling`] range is exactly that, and it cuts the payload 4× vs
`float`. The client maps `bin/255` → color (or a TUI ramp ` .:-=+*#%@`) directly.
If a future client genuinely needs raw float bins, that's an additive
`repeated float bins_db = 8;` later — not needed now.

### 2.2 The control RPC

```proto
  // Enable/disable and size a channel's spectrum (waterfall) stream. Default OFF
  // so the FFT costs nothing when no one is watching. Idempotent per channel.
  rpc ConfigureSpectrum(ConfigureSpectrumRequest) returns (ConfigureSpectrumResponse);

message ConfigureSpectrumRequest {
  uint32 channel    = 1;
  bool   enable     = 2;   // false = stop the producer (other fields ignored)
  uint32 bin_count  = 3;   // output bins (display width); 0 = server default (256)
  uint32 fft_size   = 4;   // FFT length; 0 = server default (2048), rounded to pow2
  uint32 rate_hz    = 5;   // target lines/sec; 0 = server default (15)
  float  freq_lo_hz = 6;   // passband window low edge; 0 = 0 Hz
  float  freq_hi_hz = 7;   // passband window high edge; 0 = Nyquist (rate/2)
}

message ConfigureSpectrumResponse {
  uint32 bin_count    = 1;   // actual, after clamping
  uint32 fft_size     = 2;   // actual
  uint32 rate_hz      = 3;   // actual achievable (see hop constraint, §4)
  float  freq_start_hz = 4;  // matches SpectrumFrame.freq_start_hz
  float  freq_step_hz  = 5;  // matches SpectrumFrame.freq_step_hz
}
```

`freq_lo_hz`/`freq_hi_hz` give a **zoom**: a 0–3 kHz window puts all the display
columns on the SSB passband where the modes actually live, instead of wasting them
on empty 3–24 kHz. Optional; default is the full passband.

## 3. Magnitude pipeline (server side)

Per emitted line, starting from one `Stft` spectrum `Vec<Complex<f32>>` (length
`nfft`):

1. Take the real half — bins `0..=nfft/2` (positive frequencies only).
2. Magnitude → amplitude-normalized: `mag[k] = |X[k]| / (window_sum/2)`.
3. dBFS: `db[k] = 20*log10(max(mag[k], 1e-9))` (full-scale sine ⇒ ~0 dBFS, matching
   the existing dbfs convention).
4. Restrict to `[freq_lo_hz, freq_hi_hz]` using `bin_hz`.
5. **Pool** that range down to `bin_count` buckets by **max** (peak-hold per bucket
   keeps narrow signals visible; mean would wash CW/PSK carriers out).
6. Quantize each bucket dB to uint8 over [`db_floor`,`db_ceiling`] (default
   −120..0 dBFS): `u = clamp(round(255*(db-floor)/(ceiling-floor)), 0, 255)`.

`freq_step_hz = (freq_hi_hz - freq_lo_hz) / bin_count`; `freq_start_hz =
freq_lo_hz + freq_step_hz/2` (bucket centers). The display labels its axis straight
from those two numbers — no need to know `nfft`.

## 4. Producer wiring (`core/rx_worker.rs`)

When spectrum is enabled for a channel, the worker holds an `Option<SpectrumTap>`:

```
struct SpectrumTap { stft: Stft, cfg: SpectrumCfg, telemetry: Sender<TelemetryEvent> }
```

In the existing per-chunk loop, after resample + `rx_gain` (same samples handed to
the demod), call `tap.feed(&samples)`; for each STFT frame it returns, run §3 and
`telemetry.send(TelemetryEvent::SpectrumFrame { … })`. Nothing else in the worker
changes.

- **Rate / hop.** `hop = clamp(working_rate / rate_hz, 1, nfft)` (the `Stft`
  constructor asserts `hop <= nfft`). If the requested `rate_hz` would need
  `hop > nfft`, the real ceiling is `working_rate / nfft` (~23 line/s at 48 kHz /
  2048) — already plenty; report the achievable `rate_hz` in the response.
- **Cost.** One 2048-pt real FFT at ~15/s is negligible; and it runs **only when
  enabled**. 256 uint8 bins × 15/s = ~3.8 KB/s per channel on the wire — trivial
  over UDS or mTLS.
- **Lifecycle.** `enable=false`, channel teardown, or audio reconfigure drops the
  tap. Rebind/resample changes `working_rate`; recompute `hop`/`freq_step` on
  (re)enable.

## 5. Plumbing checklist (files)

- `proto/omnimodem.proto` — `SpectrumFrame`, `Event.spectrum_frame = 13`,
  `ConfigureSpectrum` + req/resp. (PR description must state "additive within v1".)
- `crates/dsp/src/frontend/spectrum.rs` *(new, small)* — `magnitudes_dbfs`,
  range-restrict, max-pool, uint8 quantize. Pure fn over an `Stft` frame; unit-test
  with a known tone (peak lands in the right bucket) and a full-scale sine (~0 dBFS).
  Keep `Stft` itself untouched.
- `crates/omnimodem/src/core/event.rs` — `TelemetryEvent::SpectrumFrame { channel,
  timestamp_ns, freq_start_hz, freq_step_hz, db_floor, db_ceiling, bins: Vec<u8> }`.
- `crates/omnimodem/src/core/command.rs` — `Command::ConfigureSpectrum { … }`
  (mirror `SetAudioGain`).
- `crates/omnimodem/src/core/mod.rs` — own the per-channel `SpectrumCfg`; create/
  drop the tap on the command; clone the telemetry sender into the worker.
- `crates/omnimodem/src/core/rx_worker.rs` — the `SpectrumTap` hook above.
- `crates/omnimodem/src/grpc/convert.rs` — `TelemetryEvent::SpectrumFrame` →
  `Event::Kind::SpectrumFrame`.
- `crates/omnimodem/src/grpc/service.rs` — `configure_spectrum` handler →
  `Command::ConfigureSpectrum`, echo actual params.

## 6. Client (TUI) consumption — sanity check on the contract

The TUI keeps a ring of the last N `SpectrumFrame.bins` (N = waterfall height),
maps each uint8 to a shade (`░▒▓█` or a 256-color ramp), and labels the X axis from
`freq_start_hz`/`freq_step_hz`. A horizontal one-line spectrogram (peak-hold) is the
degenerate height-1 case. On the FT8 screen the same bins highlight the active
sub-band; on ragchew screens they show where to tune. This is exactly the gap called
out in the TUI design's §9 — once this lands, the "level-meter-only" placeholder
becomes a real waterfall.

## 7. Open questions / decisions for the implementer

1. **uint8 vs float bins.** Recommending packed uint8 + `db_floor`/`db_ceiling`
   (compact, render-ready). Add `repeated float` only if a precise-dB client appears.
2. **Pooling = max vs mean.** Recommending **max** (peak-hold) so narrow carriers
   survive decimation. Mean is smoother but hides exactly the signals operators hunt.
3. **Dynamic range floor/ceiling.** Fixed server default (−120..0 dBFS) reported in
   every frame, or client-settable in `ConfigureSpectrum`? Start fixed; promote to a
   request field (additive) if auto-gain'd displays want it.
4. **TX-side monitor.** This designs the **RX** passband spectrum. A TX monitor
   (FFT of the outgoing modulated audio) is a natural follow-on using the same
   `SpectrumFrame` from the TX worker — out of scope here unless you want it now.
5. **`fanout` vs in-worker tap.** Tapping inside the RX worker reuses the samples
   with zero copies and no extra consumer. If you'd rather decouple it (e.g. spectrum
   surviving demod hiccups), make it a `fanout` capture consumer instead — costs one
   extra resampled copy.
