# Waterfall / Spectrum API — Design

> Status: **Design / RFC**, ready to implement. Daemon-side (`omnimodemd` + `dsp`)
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
  float  freq_hi_hz = 7;   // passband window high edge; 0 = Nyquist (native_rate/2)
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
columns on the SSB passband where the modes actually live, instead of spreading them
across the full 0–`native_rate/2` span (24 kHz on a 48 kHz AFSK channel, but only
6 kHz on a 12 kHz FT8 channel). Optional; default is the full passband to Nyquist.

## 3. Magnitude pipeline (server side)

Per emitted line, starting from one `Stft` spectrum `Vec<Complex<f32>>` (length
`nfft`):

1. Take the real half — bins `0..=nfft/2` (positive frequencies only).
2. Magnitude → amplitude-normalized: `mag[k] = |X[k]| / (window_sum/2)`.
3. dBFS: `db[k] = 20*log10(max(mag[k], 1e-9))` (full-scale sine ⇒ ~0 dBFS, matching
   the existing dbfs convention).
4. Restrict to `[freq_lo_hz, freq_hi_hz]` using `bin_hz(bin, native_rate)` (the tap
   rate is the demod native rate — see §4).
5. **Pool** that range down to `bin_count` buckets by **max** (peak-hold per bucket
   keeps narrow signals visible; mean would wash CW/PSK carriers out).
6. Quantize each bucket dB to uint8 over [`db_floor`,`db_ceiling`] (default
   −120..0 dBFS): `u = clamp(round(255*(db-floor)/(ceiling-floor)), 0, 255)`.

**Clamp `bin_count` to the available FFT bins in the range.** Max-pooling assumes
≥1 input bin per output bucket; a zoomed window with a large `bin_count` and a small
`fft_size` can leave it with *fewer* FFT bins than buckets (e.g. a 0–3 kHz window at
12 kHz native with `nfft=2048` has only ~512 bins). When
`range_bins < bin_count`, clamp `bin_count = range_bins` (don't upsample/duplicate)
and report the clamped value in `ConfigureSpectrumResponse.bin_count` — the response
already echoes the actual.

`freq_step_hz = (freq_hi_hz - freq_lo_hz) / bin_count`; `freq_start_hz =
freq_lo_hz + freq_step_hz/2` (bucket centers, computed *after* the clamp above). The
display labels its axis straight from those two numbers — no need to know `nfft`.

## 4. Producer wiring (`core/rx_worker.rs`)

**Enable/disable must reach a *running* worker — follow the `AudioGain` precedent,
not a plain local.** The RX worker thread (`spawn_streaming` / `spawn_windowed` in
`rx_worker.rs`) only ever reads its `capture.rx` channel; there is no command path
into the thread. `SetAudioGain` already solves exactly this: `AudioGain` is a
clonable `Arc<Atomic…>` handle the core writes and the worker reads with one relaxed
load per chunk, so a config change takes effect with **no respawn** (see
`core/gain.rs`). Spectrum needs the same shape — a shared `SpectrumControl` handle
cloned into the worker, *not* an `Option<SpectrumTap>` that only the thread can see
(a plain local could never be toggled by `ConfigureSpectrum` after spawn):

```rust
// shared, like AudioGain: core writes, worker reads each chunk
struct SpectrumControl { cfg: Arc<Mutex<Option<SpectrumCfg>>> }  // None = OFF
// worker-owned, rebuilt when cfg's generation changes:
struct SpectrumTap { stft: Stft, cfg: SpectrumCfg, sample_rate: u32 }
```

Each chunk the worker checks the handle: if a config is present and the tap is
missing or stale, it (re)builds `SpectrumTap`; if the config cleared, it drops the
tap. The worker already owns its `telemetry: broadcast::Sender<TelemetryEvent>`
clone, so no extra plumbing is needed to emit.

**The tap's sample rate is the demod's *native* rate, not the capture/working rate.**
The post-`rx_gain` samples handed to the demod have already been resampled to
`demod.caps().native_rate` (the `native` local in both spawners) — e.g. 12 kHz for
FT8, 48 kHz for AFSK1200 — *not* the 48 kHz capture rate. Every rate-dependent
quantity (`hop`, `bin_hz`, default `freq_hi_hz = native/2`, `freq_step_hz`) must key
off `native`. This is known at tap-build time, so build the `Stft` then.

In the existing per-chunk loop, after resample + `rx_gain` (same samples handed to
the demod), call `tap.stft.feed(&samples)`; for each STFT frame it returns, run §3
and `telemetry.send(TelemetryEvent::SpectrumFrame { … })`.

- **Two worker paths.** `spawn_streaming` (AFSK/CW/PSK) feeds per audio chunk;
  `spawn_windowed` (FT8/WSPR) buffers multi-second windows before decoding. The hook
  goes in **both** — feed the post-gain `chunk_samples` to the tap *before* the
  windowing `buf.extend_from_slice`, so the waterfall updates continuously even on
  windowed modes that only decode every 15 s. (If you'd rather ship streaming-only
  first and treat windowed modes as a follow-on, say so explicitly — but the FT8
  screen is precisely where a waterfall is most wanted, so don't skip it silently.)
- **Rate / hop.** `hop = clamp(native_rate / rate_hz, 1, nfft)` (the `Stft`
  constructor asserts `hop <= nfft`). If the requested `rate_hz` would need
  `hop > nfft`, the real ceiling is `native_rate / nfft` (~5.9 line/s at FT8's
  12 kHz / 2048, ~23 at 48 kHz) — report the achievable `rate_hz` in the response.
- **Cost.** One 2048-pt real FFT at ~15/s is negligible; and it runs **only when
  enabled**. 256 uint8 bins × 15/s = ~3.8 KB/s per channel on the wire — trivial
  over UDS or mTLS.
- **Lifecycle.** `enable=false`, channel teardown, or audio reconfigure clears the
  shared cfg → the worker drops its tap. `ConfigureAudio` can change `native` (a
  different mode/rate); the worker rebuilds the tap (new `hop`/`freq_step`) on the
  next chunk when it sees the cfg generation bumped.

## 5. Plumbing checklist (files)

- `proto/omnimodem.proto` — `SpectrumFrame`, `Event.spectrum_frame = 13`,
  `ConfigureSpectrum` + req/resp. (PR description must state "additive within v1".)
- `crates/dsp/src/frontend/spectrum.rs` *(new, small)* — `magnitudes_dbfs`,
  range-restrict, max-pool, uint8 quantize. Pure fn over an `Stft` frame; unit-test
  with a known tone (peak lands in the right bucket) and a full-scale sine (~0 dBFS).
  Keep `Stft` itself untouched.
- `crates/omnimodemd/src/core/event.rs` — `TelemetryEvent::SpectrumFrame { channel,
  timestamp_ns, freq_start_hz, freq_step_hz, db_floor, db_ceiling, bins: Vec<u8> }`.
- `crates/omnimodemd/src/core/command.rs` — `Command::ConfigureSpectrum { … }`
  (mirror `SetAudioGain`).
- `crates/omnimodemd/src/core/gain.rs` (or a sibling) — a `SpectrumControl` shared
  handle modeled on `AudioGain` (clonable `Arc`; core writes cfg, worker reads).
- `crates/omnimodemd/src/core/mod.rs` — own a per-channel `SpectrumControl` in
  `LiveBindings` (alongside `gains`); clone it into the worker in `try_spawn_workers`
  (both `spawn_streaming` and `spawn_windowed`); set/clear its cfg on the command.
  The telemetry sender is *already* passed to the workers — no new plumbing there.
- `crates/omnimodemd/src/core/rx_worker.rs` — the `SpectrumTap` hook in **both**
  `spawn_streaming` and `spawn_windowed` (see §4); add a `SpectrumControl` arg to each
  spawner (mirrors how `gain: AudioGain` is threaded today).
- `crates/omnimodemd/src/grpc/convert.rs` — `TelemetryEvent::SpectrumFrame` →
  `Event::Kind::SpectrumFrame`.
- `crates/omnimodemd/src/grpc/service.rs` — `configure_spectrum` handler →
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
