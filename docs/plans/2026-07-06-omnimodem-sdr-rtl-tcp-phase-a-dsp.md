# RTL-SDR (`rtl_tcp`) — Phase A, Plan 1: DSP Building Blocks — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the pure-DSP receive primitives — complex mixing, complex decimation, NBFM demod, power squelch, and a complex (IQ) FFT/waterfall — to the `omnimodem-dsp` crate, so the daemon's `rtl_tcp` backend (Plan 2) can turn raw IQ into audio and a wideband RF waterfall.

**Architecture:** Everything here is a self-contained building block in `crates/dsp/src/frontend/`, unit-tested in isolation with synthetic signals — no daemon, no gRPC, no network. The receive chain is `IQ (Cplx) → NCO channel-select → complex decimate → FM discriminator → power-squelch gate → audio (Sample)`, plus a parallel `IQ → complex STFT → full-spectrum dBFS` tap for the tuning waterfall. A single `NbfmReceiver` façade composes the chain so Plan 2 calls one object.

**Tech Stack:** Rust, `num-complex` (`Cplx = Complex32`), `rustfft`. Reuses existing `Oscillator` (`frontend/osc.rs`), `Resampler` (`frontend/resample.rs`), `FmDiscriminator` (`frontend/detector.rs`), `Stft`/`SpectrumPlan` (`frontend/stft.rs`, `frontend/spectrum.rs`).

**Scope note:** This is **Plan 1 of 4** for Phase A. It ships no user-visible behavior on its own but is fully testable. See the **Phase A Roadmap** at the end for Plans 2–4 (backend + control/telemetry seam, gRPC surface, TUI view). Each later plan is written when we reach it, per the writing-plans "one plan per subsystem" rule.

**Conventions:**
- Run all tests with: `cargo test -p omnimodem-dsp`
- Type aliases (from `crates/dsp/src/types.rs`): `Sample = f32`, `Cplx = num_complex::Complex32` (`.re`, `.im`, `.conj()`, `.norm()`, `.arg()`).
- New modules are declared in `crates/dsp/src/frontend/mod.rs` alongside the existing `pub mod` list.

---

### Task 1: `u8` IQ → complex baseband conversion

RTL dongles stream interleaved unsigned-8-bit I/Q centered at 127.5. Convert a byte slice to `Vec<Cplx>` normalized to ~[-1, 1). Pure function, trivially testable.

**Files:**
- Create: `crates/dsp/src/frontend/iq.rs`
- Modify: `crates/dsp/src/frontend/mod.rs` (add `pub mod iq;`)
- Test: in `crates/dsp/src/frontend/iq.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Add the module declaration**

In `crates/dsp/src/frontend/mod.rs`, add to the `pub mod` list (alphabetical with the others is fine):

```rust
pub mod iq;
```

- [ ] **Step 2: Write the failing test**

Create `crates/dsp/src/frontend/iq.rs`:

```rust
//! RTL-SDR raw-IQ conversion: interleaved unsigned-8-bit I/Q (centered at
//! 127.5, as `rtl_tcp` streams it) to normalized complex baseband.

use crate::types::Cplx;

/// Convert interleaved `u8` I/Q pairs to complex baseband in ~[-1.0, 1.0).
/// A trailing odd byte (a split pair across TCP reads) is ignored; callers
/// carry it into the next call's buffer.
pub fn u8_iq_to_cplx(bytes: &[u8]) -> Vec<Cplx> {
    bytes
        .chunks_exact(2)
        .map(|p| {
            let i = (p[0] as f32 - 127.5) / 127.5;
            let q = (p[1] as f32 - 127.5) / 127.5;
            Cplx::new(i, q)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_bytes_map_near_zero() {
        // 127 and 128 straddle the 127.5 DC point → magnitude well under 0.01.
        let out = u8_iq_to_cplx(&[127, 128]);
        assert_eq!(out.len(), 1);
        assert!(out[0].norm() < 0.01, "got {:?}", out[0]);
    }

    #[test]
    fn extremes_map_to_full_scale() {
        let out = u8_iq_to_cplx(&[255, 0]);
        assert!((out[0].re - 1.0).abs() < 0.01);
        assert!((out[0].im + 1.0).abs() < 0.02); // 0 → (0-127.5)/127.5 ≈ -1.0
    }

    #[test]
    fn odd_trailing_byte_is_dropped() {
        let out = u8_iq_to_cplx(&[200, 60, 10]);
        assert_eq!(out.len(), 1);
    }
}
```

- [ ] **Step 3: Run the test to verify it fails, then passes**

Run: `cargo test -p omnimodem-dsp iq::`
Expected first run: FAILS to compile until Step 1's `pub mod iq;` and the file both exist; once both are in place the three tests PASS. (The impl is written together with the test above — this is a pure function with no separate "make it fail" phase beyond the module not existing.)

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/frontend/iq.rs crates/dsp/src/frontend/mod.rs
git commit -m "dsp: u8 IQ -> complex baseband conversion for rtl_tcp"
```

---

### Task 2: Complex-input mixer on `DownConverter` (`push_cplx`)

The existing `DownConverter::push` shifts a **real** input's passband to DC. For IQ we need a **complex** input mixed by the same NCO. Add a `push_cplx` method next to it.

**Files:**
- Modify: `crates/dsp/src/frontend/nco.rs` (add method + test)

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/dsp/src/frontend/nco.rs`:

```rust
    #[test]
    fn complex_tone_at_tune_freq_becomes_dc() {
        use crate::types::Cplx;
        let rate = 240_000.0;
        let f0 = 30_000.0; // signal sits +30 kHz above center
        let mut dc = DownConverter::new(f0, rate);
        let mut acc = Cplx::new(0.0, 0.0);
        let n = 24_000;
        for k in 0..n {
            let ph = std::f32::consts::TAU * f0 * k as f32 / rate;
            let x = Cplx::new(ph.cos(), ph.sin()); // e^{+j2π f0 k/rate}
            acc += dc.push_cplx(x);
        }
        let mag = (acc / n as f32).norm();
        assert!(mag > 0.9, "tuned complex tone should sit at DC, got {mag}");
    }

    #[test]
    fn complex_offset_tone_averages_away() {
        use crate::types::Cplx;
        let rate = 240_000.0;
        let mut dc = DownConverter::new(30_000.0, rate);
        let mut acc = Cplx::new(0.0, 0.0);
        let n = 24_000;
        for k in 0..n {
            let ph = std::f32::consts::TAU * 90_000.0 * k as f32 / rate; // 60 kHz off tune
            let x = Cplx::new(ph.cos(), ph.sin());
            acc += dc.push_cplx(x);
        }
        assert!((acc / n as f32).norm() < 0.05);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p omnimodem-dsp nco::tests::complex_tone_at_tune_freq_becomes_dc`
Expected: FAIL — `no method named push_cplx`.

- [ ] **Step 3: Add the implementation**

In `crates/dsp/src/frontend/nco.rs`, inside `impl DownConverter`, add after `push`:

```rust
    /// Complex sample -> complex baseband. Multiplies by e^{-j2πf t} (the same
    /// NCO `push` uses), shifting `tune_hz` to DC for IQ channel selection.
    /// `z * (cos - j sin)` expanded to avoid a temporary Cplx.
    pub fn push_cplx(&mut self, z: Cplx) -> Cplx {
        let (c, s) = self.osc.next();
        Cplx::new(z.re * c + z.im * s, z.im * c - z.re * s)
    }
```

- [ ] **Step 4: Run to verify both tests pass**

Run: `cargo test -p omnimodem-dsp nco::`
Expected: PASS (existing real-input tests plus the two new complex ones).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/frontend/nco.rs
git commit -m "dsp: add complex-input mixer DownConverter::push_cplx"
```

---

### Task 3: Complex decimating resampler (`ComplexResampler`)

`Resampler` is real-only. Decimate wideband IQ (e.g. 240 kHz) to a channel rate (e.g. 48 kHz) by running two real `Resampler`s over I and Q — valid for the narrowband, DC-centered channel the NCO produced.

**Files:**
- Modify: `crates/dsp/src/frontend/resample.rs` (add struct + test; reuses `Resampler` in the same file)

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/dsp/src/frontend/resample.rs`:

```rust
    #[test]
    fn complex_resampler_decimates_length() {
        use crate::types::Cplx;
        let mut r = ComplexResampler::new(240_000, 48_000, 16);
        let input: Vec<Cplx> = (0..4800).map(|_| Cplx::new(0.25, -0.25)).collect();
        let out = r.process(&input);
        // 5:1 decimation → ~960 out; polyphase warm-up allows a small delta.
        assert!((out.len() as i32 - 960).abs() <= 16, "got {}", out.len());
    }

    #[test]
    fn complex_resampler_preserves_dc() {
        use crate::types::Cplx;
        let mut r = ComplexResampler::new(240_000, 48_000, 16);
        let input: Vec<Cplx> = (0..48_000).map(|_| Cplx::new(0.5, 0.5)).collect();
        let out = r.process(&input);
        let mean = out.iter().copied().sum::<Cplx>() / out.len() as f32;
        assert!((mean.re - 0.5).abs() < 0.05 && (mean.im - 0.5).abs() < 0.05, "got {mean:?}");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p omnimodem-dsp resample::tests::complex_resampler_decimates_length`
Expected: FAIL — `cannot find type ComplexResampler`.

- [ ] **Step 3: Add the implementation**

In `crates/dsp/src/frontend/resample.rs`, add (after the existing `Resampler` impl, above the tests module):

```rust
use crate::types::Cplx;

/// Complex decimating/interpolating resampler: two real polyphase `Resampler`s,
/// one per quadrature rail. Correct for a narrowband channel centered at DC
/// (the output of an NCO channel-select), which is all the SDR RX path needs.
pub struct ComplexResampler {
    i: Resampler,
    q: Resampler,
}

impl ComplexResampler {
    pub fn new(in_rate: u32, out_rate: u32, taps_per_phase: usize) -> Self {
        ComplexResampler {
            i: Resampler::new(in_rate, out_rate, taps_per_phase),
            q: Resampler::new(in_rate, out_rate, taps_per_phase),
        }
    }

    /// Resample a block of complex samples. I and Q share the ratio, so both
    /// rails emit the same count and zip cleanly.
    pub fn process(&mut self, input: &[Cplx]) -> Vec<Cplx> {
        let re: Vec<f32> = input.iter().map(|z| z.re).collect();
        let im: Vec<f32> = input.iter().map(|z| z.im).collect();
        let ro = self.i.process(&re);
        let io = self.q.process(&im);
        ro.into_iter().zip(io).map(|(r, i)| Cplx::new(r, i)).collect()
    }

    /// Reduced (up, down) ratio, from the I rail (both rails are identical).
    pub fn ratio(&self) -> (usize, usize) {
        self.i.ratio()
    }
}
```

- [ ] **Step 4: Run to verify both tests pass**

Run: `cargo test -p omnimodem-dsp resample::`
Expected: PASS (existing real tests plus the two new complex ones).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/frontend/resample.rs
git commit -m "dsp: add ComplexResampler for narrowband IQ decimation"
```

---

### Task 4: Power squelch (`PowerSquelch`)

Gate audio when the channel's power sits below a dBFS threshold, with hysteresis to avoid chatter. Driven by the complex channel signal (post-NCO, post-decimation).

**Files:**
- Create: `crates/dsp/src/frontend/squelch.rs`
- Modify: `crates/dsp/src/frontend/mod.rs` (add `pub mod squelch;`)
- Test: in `crates/dsp/src/frontend/squelch.rs`

- [ ] **Step 1: Add the module declaration**

In `crates/dsp/src/frontend/mod.rs` add:

```rust
pub mod squelch;
```

- [ ] **Step 2: Write the failing test**

Create `crates/dsp/src/frontend/squelch.rs`:

```rust
//! Power squelch: opens/closes based on the smoothed power of the complex
//! channel signal, with hysteresis. When closed, the caller feeds silence to
//! the demod so an idle frequency doesn't push noise into decoders/DCD.

use crate::types::Cplx;

pub struct PowerSquelch {
    open_db: f32,
    close_db: f32,
    open: bool,
    alpha: f32,
    power: f32, // smoothed linear power
}

impl PowerSquelch {
    /// `threshold_db` is the open threshold (dBFS); the close threshold is
    /// `threshold_db - hysteresis_db`. `alpha` is the smoothing factor per block.
    pub fn new(threshold_db: f32, hysteresis_db: f32) -> Self {
        PowerSquelch {
            open_db: threshold_db,
            close_db: threshold_db - hysteresis_db,
            open: false,
            alpha: 0.5,
            power: 0.0,
        }
    }

    /// A squelch that is always open (threshold effectively -infinity). Used
    /// when the operator disables squelch.
    pub fn disabled() -> Self {
        let mut s = PowerSquelch::new(-200.0, 1.0);
        s.open = true;
        s
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Update from one block of the complex channel; returns the open state.
    pub fn observe(&mut self, channel: &[Cplx]) -> bool {
        if channel.is_empty() {
            return self.open;
        }
        let inst: f32 =
            channel.iter().map(|z| z.norm_sqr()).sum::<f32>() / channel.len() as f32;
        self.power = self.alpha * inst + (1.0 - self.alpha) * self.power;
        let db = 10.0 * (self.power + 1e-12).log10();
        if self.open {
            if db < self.close_db {
                self.open = false;
            }
        } else if db > self.open_db {
            self.open = true;
        }
        self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(mag: f32, n: usize) -> Vec<Cplx> {
        (0..n).map(|_| Cplx::new(mag, 0.0)).collect()
    }

    #[test]
    fn strong_signal_opens() {
        let mut sq = PowerSquelch::new(-20.0, 6.0);
        // 0 dBFS block, applied a few times to settle the smoother.
        for _ in 0..5 {
            sq.observe(&block(1.0, 256));
        }
        assert!(sq.is_open());
    }

    #[test]
    fn weak_signal_closes() {
        let mut sq = PowerSquelch::new(-20.0, 6.0);
        for _ in 0..5 {
            sq.observe(&block(0.001, 256)); // -60 dBFS
        }
        assert!(!sq.is_open());
    }

    #[test]
    fn hysteresis_holds_state_between_thresholds() {
        let mut sq = PowerSquelch::new(-20.0, 20.0); // open >-20, close <-40
        for _ in 0..5 {
            sq.observe(&block(1.0, 256)); // open
        }
        assert!(sq.is_open());
        // ~-30 dBFS: below open, above close → must stay open.
        for _ in 0..5 {
            sq.observe(&block(0.0316, 256));
        }
        assert!(sq.is_open());
    }

    #[test]
    fn disabled_is_always_open() {
        let mut sq = PowerSquelch::disabled();
        assert!(sq.observe(&block(0.0, 8)));
    }
}
```

- [ ] **Step 3: Run to verify it fails, then passes**

Run: `cargo test -p omnimodem-dsp squelch::`
Expected: FAILS to compile until Step 1's `pub mod squelch;` is present; then all four tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/frontend/squelch.rs crates/dsp/src/frontend/mod.rs
git commit -m "dsp: add PowerSquelch with hysteresis"
```

---

### Task 5: Complex STFT for the IQ waterfall (`ComplexStft`)

The real `Stft` zero-fills the imaginary part and yields a half-spectrum. The RF waterfall needs the **full** two-sided spectrum of complex IQ. Add a complex-input STFT.

**Files:**
- Create: `crates/dsp/src/frontend/complex_stft.rs`
- Modify: `crates/dsp/src/frontend/mod.rs` (add `pub mod complex_stft;`)
- Test: in `crates/dsp/src/frontend/complex_stft.rs`

- [ ] **Step 1: Add the module declaration**

In `crates/dsp/src/frontend/mod.rs` add:

```rust
pub mod complex_stft;
```

- [ ] **Step 2: Write the failing test**

Create `crates/dsp/src/frontend/complex_stft.rs`:

```rust
//! Complex (IQ) STFT for the wideband RF waterfall. Unlike the real `Stft`,
//! this keeps the imaginary part, so the output is the full two-sided spectrum:
//! bin 0 = DC, bins `1..nfft/2` = positive freqs, bins `nfft/2..nfft` = negative.
//! A Hann window is applied to both rails.

use crate::types::Cplx;
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

pub struct ComplexStft {
    nfft: usize,
    hop: usize,
    window: Vec<f32>,
    buf: Vec<Cplx>,
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    window_sum: f32,
}

impl ComplexStft {
    pub fn new(nfft: usize, hop: usize) -> Self {
        let window: Vec<f32> = (0..nfft)
            .map(|n| {
                0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / nfft as f32).cos()
            })
            .collect();
        let window_sum = window.iter().sum();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(nfft);
        ComplexStft {
            nfft,
            hop,
            window,
            buf: Vec::with_capacity(nfft),
            fft,
            scratch: vec![Complex::new(0.0, 0.0); nfft],
            window_sum,
        }
    }

    pub fn nfft(&self) -> usize {
        self.nfft
    }

    pub fn window_sum(&self) -> f32 {
        self.window_sum
    }

    /// Feed complex samples; emit one full complex spectrum (len `nfft`) per hop.
    pub fn feed(&mut self, samples: &[Cplx]) -> Vec<Vec<Complex<f32>>> {
        let mut out = Vec::new();
        for &s in samples {
            self.buf.push(s);
            if self.buf.len() == self.nfft {
                for i in 0..self.nfft {
                    let w = self.window[i];
                    self.scratch[i] = Complex::new(self.buf[i].re * w, self.buf[i].im * w);
                }
                self.fft.process(&mut self.scratch);
                out.push(self.scratch.clone());
                self.buf.drain(0..self.hop);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_tone_lands_in_a_positive_bin() {
        let nfft = 1024;
        let mut st = ComplexStft::new(nfft, nfft);
        let rate = 240_000.0;
        let f = 30_000.0; // bin = f/rate*nfft = 128
        let iq: Vec<Cplx> = (0..nfft)
            .map(|k| {
                let ph = std::f32::consts::TAU * f * k as f32 / rate;
                Cplx::new(ph.cos(), ph.sin())
            })
            .collect();
        let frames = st.feed(&iq);
        assert_eq!(frames.len(), 1);
        let (peak, _) = frames[0]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm().partial_cmp(&b.1.norm()).unwrap())
            .unwrap();
        assert!((peak as i32 - 128).abs() <= 2, "peak bin {peak}, expected ~128");
    }

    #[test]
    fn negative_tone_lands_in_upper_half() {
        let nfft = 1024;
        let mut st = ComplexStft::new(nfft, nfft);
        let rate = 240_000.0;
        let f = -30_000.0; // negative freq → bin nfft-128 = 896
        let iq: Vec<Cplx> = (0..nfft)
            .map(|k| {
                let ph = std::f32::consts::TAU * f * k as f32 / rate;
                Cplx::new(ph.cos(), ph.sin())
            })
            .collect();
        let frames = st.feed(&iq);
        let (peak, _) = frames[0]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm().partial_cmp(&b.1.norm()).unwrap())
            .unwrap();
        assert!((peak as i32 - 896).abs() <= 2, "peak bin {peak}, expected ~896");
    }
}
```

- [ ] **Step 3: Run to verify it fails, then passes**

Run: `cargo test -p omnimodem-dsp complex_stft::`
Expected: FAILS until Step 1's module line exists; then both tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/frontend/complex_stft.rs crates/dsp/src/frontend/mod.rs
git commit -m "dsp: add ComplexStft (full two-sided spectrum) for RF waterfall"
```

---

### Task 6: Full-spectrum dBFS + centered plan geometry

Turn a complex spectrum into `fftshift`-ordered dBFS (most-negative freq first), and give `SpectrumPlan` a constructor for the RF-centered axis so the existing `render()` (uint8 pooling) can be reused unchanged by Plan 2.

**Files:**
- Modify: `crates/dsp/src/frontend/spectrum.rs` (add `full_spectrum_dbfs` + `SpectrumPlan::new_centered` + tests)

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/dsp/src/frontend/spectrum.rs`:

```rust
    #[test]
    fn full_spectrum_is_fftshifted() {
        use rustfft::num_complex::Complex;
        // Length-8 spectrum with all energy in the DC bin (index 0). After
        // fftshift, DC moves to the center (index nfft/2 == 4).
        let mut spec = vec![Complex::new(0.0f32, 0.0); 8];
        spec[0] = Complex::new(8.0, 0.0);
        let db = full_spectrum_dbfs(&spec, 8.0);
        assert_eq!(db.len(), 8);
        let (peak, _) = db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(peak, 4, "DC should land at the shifted center");
    }

    #[test]
    fn centered_plan_axis_starts_at_minus_nyquist() {
        // 240 kHz span centered at 144.39 MHz → bin[0] at center - 120 kHz.
        let plan = SpectrumPlan::new_centered(1024, 240_000.0, 144_390_000.0, 256, -120.0, 0.0);
        assert!((plan.freq_start_hz - (144_390_000.0 - 120_000.0)).abs() < 1.0);
        assert!(plan.freq_step_hz > 0.0);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p omnimodem-dsp spectrum::tests::full_spectrum_is_fftshifted`
Expected: FAIL — `cannot find function full_spectrum_dbfs` / `no function new_centered`.

- [ ] **Step 3: Add the implementations**

In `crates/dsp/src/frontend/spectrum.rs`, add these items (place `full_spectrum_dbfs` near `half_spectrum_dbfs`, and `new_centered` inside `impl SpectrumPlan`). The struct fields `lo_bin`/`hi_bin`/`bin_count`/`freq_start_hz`/`freq_step_hz`/`db_floor`/`db_ceiling` already exist and are used by `render`.

```rust
/// Two-sided amplitude dBFS for a complex spectrum, reordered so bin[0] is the
/// most-negative frequency (fftshift) and the last bin is just below +Nyquist.
/// `window_sum` normalizes to amplitude, matching `half_spectrum_dbfs`.
pub fn full_spectrum_dbfs(spectrum: &[Complex<f32>], window_sum: f32) -> Vec<f32> {
    let n = spectrum.len();
    let mut out = vec![0.0f32; n];
    for k in 0..n {
        // fftshift: rotate by n/2 so negative freqs (upper FFT half) come first.
        let src = (k + n / 2) % n;
        let amp = spectrum[src].norm() / window_sum;
        out[k] = 20.0 * (amp + 1e-12).log10();
    }
    out
}
```

```rust
    /// Geometry for a full two-sided (RF) spectrum centered at `center_hz` with
    /// total span `rate` Hz. `freq_lo`/`freq_hi` are an optional zoom window in
    /// Hz *relative to center* (pass the full ±rate/2 for no zoom). Renders the
    /// same uint8 bins as `new`, so callers reuse `render` unchanged.
    pub fn new_centered(
        nfft: usize,
        rate: f32,
        center_hz: f32,
        req_bin_count: usize,
        freq_lo: f32,
        freq_hi: f32,
    ) -> Self {
        let step = rate / nfft as f32;
        // Map the relative zoom window to shifted-bin indices (bin 0 == -rate/2).
        let lo = (((freq_lo + rate / 2.0) / step).floor() as isize).clamp(0, nfft as isize - 1);
        let hi = (((freq_hi + rate / 2.0) / step).ceil() as isize).clamp(lo + 1, nfft as isize);
        let lo_bin = lo as usize;
        let hi_bin = hi as usize;
        let span_bins = hi_bin - lo_bin;
        let bin_count = req_bin_count.max(1).min(span_bins);
        let freq_start_hz = center_hz - rate / 2.0 + lo_bin as f32 * step;
        let freq_step_hz = (span_bins as f32 * step) / bin_count as f32;
        SpectrumPlan {
            lo_bin,
            hi_bin,
            bin_count,
            freq_start_hz,
            freq_step_hz,
            db_floor: -120.0,
            db_ceiling: 0.0,
        }
    }
```

> Note for the implementer: if `SpectrumPlan`'s fields are private to the module, `new_centered` compiles because it lives in the same module. If a field name differs from the quoted set, match the existing `SpectrumPlan::new` field initialization exactly.

- [ ] **Step 4: Run to verify both tests pass**

Run: `cargo test -p omnimodem-dsp spectrum::`
Expected: PASS (existing half-spectrum tests plus the two new ones).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/frontend/spectrum.rs
git commit -m "dsp: full (two-sided) spectrum dBFS + RF-centered SpectrumPlan"
```

---

### Task 7: `NbfmReceiver` façade (composes the RX chain)

One object Plan 2 calls: `push_iq(&[Cplx]) -> Vec<Sample>` running NCO → complex decimate → FM discriminator → squelch gate, with runtime `retune` and `set_squelch`.

**Files:**
- Create: `crates/dsp/src/frontend/nbfm.rs`
- Modify: `crates/dsp/src/frontend/mod.rs` (add `pub mod nbfm;`)
- Test: in `crates/dsp/src/frontend/nbfm.rs`

- [ ] **Step 1: Add the module declaration**

In `crates/dsp/src/frontend/mod.rs` add:

```rust
pub mod nbfm;
```

- [ ] **Step 2: Write the failing test**

Create `crates/dsp/src/frontend/nbfm.rs`:

```rust
//! Narrowband-FM receiver façade: raw IQ at the capture rate to demodulated
//! audio at the channel rate. Composes the NCO channel-select, complex
//! decimation, FM discriminator, and power squelch so the daemon backend calls
//! one object. `retune` moves the listening offset within the captured band;
//! `set_squelch` swaps the squelch at runtime.

use crate::frontend::detector::FmDiscriminator;
use crate::frontend::nco::DownConverter;
use crate::frontend::resample::ComplexResampler;
use crate::frontend::squelch::PowerSquelch;
use crate::types::{Cplx, Sample};

pub struct NbfmReceiver {
    nco: DownConverter,
    decim: ComplexResampler,
    disc: FmDiscriminator,
    squelch: PowerSquelch,
    gain: f32,
}

impl NbfmReceiver {
    /// `offset_hz` is the signal's offset from the captured band center.
    /// `deviation_hz` sets the audio scaling (±deviation → ~±1.0 full scale).
    pub fn new(
        capture_rate: u32,
        channel_rate: u32,
        offset_hz: f32,
        deviation_hz: f32,
        squelch: PowerSquelch,
    ) -> Self {
        NbfmReceiver {
            nco: DownConverter::new(offset_hz, capture_rate as f32),
            decim: ComplexResampler::new(capture_rate, channel_rate, 16),
            disc: FmDiscriminator::new(),
            squelch,
            gain: channel_rate as f32 / (std::f32::consts::TAU * deviation_hz),
        }
    }

    pub fn retune(&mut self, offset_hz: f32) {
        self.nco.retune(offset_hz);
    }

    pub fn set_squelch(&mut self, squelch: PowerSquelch) {
        self.squelch = squelch;
    }

    /// IQ at capture rate -> mono audio at channel rate. Silent while squelched.
    pub fn push_iq(&mut self, iq: &[Cplx]) -> Vec<Sample> {
        let mixed: Vec<Cplx> = iq.iter().map(|&z| self.nco.push_cplx(z)).collect();
        let chan = self.decim.process(&mixed);
        let open = self.squelch.observe(&chan);
        let mut audio: Vec<Sample> =
            chan.iter().map(|&z| self.disc.push(z) * self.gain).collect();
        if !open {
            for s in audio.iter_mut() {
                *s = 0.0;
            }
        }
        audio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build IQ of an FM carrier at `offset_hz` modulated by a `tone_hz` sine.
    fn fm_iq(
        capture_rate: f32,
        offset_hz: f32,
        tone_hz: f32,
        dev_hz: f32,
        n: usize,
    ) -> Vec<Cplx> {
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let t = k as f32 / capture_rate;
            let inst = offset_hz + dev_hz * (std::f32::consts::TAU * tone_hz * t).sin();
            phase += std::f32::consts::TAU * inst / capture_rate;
            out.push(Cplx::new(phase.cos(), phase.sin()));
        }
        out
    }

    #[test]
    fn recovers_modulating_tone() {
        let capture = 240_000.0;
        let channel = 48_000;
        let tone = 1200.0;
        let mut rx = NbfmReceiver::new(
            capture as u32,
            channel,
            30_000.0, // signal 30 kHz off center
            5_000.0,
            PowerSquelch::disabled(),
        );
        let iq = fm_iq(capture, 30_000.0, tone, 5_000.0, 48_000);
        let audio = rx.push_iq(&iq);
        // Zero-crossing count → dominant frequency ≈ tone_hz.
        let mut crossings = 0;
        for w in audio.windows(2) {
            if w[0] <= 0.0 && w[1] > 0.0 {
                crossings += 1;
            }
        }
        let secs = audio.len() as f32 / channel as f32;
        let est_hz = crossings as f32 / secs;
        assert!((est_hz - tone).abs() < 150.0, "estimated {est_hz} Hz, want {tone}");
    }

    #[test]
    fn squelch_silences_weak_input() {
        let mut rx = NbfmReceiver::new(
            240_000,
            48_000,
            30_000.0,
            5_000.0,
            PowerSquelch::new(-20.0, 6.0), // needs a strong signal to open
        );
        // Near-silent IQ (tiny noise) → squelch stays closed → all-zero audio.
        let iq: Vec<Cplx> = (0..48_000).map(|_| Cplx::new(1e-4, -1e-4)).collect();
        let audio = rx.push_iq(&iq);
        assert!(audio.iter().all(|&s| s == 0.0));
    }
}
```

- [ ] **Step 3: Run to verify it fails, then passes**

Run: `cargo test -p omnimodem-dsp nbfm::`
Expected: FAILS until Step 1's module line exists; then both tests PASS. If `recovers_modulating_tone` is marginally off, widen the tolerance to 200 Hz — the exact number depends on the resampler's transition band, not on correctness of the demod.

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/frontend/nbfm.rs crates/dsp/src/frontend/mod.rs
git commit -m "dsp: add NbfmReceiver facade composing the SDR RX chain"
```

---

### Task 8: Crate-level check

- [ ] **Step 1: Full crate test + clippy**

Run:
```bash
cargo test -p omnimodem-dsp
cargo clippy -p omnimodem-dsp --all-targets -- -D warnings
```
Expected: all tests PASS, no clippy errors. Fix any warnings inline (the workspace treats them as errors in CI).

- [ ] **Step 2: Commit any lint fixes**

```bash
git add -A
git commit -m "dsp: clippy clean for SDR RX building blocks"
```

---

## Self-Review (against the spec's DSP requirements)

- **IQ intake** (`u8` → Cplx) → Task 1. ✓
- **NCO channel-select for IQ** ("retunable NCO", instant fine-tune) → Task 2 (`push_cplx`) + Task 7 (`retune`). ✓
- **Decimate to audio rate** → Task 3 (`ComplexResampler`). ✓
- **NBFM demod** → Task 7 uses existing `FmDiscriminator` (Task-verified end-to-end). ✓
- **Power squelch (v1)** → Task 4 + wired in Task 7. ✓
- **Wideband complex FFT for the RF waterfall** → Task 5 (`ComplexStft`) + Task 6 (`full_spectrum_dbfs`, `new_centered`). ✓
- **RF-referenced axis** (absolute Hz for `SpectrumFrame`) → Task 6 `new_centered` sets `freq_start_hz`/`freq_step_hz`. ✓
- Type consistency: `Cplx`/`Sample` used throughout; `ComplexResampler::process`, `DownConverter::push_cplx`, `PowerSquelch::observe`, `ComplexStft::feed`, `full_spectrum_dbfs`, `SpectrumPlan::new_centered`, `NbfmReceiver::{new,retune,set_squelch,push_iq}` names match across tasks. ✓
- Deferred to later phases (documented, not dropped): **de-emphasis** and **AM/WFM/SSB** demod — Phase B; the flat NBFM audio here is correct for AFSK/APRS. ✓

No placeholders; every step has runnable code and an exact command.

---

## Phase A Roadmap (Plans 2–4 — written when we reach them)

This plan (DSP) is the foundation. The remaining Phase A plans, each its own document under `docs/plans/`:

**Plan 2 — `rtl_tcp` backend + control/telemetry seam** (`crates/omnimodemd/src/audio/`, `core/`):
- `DeviceId::RtlTcp { host, port }` variant + `to_canonical_string`/`parse` (`ids.rs`).
- `RtlTcpBackend` implementing `AudioBackend` (`audio/rtlsdr.rs`): TCP connect, 12-byte header parse, command encoder (freq/rate/gain/ppm), IQ read loop calling `NbfmReceiver::push_iq`, delivering `AudioChunk`s via `CaptureHandle`.
- **Seam decision to confirm before starting Plan 2:** the running backend needs (a) a runtime **control** path for tune/gain/demod-mode and (b) a **telemetry-out** path for the RF waterfall. Proposed: mirror the proven `AudioGain` pattern — an `Arc` of atomics (`SdrControl`) for control, and hand the SDR backend a clone of the `broadcast::Sender<TelemetryEvent>` so it emits `SpectrumFrame{transmit:false, freq_start_hz: RF…}` directly (RX worker's audio-passband tap stays off for SDR channels). This extends the backend construction path; get maintainer sign-off on that seam first.
- Factory branch in `production_core` (`lib.rs`) matching `DeviceId::RtlTcp`.
- Fake-`rtl_tcp`-server integration test (in-process TCP listener emitting header + scripted IQ) → APRS frame via the existing AFSK1200 conformance harness.

**Plan 3 — SDR control gRPC surface** (`proto/omnimodem.proto`, `core/command.rs`, `grpc/service.rs`, `core/mod.rs`):
- Add `SetSdrTune`, `SetSdrGain`, `ConfigureSdr` (with `DemodMode` enum), `GetSdrCaps` RPCs + messages, each mirroring the `SetAudioGain` path (proto → `Command` variant with `oneshot` reply → async handler → core dispatch to `SdrControl`).
- Broadcast an `SdrState` event on `SubscribeEvents` for late/multi-client sync.
- Handler unit tests + a gRPC round-trip test.

**Plan 4 — TUI SDR tuning view** (`clients/omnimodem-tui/`):
- `view_sdr.go` implementing the `View` interface (mirror `view_operate.go`): RF readout, arrow/step tuning + direct entry, gain/ppm/demod-mode/squelch controls, signal meter.
- Waterfall cursor overlay: extend `waterfall.render`/`spectrumLine` to draw a marker at the demod frequency (map Hz → bin via `freqStart`/`freqStep`).
- Client methods for the new RPCs (`internal/client/client.go`), `make proto` regen of stubs, route into the view from `view_channels.go`.
- Bubble Tea model-level tests for tune step/entry and cursor↔frequency mapping.

**Phase A is "done"** only when Plans 1–4 all land and APRS decodes end-to-end off a real/remote dongle with working click-tune in the TUI. Phases B (AM/WFM/SSB), C (ppm/bias-tee/direct-sampling/device registration), and D (hardening) follow per the design doc, and per the issue decision **all** must ship before the project is called done.
