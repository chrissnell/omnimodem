# Waterfall / Spectrum API Implementation Plan

> ## ✅ STATUS: LANDED in [#24](https://github.com/chrissnell/omnimodem/pull/24) (commit `57a3f9b`, on `main`).
> The waterfall/spectrum API shipped — every task below is done (boxes checked).
> The shipped implementation **diverges from this plan in a few places, and the
> shipped choices win** — see [§ Reconciliation](#reconciliation-plan-vs-what-shipped-24)
> immediately below. This document is retained as the design-decision record; the
> local in-progress branch that pre-dated #24 is abandoned (superseded by #24).

> **For agentic workers (historical):** this was an executable plan
> (superpowers:executing-plans). It is now a record, not pending work.

**Goal:** Implement the waterfall/spectrum API from `docs/design/2026-06-23-omnimodem-waterfall-spectrum-api.md` — a streamed `SpectrumFrame` event plus a `ConfigureSpectrum` RPC — so any frontend can draw a real waterfall. **(Achieved in #24.)**

**Architecture (as shipped):** A per-frame pure transform in `dsp::frontend::spectrum` (magnitude→dBFS→range-restrict→max-pool→uint8) feeds a stateful `SpectrumTap` in the RX workers, fed by the existing `Stft`. A shared `SpectrumControl` cell (same clone-into-worker pattern as `AudioGain`) toggles a *running* worker with no respawn. Frames ride the existing LOSSY `TelemetryEvent` broadcast.

**Tech Stack:** Rust, tonic/prost (gRPC), `rustfft` (via `dsp::frontend::stft::Stft`), `std::thread` sync core.

## Reconciliation: plan vs what shipped (#24)

`#24` matches this plan's overall shape — proto `SpectrumFrame` (Event oneof tag 13,
LOSSY) + `ConfigureSpectrum` RPC (additive in `omnimodem.v1`); `dsp::frontend::spectrum`
pure pipeline; `core::spectrum::SpectrumControl`; an `RxWorker` tap in both the
streaming and windowed paths; convert/service glue; uint8 max-pool dBFS encoding;
default-OFF; RPC echoes the actual clamped params. Where it **diverged, follow the
shipped code, not the task bodies below**:

1. **Tap point — native rate, not capture rate.** This plan proposed a *deviation*:
   tap raw capture-rate samples *before* resample. **#24 did not take it.** It taps
   the **post-gain samples at the demod native rate** (the original design's approach)
   in both worker paths, adding `mode::registry::native_rate(cfg)` so the FFT sees
   exactly the mode's working band. Tasks 3 & 6 below describe the capture-rate tap —
   that approach was discarded.
2. **Resolution lives in `dsp`, keyed on native rate.** This plan resolved params in
   the core against the capture rate (`ResolvedSpectrumCfg`). #24 resolves in
   `dsp` via `SpectrumSetup::resolve(native_rate, …)` + `SpectrumPlan::new`, and the
   *same* resolve feeds both the RPC echo and the producer, so they can never
   disagree. `SpectrumPlan` clamps `freq_lo/hi` to `[0, Nyquist]`.
3. **`SpectrumControl` uses a generation counter, not a polled mutex.** This plan
   stored `Arc<Mutex<Option<ResolvedSpectrumCfg>>>` read every chunk. #24 holds the
   raw `SpectrumCfg` behind an `AtomicU64` generation: the worker does one relaxed
   `generation()` load per chunk and only locks/rebuilds the tap when it changes —
   a cheaper hot path.
4. **`timestamp_ns` is stamped, not 0.** This plan emitted `0`; #24 (code-review
   follow-up) stamps wall-clock at emit so the waterfall has a real time axis.
5. **Lifecycle: drop on device departure.** #24 (review follow-up) drops a channel's
   `SpectrumControl` when its device departs, so a replugged device starts with the
   waterfall OFF rather than silently resuming the FFT. This plan didn't specify it.
6. **`fft_size` clamp.** Shipped clamps to `[64, 16384]` then rounds up to a power of
   two; this plan said `[256, 8192]`. Minor.

Per #24: `dsp` 230 tests and `omnimodem` 140 lib + integration tests pass.

---

### Original plan (below) — historical; tap-point details superseded per §Reconciliation.

**Build env note:** this workspace needs `protoc` and ALSA dev libs. Export before any cargo command:
```bash
export PROTOC="$HOME/.local/bin/protoc"
export PATH="$HOME/.local/bin:$PATH"
export PKG_CONFIG_PATH=/tmp/alsa-root/prefix/usr/lib/x86_64-linux-gnu/pkgconfig
export LIBRARY_PATH=/tmp/alsa-root/prefix/usr/lib/x86_64-linux-gnu
export LD_LIBRARY_PATH=/tmp/alsa-root/prefix/usr/lib/x86_64-linux-gnu
```

---

## File Structure

- `proto/omnimodem.proto` — `SpectrumFrame`, `Event.spectrum_frame = 13`, `ConfigureSpectrum` RPC + req/resp (additive).
- `crates/dsp/src/frontend/spectrum.rs` *(new)* — pure per-frame transform + unit tests.
- `crates/dsp/src/frontend/mod.rs` — `pub mod spectrum;`.
- `crates/omnimodem/src/core/spectrum.rs` *(new)* — `SpectrumControl`, `ResolvedSpectrumCfg`, `resolve()`, `SpectrumTap` + unit tests.
- `crates/omnimodem/src/core/event.rs` — `TelemetryEvent::SpectrumFrame`.
- `crates/omnimodem/src/core/command.rs` — `Command::ConfigureSpectrum`, `ConfigureSpectrumOk`.
- `crates/omnimodem/src/core/mod.rs` — own `controls`/`rx_rates` maps; handler; clone control into workers.
- `crates/omnimodem/src/core/rx_worker.rs` — call `SpectrumTap` in the streaming + windowed loops.
- `crates/omnimodem/src/grpc/convert.rs` — `TelemetryEvent::SpectrumFrame` → proto.
- `crates/omnimodem/src/grpc/service.rs` — `configure_spectrum` handler.

---

## Task 1: Proto surface

**Files:**
- Modify: `proto/omnimodem.proto`

- [x] **Step 1: Add the `SpectrumFrame` message** after `ChannelMetrics` (anywhere in the message section):

```proto
// One waterfall line: a magnitude spectrum over the channel's audio passband.
// LOSSY class — a lagging subscriber drops lines (a missed line is invisible).
message SpectrumFrame {
  uint32 channel       = 1;
  uint64 timestamp_ns  = 2;   // 0 in this phase (no wall-clock stamp yet)
  float  freq_start_hz = 3;   // center frequency of bin[0]
  float  freq_step_hz  = 4;   // Hz per output bin
  float  db_floor      = 5;   // dBFS mapped to bin value 0
  float  db_ceiling    = 6;   // dBFS mapped to bin value 255
  bytes  bins          = 7;   // uint8 quantized dBFS, len == bin_count, low→high freq
}
```

- [x] **Step 2: Add to the `Event` oneof** (after `channel_metrics = 12`):

```proto
    SpectrumFrame spectrum_frame = 13;  // LOSSY class
```

- [x] **Step 3: Add the RPC** in `service ModemControl` (after `SetAudioGain`):

```proto
  // Enable/disable and size a channel's spectrum (waterfall) stream. Default OFF
  // so the FFT costs nothing when no one is watching. Idempotent per channel.
  rpc ConfigureSpectrum(ConfigureSpectrumRequest) returns (ConfigureSpectrumResponse);
```

- [x] **Step 4: Add req/resp messages:**

```proto
message ConfigureSpectrumRequest {
  uint32 channel    = 1;
  bool   enable     = 2;   // false = stop the producer (sizing fields ignored)
  uint32 bin_count  = 3;   // output bins (display width); 0 = server default (256)
  uint32 fft_size   = 4;   // FFT length; 0 = server default (2048), forced to pow2
  uint32 rate_hz    = 5;   // target lines/sec; 0 = server default (15)
  float  freq_lo_hz = 6;   // passband window low edge; 0 = 0 Hz
  float  freq_hi_hz = 7;   // passband window high edge; 0 = Nyquist (capture_rate/2)
}

message ConfigureSpectrumResponse {
  uint32 bin_count     = 1;   // actual, after clamping
  uint32 fft_size      = 2;   // actual
  uint32 rate_hz       = 3;   // actual achievable line rate
  float  freq_start_hz = 4;   // matches SpectrumFrame.freq_start_hz
  float  freq_step_hz  = 5;   // matches SpectrumFrame.freq_step_hz
}
```

- [x] **Step 5: Verify it compiles** (regenerates tonic bindings):

Run: `cargo check -p omnimodem`
Expected: builds; `proto::SpectrumFrame`, `proto::ConfigureSpectrumRequest/Response` now exist.

- [x] **Step 6: Commit**

```bash
git add proto/omnimodem.proto
git commit -m "proto: add SpectrumFrame event + ConfigureSpectrum RPC (additive, v1)"
```

---

## Task 2: DSP per-frame transform (`dsp::frontend::spectrum`)

Pure functions over one `Stft` spectrum. No daemon deps.

**Files:**
- Create: `crates/dsp/src/frontend/spectrum.rs`
- Modify: `crates/dsp/src/frontend/mod.rs`

- [x] **Step 1: Register the module.** Add to `crates/dsp/src/frontend/mod.rs` after `pub mod stft;`:

```rust
pub mod spectrum;
```

- [x] **Step 2: Write the failing tests.** Create `crates/dsp/src/frontend/spectrum.rs`:

```rust
//! Per-line waterfall transform: one STFT spectrum → quantized dBFS bins.
//! Pure functions; the daemon's RX worker wires these to `Stft` output.

use rustfft::num_complex::Complex;

/// Positive-frequency dBFS for one STFT frame. Input is a full `nfft` complex
/// spectrum; output is the real half `0..=nfft/2`, amplitude-normalized by the
/// window sum so a full-scale sine reads ~0 dBFS.
pub fn half_spectrum_dbfs(spectrum: &[Complex<f32>], window_sum: f32) -> Vec<f32> {
    let nfft = spectrum.len();
    let norm = (window_sum / 2.0).max(1e-9);
    (0..=nfft / 2)
        .map(|k| {
            let mag = spectrum[k].norm() / norm;
            20.0 * mag.max(1e-9).log10()
        })
        .collect()
}

/// Max-pool the inclusive source range `lo..=hi` of `db` into `buckets` output
/// bins (peak-hold keeps narrow carriers visible). `buckets` is clamped to the
/// range width by the caller; this asserts the precondition.
pub fn max_pool(db: &[f32], lo: usize, hi: usize, buckets: usize) -> Vec<f32> {
    assert!(hi >= lo && hi < db.len() && buckets >= 1);
    let width = hi - lo + 1;
    (0..buckets)
        .map(|b| {
            let start = lo + b * width / buckets;
            let end = (lo + (b + 1) * width / buckets).max(start + 1).min(hi + 1);
            db[start..end].iter().copied().fold(f32::NEG_INFINITY, f32::max)
        })
        .collect()
}

/// Quantize dBFS to uint8 over `[floor, ceiling]` (0=floor, 255=ceiling).
pub fn quantize(db: &[f32], floor: f32, ceiling: f32) -> Vec<u8> {
    let span = (ceiling - floor).max(1e-9);
    db.iter()
        .map(|&v| (((v - floor) / span) * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::{num_complex::Complex, FftPlanner};

    // Hann-windowed FFT of a full-scale sine peaks at ~0 dBFS in its bin.
    fn windowed_fft(freq: f32, rate: f32, nfft: usize) -> (Vec<Complex<f32>>, f32) {
        let window: Vec<f32> = (0..nfft)
            .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / nfft as f32).cos())
            .collect();
        let window_sum: f32 = window.iter().sum();
        let mut buf: Vec<Complex<f32>> = (0..nfft)
            .map(|n| {
                let s = (std::f32::consts::TAU * freq * n as f32 / rate).sin();
                Complex::new(s * window[n], 0.0)
            })
            .collect();
        FftPlanner::new().plan_fft_forward(nfft).process(&mut buf);
        (buf, window_sum)
    }

    #[test]
    fn full_scale_sine_reads_near_zero_dbfs() {
        let nfft = 2048;
        let rate = 48000.0;
        let tone = 3000.0;
        let (spec, ws) = windowed_fft(tone, rate, nfft);
        let db = half_spectrum_dbfs(&spec, ws);
        let peak = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!((peak - 0.0).abs() < 1.0, "peak {peak} dBFS not ~0");
    }

    #[test]
    fn peak_lands_in_the_expected_bucket() {
        let nfft = 2048;
        let rate = 48000.0;
        let tone = 3000.0;
        let (spec, ws) = windowed_fft(tone, rate, nfft);
        let db = half_spectrum_dbfs(&spec, ws); // len nfft/2+1, covers 0..24kHz
        let buckets = 256;
        let pooled = max_pool(&db, 0, nfft / 2, buckets);
        let (argmax, _) = pooled
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        // bucket center frequency ≈ tone
        let step = (rate / 2.0) / buckets as f32;
        let center = (argmax as f32 + 0.5) * step;
        assert!((center - tone).abs() < step * 2.0, "peak bucket {center}Hz vs {tone}");
    }

    #[test]
    fn quantize_maps_endpoints() {
        let q = quantize(&[-120.0, 0.0, -60.0, 10.0, -200.0], -120.0, 0.0);
        assert_eq!(q[0], 0);
        assert_eq!(q[1], 255);
        assert_eq!(q[2], 128);
        assert_eq!(q[3], 255); // clamped
        assert_eq!(q[4], 0); // clamped
    }
}
```

- [x] **Step 3: Run tests, expect FAIL** (module not yet wired):

Run: `cargo test -p omnimodem-dsp spectrum`
Expected: compiles after Step 1+2; tests pass. (If you wrote Step 2 fully, they pass immediately — that is fine for a pure-function task.)

- [x] **Step 4: Run tests, expect PASS:**

Run: `cargo test -p omnimodem-dsp spectrum`
Expected: 3 passed.

- [x] **Step 5: Commit**

```bash
git add crates/dsp/src/frontend/spectrum.rs crates/dsp/src/frontend/mod.rs
git commit -m "dsp: spectrum line transform (half-spectrum dBFS, max-pool, quantize)"
```

---

## Task 3: Core resolution + tap (`core/spectrum.rs`)

Resolve request params (against the capture rate) into a `ResolvedSpectrumCfg`; hold a shared `SpectrumControl`; a `SpectrumTap` runs the per-chunk FFT.

**Files:**
- Create: `crates/omnimodem/src/core/spectrum.rs`
- Modify: `crates/omnimodem/src/core/mod.rs` (add `mod spectrum;` + re-exports)

- [x] **Step 1: Register the module.** In `crates/omnimodem/src/core/mod.rs`, next to `mod gain;`:

```rust
mod spectrum;
pub(crate) use spectrum::{ResolvedSpectrumCfg, SpectrumControl, SpectrumParams, SpectrumTap};
```

- [x] **Step 2: Write `core/spectrum.rs` with tests:**

```rust
//! Spectrum (waterfall) producer: request resolution + the per-worker FFT tap.
//!
//! The core resolves a client request against the channel's capture rate into a
//! `ResolvedSpectrumCfg` and publishes it on a shared `SpectrumControl` cell
//! (cloned into the RX worker, like `AudioGain`). The worker's `SpectrumTap`
//! reads the cell each chunk and emits `TelemetryEvent::SpectrumFrame`.

use crate::core::event::TelemetryEvent;
use crate::ids::ChannelId;
use omnimodem_dsp::frontend::spectrum::{half_spectrum_dbfs, max_pool, quantize};
use omnimodem_dsp::frontend::stft::Stft;
use omnimodem_dsp::types::Sample;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

const DEFAULT_BINS: u32 = 256;
const DEFAULT_FFT: u32 = 2048;
const DEFAULT_RATE: u32 = 15;
const DB_FLOOR: f32 = -120.0;
const DB_CEILING: f32 = 0.0;

/// Raw request sizing (0 == "use default"), as it arrives from the RPC.
#[derive(Debug, Clone, Copy)]
pub struct SpectrumParams {
    pub bin_count: u32,
    pub fft_size: u32,
    pub rate_hz: u32,
    pub freq_lo_hz: f32,
    pub freq_hi_hz: f32,
}

/// Fully resolved, rate-aware spectrum config. Built by `resolve`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSpectrumCfg {
    pub nfft: usize,
    pub hop: usize,
    pub lo_bin: usize,
    pub hi_bin: usize,
    pub bin_count: usize,
    pub freq_start_hz: f32,
    pub freq_step_hz: f32,
    pub db_floor: f32,
    pub db_ceiling: f32,
    pub rate_hz: u32,
}

/// Resolve a request against `capture_rate` (Hz). Pure; testable.
pub fn resolve(p: SpectrumParams, capture_rate: u32) -> ResolvedSpectrumCfg {
    let rate = capture_rate.max(1) as f32;
    let nyquist = rate / 2.0;

    let nfft = {
        let req = if p.fft_size == 0 { DEFAULT_FFT } else { p.fft_size };
        (req.next_power_of_two() as usize).clamp(256, 8192)
    };
    let half = nfft / 2; // top usable bin index

    let lo_hz = p.freq_lo_hz.max(0.0).min(nyquist);
    let hi_hz = if p.freq_hi_hz <= 0.0 { nyquist } else { p.freq_hi_hz.clamp(lo_hz + 1.0, nyquist) };

    let lo_bin = ((lo_hz * nfft as f32 / rate).round() as usize).min(half);
    let hi_bin = ((hi_hz * nfft as f32 / rate).round() as usize).clamp(lo_bin + 1, half);

    let avail = hi_bin - lo_bin + 1;
    let req_bins = if p.bin_count == 0 { DEFAULT_BINS } else { p.bin_count } as usize;
    let bin_count = req_bins.clamp(1, avail);

    let req_rate = if p.rate_hz == 0 { DEFAULT_RATE } else { p.rate_hz };
    let hop = ((capture_rate / req_rate.max(1)) as usize).clamp(1, nfft);
    let rate_hz = (capture_rate as f32 / hop as f32).round() as u32;

    // Actual covered band uses bin edges so freq labels match the pooled output.
    let band_lo = lo_bin as f32 * rate / nfft as f32;
    let band_hi = (hi_bin + 1) as f32 * rate / nfft as f32;
    let freq_step_hz = (band_hi - band_lo) / bin_count as f32;
    let freq_start_hz = band_lo + freq_step_hz / 2.0;

    ResolvedSpectrumCfg {
        nfft, hop, lo_bin, hi_bin, bin_count,
        freq_start_hz, freq_step_hz,
        db_floor: DB_FLOOR, db_ceiling: DB_CEILING, rate_hz,
    }
}

/// Shared enable/config cell. `None` == disabled. Cloned into the RX worker.
#[derive(Clone, Default)]
pub struct SpectrumControl(Arc<Mutex<Option<ResolvedSpectrumCfg>>>);

impl SpectrumControl {
    pub fn set(&self, cfg: Option<ResolvedSpectrumCfg>) {
        *self.0.lock().unwrap() = cfg;
    }
    pub fn get(&self) -> Option<ResolvedSpectrumCfg> {
        self.0.lock().unwrap().clone()
    }
}

/// Per-worker stateful FFT tap. Lives in the worker thread; rebuilds its `Stft`
/// when the resolved config changes, drops it when disabled.
pub struct SpectrumTap {
    channel: ChannelId,
    telemetry: broadcast::Sender<TelemetryEvent>,
    active: Option<(ResolvedSpectrumCfg, Stft)>,
}

impl SpectrumTap {
    pub fn new(channel: ChannelId, telemetry: broadcast::Sender<TelemetryEvent>) -> Self {
        SpectrumTap { channel, telemetry, active: None }
    }

    /// Feed one chunk of raw capture samples (at the capture rate). Reads the
    /// shared control, (re)builds/drops the `Stft` as needed, and emits one
    /// `SpectrumFrame` per completed STFT hop.
    pub fn process(&mut self, control: &SpectrumControl, raw: &[Sample]) {
        let want = control.get();
        match (&want, &self.active) {
            (None, _) => {
                self.active = None;
                return;
            }
            (Some(cfg), Some((cur, _))) if cur == cfg => {}
            (Some(cfg), _) => {
                self.active = Some((cfg.clone(), Stft::new(cfg.nfft, cfg.hop)));
            }
        }
        let Some((cfg, stft)) = self.active.as_mut() else { return };
        let window_sum = stft.window_sum();
        for spec in stft.feed(raw) {
            let db = half_spectrum_dbfs(&spec, window_sum);
            let pooled = max_pool(&db, cfg.lo_bin, cfg.hi_bin, cfg.bin_count);
            let bins = quantize(&pooled, cfg.db_floor, cfg.db_ceiling);
            let _ = self.telemetry.send(TelemetryEvent::SpectrumFrame {
                channel: self.channel,
                timestamp_ns: 0,
                freq_start_hz: cfg.freq_start_hz,
                freq_step_hz: cfg.freq_step_hz,
                db_floor: cfg.db_floor,
                db_ceiling: cfg.db_ceiling,
                bins,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_resolve_against_48k() {
        let c = resolve(
            SpectrumParams { bin_count: 0, fft_size: 0, rate_hz: 0, freq_lo_hz: 0.0, freq_hi_hz: 0.0 },
            48_000,
        );
        assert_eq!(c.nfft, 2048);
        assert_eq!(c.bin_count, 256);
        assert_eq!(c.lo_bin, 0);
        assert_eq!(c.hi_bin, 1024);
        assert_eq!(c.hop, 3200); // 48000/15
        assert_eq!(c.rate_hz, 15);
        assert!((c.db_floor - -120.0).abs() < 1e-6);
    }

    #[test]
    fn fft_size_forced_to_pow2_and_clamped() {
        let c = resolve(
            SpectrumParams { bin_count: 0, fft_size: 1000, rate_hz: 0, freq_lo_hz: 0.0, freq_hi_hz: 0.0 },
            48_000,
        );
        assert_eq!(c.nfft, 1024);
    }

    #[test]
    fn passband_zoom_restricts_bins_and_freq() {
        let c = resolve(
            SpectrumParams { bin_count: 200, fft_size: 2048, rate_hz: 15, freq_lo_hz: 0.0, freq_hi_hz: 3000.0 },
            48_000,
        );
        assert_eq!(c.hi_bin, 128); // 3000*2048/48000 = 128
        assert_eq!(c.bin_count, 129.min(200)); // avail = 128-0+1 = 129
        assert!(c.freq_start_hz > 0.0 && c.freq_step_hz > 0.0);
        assert!((c.freq_start_hz + c.freq_step_hz * (c.bin_count as f32 - 0.5) - 3000.0).abs() < 50.0);
    }

    #[test]
    fn high_rate_clamps_hop_to_one_and_reports_actual() {
        let c = resolve(
            SpectrumParams { bin_count: 0, fft_size: 256, rate_hz: 100_000, freq_lo_hz: 0.0, freq_hi_hz: 0.0 },
            48_000,
        );
        assert_eq!(c.hop, 1);
        assert_eq!(c.rate_hz, 48_000);
    }
}
```

- [x] **Step 3: Add the event variant first** if not yet present — Task 4 Step 1 defines `TelemetryEvent::SpectrumFrame`. Do Task 4 Step 1 now if compiling this task fails on the missing variant, then return here.

- [x] **Step 4: Run tests:**

Run: `cargo test -p omnimodem core::spectrum`
Expected: 4 passed.

- [x] **Step 5: Commit**

```bash
git add crates/omnimodem/src/core/spectrum.rs crates/omnimodem/src/core/mod.rs
git commit -m "core: spectrum request resolution + SpectrumControl/SpectrumTap"
```

---

## Task 4: Telemetry event + proto conversion

**Files:**
- Modify: `crates/omnimodem/src/core/event.rs`
- Modify: `crates/omnimodem/src/grpc/convert.rs`

- [x] **Step 1: Add the variant.** In `core/event.rs`, inside `enum TelemetryEvent`, after `ChannelMetrics { … }`:

```rust
    /// One waterfall line (LOSSY): quantized dBFS bins over the passband.
    SpectrumFrame {
        channel: ChannelId,
        timestamp_ns: u64,
        freq_start_hz: f32,
        freq_step_hz: f32,
        db_floor: f32,
        db_ceiling: f32,
        bins: Vec<u8>,
    },
```

- [x] **Step 2: Map to proto.** In `convert.rs` `telemetry_event_to_proto`, add a match arm before the closing `};`:

```rust
        TelemetryEvent::SpectrumFrame {
            channel,
            timestamp_ns,
            freq_start_hz,
            freq_step_hz,
            db_floor,
            db_ceiling,
            bins,
        } => Kind::SpectrumFrame(proto::SpectrumFrame {
            channel: channel.0,
            timestamp_ns,
            freq_start_hz,
            freq_step_hz,
            db_floor,
            db_ceiling,
            bins,
        }),
```

- [x] **Step 3: Build:**

Run: `cargo check -p omnimodem`
Expected: builds (the `match` is now exhaustive over the new variant).

- [x] **Step 4: Commit**

```bash
git add crates/omnimodem/src/core/event.rs crates/omnimodem/src/grpc/convert.rs
git commit -m "core+grpc: TelemetryEvent::SpectrumFrame and proto conversion"
```

---

## Task 5: Command + core handler

Wire `ConfigureSpectrum` from the async edge into the sync core: store each channel's capture rate at `ConfigureAudio`, resolve on `ConfigureSpectrum`, publish to the per-channel `SpectrumControl`, reply with actuals.

**Files:**
- Modify: `crates/omnimodem/src/core/command.rs`
- Modify: `crates/omnimodem/src/core/mod.rs`

- [x] **Step 1: Add the command + reply struct** to `command.rs`. Add to `enum Command` (after `SetAudioGain`):

```rust
    /// Enable/disable + size a channel's spectrum stream.
    ConfigureSpectrum {
        channel: ChannelId,
        enable: bool,
        params: crate::core::SpectrumParams,
        reply: oneshot::Sender<Result<ConfigureSpectrumOk, CoreError>>,
    },
```

And add the reply struct near `ConfigureAudioOk`:

```rust
/// Resolved spectrum sizing echoed back to the client.
#[derive(Debug, Clone, Copy)]
pub struct ConfigureSpectrumOk {
    pub bin_count: u32,
    pub fft_size: u32,
    pub rate_hz: u32,
    pub freq_start_hz: f32,
    pub freq_step_hz: f32,
}
```

Add `use crate::core::SpectrumParams;`-style path or reference it fully-qualified as above. Ensure `command.rs` can name `SpectrumParams` (it is re-exported from `core` in Task 3 Step 1).

- [x] **Step 2: Add per-channel state** in `core/mod.rs`. In the struct holding `gains` (the `live` state, around line 129), add:

```rust
    /// Per-channel shared spectrum control, cloned into the RX workers.
    controls: HashMap<ChannelId, SpectrumControl>,
    /// Per-channel capture (RX) rate, recorded at ConfigureAudio; needed to
    /// resolve spectrum frequency math.
    rx_rates: HashMap<ChannelId, u32>,
```

Initialize both to `HashMap::new()` wherever `gains` is initialized. Add `SpectrumControl` (and `ResolvedSpectrumCfg`, `resolve`) to the `use spectrum::...` re-export from Task 3.

- [x] **Step 3: Record the capture rate** in the `ConfigureAudio` handler arm (around line 215). After audio is configured and `rx_rate` is known (the value placed in `ConfigureAudioOk.rx_rate`), add:

```rust
            live.rx_rates.insert(id, ok.rx_rate); // for spectrum resolution
```

(Use the actual local variable name the arm assigns the rate to; grep the arm for `rx_rate`.)

- [x] **Step 4: Handle the command.** Add a new arm after `Command::SetAudioGain { .. }`:

```rust
        Command::ConfigureSpectrum { channel, enable, params, reply } => {
            let res = if !supervisor.has_channel(channel) {
                Err(CoreError::UnknownChannel(channel))
            } else if !enable {
                live.controls.entry(channel).or_default().set(None);
                Ok(ConfigureSpectrumOk { bin_count: 0, fft_size: 0, rate_hz: 0,
                    freq_start_hz: 0.0, freq_step_hz: 0.0 })
            } else {
                match live.rx_rates.get(&channel).copied() {
                    None => Err(CoreError::UnknownChannel(channel)), // audio not configured
                    Some(rate) => {
                        let cfg = spectrum::resolve(params, rate);
                        let ok = ConfigureSpectrumOk {
                            bin_count: cfg.bin_count as u32,
                            fft_size: cfg.nfft as u32,
                            rate_hz: cfg.rate_hz,
                            freq_start_hz: cfg.freq_start_hz,
                            freq_step_hz: cfg.freq_step_hz,
                        };
                        live.controls.entry(channel).or_default().set(Some(cfg));
                        Ok(ok)
                    }
                }
            };
            let _ = reply.send(res);
        }
```

Import `ConfigureSpectrumOk` (`use crate::core::command::ConfigureSpectrumOk;`) at the top of `mod.rs` if commands are not glob-imported.

- [x] **Step 5: Build:**

Run: `cargo check -p omnimodem`
Expected: builds.

- [x] **Step 6: Commit**

```bash
git add crates/omnimodem/src/core/command.rs crates/omnimodem/src/core/mod.rs
git commit -m "core: ConfigureSpectrum command + handler (resolve, publish control)"
```

---

## Task 6: Worker integration

Clone the per-channel `SpectrumControl` into the RX workers and run the tap on raw capture samples.

**Files:**
- Modify: `crates/omnimodem/src/core/rx_worker.rs`
- Modify: `crates/omnimodem/src/core/mod.rs` (spawn sites)

- [x] **Step 1: Add a `control` param** to `RxWorker::spawn_streaming` (after `gain`):

```rust
        gain: crate::core::AudioGain,
        control: crate::core::SpectrumControl,
```

Inside the thread closure, before the `loop`:

```rust
                let mut tap = crate::core::SpectrumTap::new(channel, telemetry.clone());
```

In the streaming loop, right after `let mut samples = resample(&mut resampler, to_f32(&chunk));`, tap the **raw, pre-resample** capture samples (compute once to avoid converting twice):

Change:
```rust
                            let mut samples = resample(&mut resampler, to_f32(&chunk));
```
to:
```rust
                            let raw = to_f32(&chunk);
                            tap.process(&control, &raw); // waterfall: raw capture-rate samples
                            let mut samples = resample(&mut resampler, raw);
```

- [x] **Step 2: Do the same for `spawn_windowed`** — add the same `control` param, create `let mut tap = ...` before the loop, and in its loop replace `let mut chunk_samples = resample(&mut resampler, to_f32(&chunk));` with:

```rust
                            let raw = to_f32(&chunk);
                            tap.process(&control, &raw);
                            let mut chunk_samples = resample(&mut resampler, raw);
```

- [x] **Step 3: Pass the control at both spawn sites** in `core/mod.rs` (the `spawn_streaming` call ~line 423 and `spawn_windowed` ~line 430). Before spawning, alongside `let gain = live.gains.entry(channel).or_default().clone();` (~line 409) add:

```rust
    let control = live.controls.entry(channel).or_default().clone();
```

Add `control.clone()` as the new trailing argument to each spawn call, matching the param added in Steps 1–2.

- [x] **Step 4: Build:**

Run: `cargo check -p omnimodem`
Expected: builds; both spawn calls pass `control.clone()`.

- [x] **Step 5: Commit**

```bash
git add crates/omnimodem/src/core/rx_worker.rs crates/omnimodem/src/core/mod.rs
git commit -m "core: run SpectrumTap in RX workers on raw capture samples"
```

---

## Task 7: gRPC service handler

**Files:**
- Modify: `crates/omnimodem/src/grpc/service.rs`

- [x] **Step 1: Add the handler** (after `set_audio_gain`, mirroring it):

```rust
    async fn configure_spectrum(
        &self,
        request: Request<proto::ConfigureSpectrumRequest>,
    ) -> Result<Response<proto::ConfigureSpectrumResponse>, Status> {
        let req = request.into_inner();
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::ConfigureSpectrum {
            channel: ChannelId(req.channel),
            enable: req.enable,
            params: crate::core::SpectrumParams {
                bin_count: req.bin_count,
                fft_size: req.fft_size,
                rate_hz: req.rate_hz,
                freq_lo_hz: req.freq_lo_hz,
                freq_hi_hz: req.freq_hi_hz,
            },
            reply: tx,
        })?;
        let ok = rx
            .await
            .map_err(|_| Status::unavailable("core dropped reply"))?
            .map_err(core_error_to_status)?;
        Ok(Response::new(proto::ConfigureSpectrumResponse {
            bin_count: ok.bin_count,
            fft_size: ok.fft_size,
            rate_hz: ok.rate_hz,
            freq_start_hz: ok.freq_start_hz,
            freq_step_hz: ok.freq_step_hz,
        }))
    }
```

- [x] **Step 2: Build:**

Run: `cargo check -p omnimodem`
Expected: builds; `ModemControl` trait is fully implemented.

- [x] **Step 3: Commit**

```bash
git add crates/omnimodem/src/grpc/service.rs
git commit -m "grpc: configure_spectrum handler"
```

---

## Task 8: End-to-end test + full gate

**Files:**
- Create/Modify: a test under `crates/omnimodem/tests/` (e.g. `spectrum.rs`) following the existing `subscribe.rs` / `e2e.rs` harness (file backend capture, in-process server).

- [x] **Step 1: Write the failing test.** Model it on `tests/subscribe.rs`: start the in-process service, `ConfigureChannel`, `ConfigureAudio` with the file/loopback backend, `ConfigureSpectrum{enable:true, bin_count:64, fft_size:512, rate_hz:10, freq_hi_hz:3000}`, subscribe, and assert at least one `Event::SpectrumFrame` arrives with `bins.len() == response.bin_count` and `freq_step_hz > 0`. Reuse whatever capture-injection helper `subscribe.rs`/`e2e.rs` use; do not invent a new backend.

- [x] **Step 2: Run it, expect FAIL → then PASS** once wiring is correct:

Run: `cargo test -p omnimodem --test spectrum`
Expected: PASS (frames flow end to end).

- [x] **Step 3: Full gate:**

Run: `cargo test --workspace` then `cargo clippy --workspace --all-targets -- -D warnings`
Expected: all green. Fix any clippy nits (e.g. `too_many_arguments` on the spawners already has `#[allow]`; add it if the new param trips it).

- [x] **Step 4: Commit**

```bash
git add crates/omnimodem/tests/spectrum.rs
git commit -m "test: end-to-end SpectrumFrame streaming"
```

---

## Self-review checklist (run before opening the PR)

- Proto is additive: new message/RPC, `Event` tag 13, no renumbering. PR body states "additive within v1".
- `bins.len()` always equals the resolved `bin_count`; `freq_start_hz`/`freq_step_hz` in the frame match the `ConfigureSpectrum` response.
- Spectrum is OFF until `ConfigureSpectrum{enable:true}`; `enable:false` drops the tap (no FFT cost).
- The demod sample path is unchanged (we tap `to_f32(&chunk)` and still resample the same data).
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` both pass.
