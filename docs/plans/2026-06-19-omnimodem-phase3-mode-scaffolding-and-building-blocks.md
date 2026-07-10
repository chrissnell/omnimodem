# Omnimodem Phase 3 — Mode Scaffolding & Building-Blocks Toolkit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the mode framework (`Demodulator`/`BlockDemodulator`/`Modulator` traits, `ModeCaps`, a parametric `ModeConfig`, a one-module mode registry, and the `ParallelDemodulator<D>` ensemble) on top of the soft-LLR contract, plus the individually-testable DSP/FEC/framing building blocks the Phase-4 modes (AFSK 1200, FT8, CW, RTTY, PSK31) need — all gated by a conformance harness, shipping **no end-user mode**.

**Architecture:** A new pure library crate `crates/dsp` (`omnimodem-dsp`) holds the building blocks and the mode-API traits, with the soft-LLR (`Llr`) type as the spine between detector/demapper and FEC decoder. It has **no** dependency on `omnimodem`, so it compiles and KAT-tests in isolation on every PR. `omnimodem` gains a thin `mode` module that defines the parametric `ModeConfig`, the registry that maps a config to a boxed demod/mod, and the wiring from a channel's `mode` string into that registry. Phase 3 registers only a `NullMode` framework fixture — the gRPC surface still carries `mode = "none"`; real modes and their parametric proto land in Phase 4.

**Tech Stack:** Rust (edition 2021, workspace), `rustfft` + `num-complex` (STFT/FFT), `thiserror` (error types), `proptest` (property round-trips), `insta` (modulator golden snapshots). Existing crate idioms: hardware/IO behind traits for hardware-free CI; pure functions with inline `#[cfg(test)]` unit tests; integration/KAT tests under `tests/`.

---

## Scope

**In scope (only what the Phase-4 modes need — design §"Building-block groups introduced now"):**

- **Framework:** `Sample`/`Llr`/`SoftBits` soft-information types; payload-agnostic `Frame`; `ModeCaps`; `Demodulator` (streaming) **and** `BlockDemodulator` (windowed multi-pass) **and** `Modulator` traits; parametric `ModeConfig`; one-module registry; `ParallelDemodulator<D>` ensemble + content/offset dedup window.
- **Group A — front-end DSP:** SIMD-friendly FIR; cos/sin oscillator LUT; tunable NCO/down-converter; rational polyphase resampler (upgrade of the Phase-2 linear one); overlapped STFT engine; filter-design toolkit (FIR low/bandpass, RRC, raised-cosine, Gaussian/CPFSK); peak-valley **and** decision-feedback AGC; hard-limiter correlator stage; FM-discriminator + envelope detector; per-bin noise-floor + normalized-SNR reporter; symmetric modulators GFSK/CPFSK, M-FSK tone bank, differential M-PSK, 2-FSK shift, AFSK, CW keyer.
- **Group B — sync:** DPLL clock recovery; DCD scorer with hysteresis; symbol-timing recovery (Gardner/early-late, async start-bit, PSK31 transition-minimum); Costas-loop carrier recovery + AFC; Costas-**array** generator + correlator; known-sequence/sync-word/preamble correlator; candidate finder.
- **Group C — FEC (soft-decision):** CRC library (CRC-16/X.25, CRC-14 `0x6757`); NRZI; Gray; differential; the three scramblers (self-sync G3RUH `x¹⁷+x¹²+1`, frame-reset IL2P `x⁹+x⁴+1`, additive PRBS); GF(256) Reed-Solomon (fcr=0 IL2P **and** fcr=1 FX.25); soft-LLR demapper; LDPC encoder + min-sum/BP decoder + OSD layer (174,91 for FT8); MultiSlicer; frame-dedup across parallel decoders.
- **Group D — source/message/framing:** Varicode (PSK31); Baudot/ITA2 (RTTY); Morse + SOM (CW); HDLC (flag/stuff/destuff/FCS) + AX.25/APRS UI; FX.25 (CTAG + RS wrap of intact HDLC); IL2P; WSJT-X 77-bit message codec + 28-bit callsign compression + callsign hashing + 15-bit grid.
- **Conformance harness (built here, usable in Phase 3):** KAT-vector runner for every coding block; `proptest` round-trip invariants; `insta` modulator golden snapshots; deterministic seeded-AWGN source. Reference-binary interop + BER-vs-SNR sweeps + channel simulators are the Phase-4 *gate* and may stub their fixtures here.

**Out of scope (deferred to Phase 5, per design):** convolutional encoder / soft-Viterbi / puncturing; K=32 Fano; Walsh–Hadamard/FHT; QRA; GF(2⁶) soft-RS; OFDM core + pilot sync + PAPR; vocoder interface; ARQ engine; interleavers beyond what a Phase-4 mode needs; wideband multi-signal decode, multi-pass SIC, and AP decoding (these compose *with* `ParallelDemodulator` but land with FT8's mode work in Phase 4); the Prometheus exporter and per-mode metric *content* (Phase 5). **No end-user mode ships in Phase 3.**

**Note on splitting:** this phase spans several independent subsystems (front-end DSP, sync, FEC, framing, message codecs, framework). They are sequenced as Parts A–H below; each Part produces working, isolated, KAT-tested code and could be reviewed/merged independently. Parts B–E (the pure blocks) have no ordering dependency among themselves and can be parallelized across workers; Part A (framework types) must land first, and Parts F (framing, depends on C's CRC/RS), G (message codec), and H (conformance + wiring) come after the blocks they assemble.

---

## Conventions locked here (referenced by every later task)

- **`Sample`** = `f32`, real audio normalized to `[-1.0, 1.0)`.
- **`Llr`** = `f32`, a log-likelihood ratio defined as `L = ln( P(bit = 0) / P(bit = 1) )`. **Positive ⇒ bit 0 is more likely; negative ⇒ bit 1.** A hard slice is `bit = (L < 0.0)`. `0.0` is an erasure. This single convention is used by the demapper, every FEC decoder, and the soft-combiner — do not flip it per block.
- **Working rate.** Each mode declares its native rate in `ModeCaps`; the resampler bridges the capture rate (≤ 48 kHz, Phase-2 cap) to it. FT8 = 12 000 Hz; AFSK 1200 = 48 000 Hz; PSK31/RTTY/CW = 8 000 Hz unless a mode overrides.
- **Bit order.** All framing/FEC blocks document and test LSB-first vs MSB-first explicitly (Direwolf/AX.25 is LSB-first on the wire; WSJT-X 77-bit is MSB-first big-endian into the LDPC). A round-trip test that does not assert bit order is incomplete.
- **No allocation on the streaming hot path.** `Demodulator::feed` may allocate the returned `Vec<Frame>` (rare events) but must not allocate per-sample; scratch buffers are owned by the demod and reused. Asserted by a `criterion` allocation guard in Part H.

---

## File Structure

New crate **`crates/dsp`** (`omnimodem-dsp`) — pure, no `omnimodem` dependency:

```
crates/dsp/Cargo.toml                 # lib crate; rustfft, num-complex, thiserror; dev: proptest, insta
crates/dsp/src/lib.rs                 # re-exports; module tree; crate docs
crates/dsp/src/types.rs               # Sample, Llr, SoftBits, Frame, FramePayload, FrameMeta, Cplx alias
crates/dsp/src/mode.rs                # ModeCaps, Duplex, DemodShape; Demodulator/BlockDemodulator/Modulator traits; ModError
crates/dsp/src/ensemble.rs            # ParallelDemodulator<D>, DedupWindow (content+offset)

crates/dsp/src/frontend/mod.rs
crates/dsp/src/frontend/fir.rs        # 8-accumulator FIR; filter-design (lowpass/bandpass/RRC/raised-cosine/gaussian)
crates/dsp/src/frontend/osc.rs        # 256-entry f32 cos/sin LUT oscillator
crates/dsp/src/frontend/nco.rs        # tunable NCO + complex down-converter
crates/dsp/src/frontend/resample.rs   # polyphase rational resampler (upgrade of audio::resample)
crates/dsp/src/frontend/stft.rs       # overlapped STFT/FFT engine (rustfft), window/hop config
crates/dsp/src/frontend/agc.rs        # PeakValleyAgc + DecisionFeedbackAgc
crates/dsp/src/frontend/limiter.rs    # hard-limiter correlator stage
crates/dsp/src/frontend/detector.rs   # FM discriminator, envelope detector + adaptive squelch
crates/dsp/src/frontend/noise.rs      # per-bin noise floor + normalized SNR reporter
crates/dsp/src/frontend/modulate.rs   # Gfsk, MFsk, DiffPsk, Fsk2, Afsk, CwKeyer modulators

crates/dsp/src/sync/mod.rs
crates/dsp/src/sync/dpll.rs           # DpllClockRecovery (locked/searching inertia)
crates/dsp/src/sync/dcd.rs            # DcdScorer (shift-register popcount hysteresis)
crates/dsp/src/sync/timing.rs         # Gardner, early-late, async start-bit, transition-minimum
crates/dsp/src/sync/costas.rs         # Costas-loop carrier recovery + AFC
crates/dsp/src/sync/costas_array.rs   # Costas-array generator + correlator (parametric N)
crates/dsp/src/sync/syncword.rs       # known-sequence/preamble correlator (Hamming-fuzzy)
crates/dsp/src/sync/candidate.rs      # candidate finder (freq,time,metric) sorted list

crates/dsp/src/fec/mod.rs
crates/dsp/src/fec/crc.rs             # parametric CRC; CRC-16/X.25, CRC-14 0x6757
crates/dsp/src/fec/nrzi.rs            # NRZI encode/decode
crates/dsp/src/fec/gray.rs            # Gray encode/decode; differential
crates/dsp/src/fec/scramble.rs        # SelfSyncScrambler (G3RUH), FrameResetScrambler (IL2P), AdditivePrbs
crates/dsp/src/fec/rs.rs              # GF(256) Reed-Solomon, parametric (nroots, fcr, prim)
crates/dsp/src/fec/llr.rs             # soft-LLR demapper (tone power / phase -> per-bit Llr)
crates/dsp/src/fec/ldpc.rs           # LDPC encode + min-sum/BP decode (parametric H); FT8 (174,91) matrices
crates/dsp/src/fec/osd.rs            # ordered-statistics decode layer over an Llr codeword
crates/dsp/src/fec/slicer.rs         # MultiSlicer (geometric space-gain table)

crates/dsp/src/framing/mod.rs
crates/dsp/src/framing/varicode.rs    # Varicode with pluggable table (PSK)
crates/dsp/src/framing/baudot.rs      # Baudot/ITA2 LTRS/FIGS
crates/dsp/src/framing/morse.rs       # Morse table + SOM/fuzzy best-fit
crates/dsp/src/framing/hdlc.rs        # HDLC flag/stuff/destuff/FCS
crates/dsp/src/framing/ax25.rs        # AX.25/APRS UI-frame convention
crates/dsp/src/framing/fx25.rs        # FX.25 CTAG table + RS wrap of intact HDLC
crates/dsp/src/framing/il2p.rs        # IL2P sync + transposed header + per-block RS
crates/dsp/src/framing/message77.rs   # WSJT-X 77-bit codec + callsign compress/hash + 15-bit grid

crates/dsp/tests/kat.rs               # KAT-vector conformance runner (Layer 1)
crates/dsp/tests/roundtrip.rs         # proptest round-trip invariants (Layer 3)
crates/dsp/tests/snapshots.rs         # insta modulator golden snapshots (Layer 1)
crates/dsp/tests/vectors/             # checked-in KAT vector files (hex/json) + insta .snap files
crates/dsp/src/testutil.rs            # seeded AWGN source, hex helpers, Watterson stub (cfg feature "testutil")
```

Changes to **`crates/omnimodem`**:

```
crates/omnimodem/Cargo.toml          # add `omnimodem-dsp = { path = "../dsp" }`
crates/omnimodem/src/mode/mod.rs     # ModeConfig (parametric enum), ModeKind; re-export dsp mode-api
crates/omnimodem/src/mode/registry.rs# build_demod / build_modulator from ModeConfig; NullMode fixture
crates/omnimodem/src/supervisor/channel.rs  # ChannelConfig.mode: String -> retains string, parsed via mode::parse
crates/omnimodem/src/lib.rs          # `pub mod mode;`
Cargo.toml (workspace)                # add "crates/dsp" member; workspace deps rustfft/num-complex/proptest/insta
```

---

# PART A — Framework: soft-information types, traits, registry, ensemble

## Task 1: Create the `omnimodem-dsp` crate and workspace wiring

**Files:**
- Create: `crates/dsp/Cargo.toml`
- Create: `crates/dsp/src/lib.rs`
- Modify: `Cargo.toml` (workspace members + deps)

- [ ] **Step 1: Add workspace members and shared deps**

In root `Cargo.toml`, add the crate to `members` and the new shared deps:

```toml
[workspace]
members = ["crates/omnimodem", "crates/dsp"]
resolver = "2"

# ...existing [workspace.package] and [workspace.dependencies] unchanged, plus:
[workspace.dependencies]
# ...existing entries...
rustfft = "6"
num-complex = "0.4"
proptest = "1"
insta = { version = "1", features = ["json"] }
```

- [ ] **Step 2: Write the crate manifest**

`crates/dsp/Cargo.toml`:

```toml
[package]
name = "omnimodem-dsp"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rustfft.workspace = true
num-complex.workspace = true
thiserror.workspace = true

[features]
# `testutil` exposes the seeded AWGN / channel fixtures to downstream test code.
testutil = []

[dev-dependencies]
proptest.workspace = true
insta.workspace = true
```

- [ ] **Step 3: Write the module tree**

`crates/dsp/src/lib.rs`:

```rust
//! omnimodem-dsp — mode-agnostic DSP, FEC and framing building blocks.
//!
//! Pure library: no dependency on the daemon. Every block is individually
//! testable and gated by known-answer vectors (see `tests/kat.rs`). The
//! soft-information (`Llr`) type in `types` is the spine between the
//! detector/demapper and the FEC decoder.

pub mod types;
pub mod mode;
pub mod ensemble;

pub mod frontend;
pub mod sync;
pub mod fec;
pub mod framing;

#[cfg(any(test, feature = "testutil"))]
pub mod testutil;

pub use types::{Frame, FrameMeta, FramePayload, Llr, Sample, SoftBits};
pub use mode::{BlockDemodulator, Demodulator, Duplex, DemodShape, ModError, ModeCaps, Modulator};
pub use ensemble::ParallelDemodulator;
```

- [ ] **Step 4: Verify it builds (empty modules will fail until Task 2 — expect unresolved modules)**

Run: `cargo build -p omnimodem-dsp`
Expected: FAIL with "file not found for module `types`" — confirms the crate is wired into the workspace. Proceed to Task 2.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/dsp/Cargo.toml crates/dsp/src/lib.rs
git commit -m "dsp: scaffold omnimodem-dsp building-blocks crate"
```

## Task 2: Soft-information types and the payload-agnostic `Frame`

**Files:**
- Create: `crates/dsp/src/types.rs`

- [ ] **Step 1: Write the failing test (inline)**

Append to `crates/dsp/src/types.rs`:

```rust
//! Core data types shared by every block.
//!
//! `Llr` convention (locked, do not flip per block): `L = ln(P(0)/P(1))`.
//! Positive => bit 0 more likely; `hard()` slices `bit = (L < 0.0)`.

use num_complex::Complex32;

/// Real audio sample, normalized to `[-1.0, 1.0)`.
pub type Sample = f32;

/// Complex baseband sample.
pub type Cplx = Complex32;

/// A per-bit log-likelihood ratio, `ln(P(bit=0)/P(bit=1))`.
pub type Llr = f32;

/// A run of soft bits carried across the demapper -> FEC boundary.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SoftBits(pub Vec<Llr>);

impl SoftBits {
    pub fn len(&self) -> usize { self.0.len() }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
    /// Hard-slice each LLR. `bit = (l < 0.0)`; an exact `0.0` slices to 0.
    pub fn hard(&self) -> Vec<u8> {
        self.0.iter().map(|&l| u8::from(l < 0.0)).collect()
    }
}

/// Payload-agnostic so voice/packet/text/raw modes share one `Frame` type.
#[derive(Debug, Clone, PartialEq)]
pub enum FramePayload {
    /// AX.25/HDLC or other byte-oriented packet.
    Packet(Vec<u8>),
    /// Decoded human-readable text (FT8/PSK31/RTTY/CW).
    Text(String),
    /// Raw packed 77-bit WSJT-X message (10 bytes, last 3 bits zero).
    Message77([u8; 10]),
    /// Opaque vocoder bits (Phase-5 voice modes).
    Vocoder(Vec<u8>),
}

/// Decode metadata. The daemon attaches channel/timestamp downstream; the DSP
/// layer fills the signal-quality fields it measured.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrameMeta {
    pub snr_db: Option<f32>,
    pub freq_offset_hz: Option<f32>,
    pub time_offset_s: Option<f32>,
    /// Which ensemble member / slicer produced this frame.
    pub decoder: Option<String>,
    /// Sample offset of the frame within the fed buffer (dedup key).
    pub sample_offset: u64,
    pub crc_ok: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub payload: FramePayload,
    pub meta: FrameMeta,
}

impl Frame {
    pub fn packet(bytes: Vec<u8>) -> Self {
        Frame { payload: FramePayload::Packet(bytes), meta: FrameMeta::default() }
    }
    pub fn text(s: impl Into<String>) -> Self {
        Frame { payload: FramePayload::Text(s.into()), meta: FrameMeta::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llr_hard_slice_uses_locked_convention() {
        // Positive => 0, negative => 1, zero => 0.
        let sb = SoftBits(vec![3.0, -3.0, 0.0, -0.001]);
        assert_eq!(sb.hard(), vec![0, 1, 0, 1]);
    }

    #[test]
    fn frame_constructors_default_meta() {
        let f = Frame::packet(vec![1, 2, 3]);
        assert!(!f.meta.crc_ok);
        assert_eq!(f.meta.sample_offset, 0);
    }
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cargo test -p omnimodem-dsp types::`
Expected: PASS (2 tests). The `mode`, `ensemble`, etc. modules still don't exist, so a full build fails — that's expected; the `types::` filter compiles only what it needs is not true for a lib, so if the build fails on missing modules, temporarily comment the not-yet-created `pub mod` lines in `lib.rs`, run, then restore. (Cleaner: do Tasks 2–4 back-to-back, then run.)

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/src/types.rs
git commit -m "dsp: soft-information types (Sample, Llr, SoftBits) and payload-agnostic Frame"
```

## Task 3: Mode-API traits — `Demodulator`, `BlockDemodulator`, `Modulator`, `ModeCaps`

**Files:**
- Create: `crates/dsp/src/mode.rs`

- [ ] **Step 1: Write the trait definitions and a unit test for `ModeCaps` shape**

`crates/dsp/src/mode.rs`:

```rust
//! The mode framework: capability descriptor and the three demod/mod shapes.
//!
//! Two demod shapes are first-class (design §"Streaming AND block/windowed"):
//! `Demodulator` for continuous/HDLC modes (`feed(samples) -> Vec<Frame>`) and
//! `BlockDemodulator` for windowed multi-pass modes (FT8/WSPR). A mode
//! implements whichever fits; `ModeCaps::shape` declares which to the runtime.

use crate::types::{Frame, Sample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplex { Half, Full }

/// Which decode shape the runtime must drive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DemodShape {
    /// Feed samples as they arrive; frames come out when found.
    Streaming,
    /// Buffer a time-aligned window of `window_s`, decode every `period_s`.
    Windowed { window_s: f32, period_s: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeCaps {
    /// Native working sample rate in Hz (the resampler bridges to this).
    pub native_rate: u32,
    pub bandwidth_hz: f32,
    pub tx: bool,
    pub duplex: Duplex,
    pub shape: DemodShape,
}

/// Continuous/streaming demodulation.
pub trait Demodulator: Send {
    fn caps(&self) -> ModeCaps;
    /// Consume samples at `caps().native_rate`; return any frames found. Must
    /// not allocate per-sample (reuse owned scratch buffers).
    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame>;
    /// Drop all soft state (DPLL lock, AGC, partial frames).
    fn reset(&mut self);
}

/// Windowed/block multi-pass demodulation (FT8/JS8/WSPR).
pub trait BlockDemodulator: Send {
    fn caps(&self) -> ModeCaps;
    /// Decode one time-aligned window. `window_start_ns` is the wall-clock of
    /// the first sample. May return 0..N decodes (multi-pass internally).
    fn decode_window(&mut self, window: &[Sample], window_start_ns: u64) -> Vec<Frame>;
}

#[derive(Debug, thiserror::Error)]
pub enum ModError {
    #[error("payload not supported by this mode: {0}")]
    UnsupportedPayload(&'static str),
    #[error("message too long for mode: {0}")]
    TooLong(String),
    #[error("encode error: {0}")]
    Encode(String),
}

/// Symmetric transmit side: a frame's payload -> baseband audio at native rate.
pub trait Modulator: Send {
    fn caps(&self) -> ModeCaps;
    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windowed_shape_carries_grid() {
        let caps = ModeCaps {
            native_rate: 12_000,
            bandwidth_hz: 50.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Windowed { window_s: 15.0, period_s: 15.0 },
        };
        match caps.shape {
            DemodShape::Windowed { window_s, .. } => assert_eq!(window_s, 15.0),
            _ => panic!("expected windowed"),
        }
    }
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p omnimodem-dsp mode::`
Expected: PASS (1 test).

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/src/mode.rs
git commit -m "dsp: Demodulator/BlockDemodulator/Modulator traits and ModeCaps"
```

## Task 4: `ParallelDemodulator<D>` ensemble and the content+offset dedup window

**Files:**
- Create: `crates/dsp/src/ensemble.rs`

- [ ] **Step 1: Write the failing test — two members emitting the same frame at near-equal offset dedup to one**

`crates/dsp/src/ensemble.rs`:

```rust
//! Generic "hydra" ensemble (design §"Best-of-class reception, generalized").
//! Runs N demod configurations over the same samples and unions/dedups their
//! frames. The *pattern* is mode-agnostic; the member *profiles* are supplied
//! by the mode's registry module (AFSK Profile A/B, PSK loop bandwidths, ...).

use crate::mode::{Demodulator, ModeCaps};
use crate::types::Frame;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Dedup by `(content-hash, sample-offset)` within a window of `window_samples`
/// (≈3 symbol times — design §"Content+offset frame dedup"). Windowed modes
/// dedup by decoded-message + time-slot instead; that variant lives with the
/// windowed mode, not here.
pub struct DedupWindow {
    window_samples: u64,
    seen: Vec<(u64, u64)>, // (content_hash, offset)
}

impl DedupWindow {
    pub fn new(window_samples: u64) -> Self {
        DedupWindow { window_samples, seen: Vec::new() }
    }

    fn content_hash(frame: &Frame) -> u64 {
        let mut h = DefaultHasher::new();
        frame.payload.hash_into(&mut h);
        h.finish()
    }

    /// Returns true if this frame is novel (and records it); false if a near
    /// duplicate was already seen within the offset window.
    pub fn admit(&mut self, frame: &Frame) -> bool {
        let ch = Self::content_hash(frame);
        let off = frame.meta.sample_offset;
        let dup = self.seen.iter().any(|&(c, o)| {
            c == ch && off.abs_diff(o) <= self.window_samples
        });
        if dup {
            false
        } else {
            self.seen.push((ch, off));
            true
        }
    }

    /// Drop records older than `cutoff` to bound memory on a long stream.
    pub fn prune(&mut self, cutoff_offset: u64) {
        self.seen.retain(|&(_, o)| o + self.window_samples >= cutoff_offset);
    }
}

pub struct ParallelDemodulator<D: Demodulator> {
    members: Vec<D>,
    dedup: DedupWindow,
}

impl<D: Demodulator> ParallelDemodulator<D> {
    /// All members must share a `native_rate`; the first member's caps are the
    /// ensemble's caps. `dedup_window_samples` ≈ 3 symbol periods.
    pub fn new(members: Vec<D>, dedup_window_samples: u64) -> Self {
        assert!(!members.is_empty(), "ensemble needs at least one member");
        ParallelDemodulator { members, dedup: DedupWindow::new(dedup_window_samples) }
    }
}

impl<D: Demodulator> Demodulator for ParallelDemodulator<D> {
    fn caps(&self) -> ModeCaps {
        self.members[0].caps()
    }

    fn feed(&mut self, samples: &[crate::types::Sample]) -> Vec<Frame> {
        let mut out = Vec::new();
        for m in &mut self.members {
            for f in m.feed(samples) {
                if self.dedup.admit(&f) {
                    out.push(f);
                }
            }
        }
        out
    }

    fn reset(&mut self) {
        for m in &mut self.members {
            m.reset();
        }
        self.dedup.seen.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::{DemodShape, Duplex, ModeCaps};
    use crate::types::{Frame, FrameMeta, FramePayload, Sample};

    /// A stub member that emits one fixed frame at a fixed offset on first feed.
    struct OneShot { frame: Frame, fired: bool }
    impl Demodulator for OneShot {
        fn caps(&self) -> ModeCaps {
            ModeCaps { native_rate: 48_000, bandwidth_hz: 1.0, tx: false,
                       duplex: Duplex::Half, shape: DemodShape::Streaming }
        }
        fn feed(&mut self, _s: &[Sample]) -> Vec<Frame> {
            if self.fired { return vec![]; }
            self.fired = true;
            vec![self.frame.clone()]
        }
        fn reset(&mut self) { self.fired = false; }
    }

    fn frame_at(bytes: &[u8], offset: u64) -> Frame {
        Frame {
            payload: FramePayload::Packet(bytes.to_vec()),
            meta: FrameMeta { sample_offset: offset, ..Default::default() },
        }
    }

    #[test]
    fn identical_frames_within_window_dedup_to_one() {
        let a = OneShot { frame: frame_at(b"HELLO", 1000), fired: false };
        let b = OneShot { frame: frame_at(b"HELLO", 1010), fired: false }; // 10 < window
        let mut ens = ParallelDemodulator::new(vec![a, b], 100);
        let out = ens.feed(&[0.0; 8]);
        assert_eq!(out.len(), 1, "near-duplicate should be deduped");
    }

    #[test]
    fn same_content_far_apart_is_kept() {
        let a = OneShot { frame: frame_at(b"HELLO", 1000), fired: false };
        let b = OneShot { frame: frame_at(b"HELLO", 5000), fired: false };
        let mut ens = ParallelDemodulator::new(vec![a, b], 100);
        assert_eq!(ens.feed(&[0.0; 8]).len(), 2);
    }

    #[test]
    fn different_content_same_offset_is_kept() {
        let a = OneShot { frame: frame_at(b"HELLO", 1000), fired: false };
        let b = OneShot { frame: frame_at(b"WORLD", 1000), fired: false };
        let mut ens = ParallelDemodulator::new(vec![a, b], 100);
        assert_eq!(ens.feed(&[0.0; 8]).len(), 2);
    }
}
```

- [ ] **Step 2: Add the `hash_into` helper to `FramePayload`**

In `crates/dsp/src/types.rs`, add:

```rust
impl FramePayload {
    /// Stable content hash input for dedup (ignores metadata).
    pub fn hash_into<H: std::hash::Hasher>(&self, h: &mut H) {
        use std::hash::Hash;
        match self {
            FramePayload::Packet(b) => { 0u8.hash(h); b.hash(h); }
            FramePayload::Text(s) => { 1u8.hash(h); s.hash(h); }
            FramePayload::Message77(m) => { 2u8.hash(h); m.hash(h); }
            FramePayload::Vocoder(b) => { 3u8.hash(h); b.hash(h); }
        }
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p omnimodem-dsp ensemble::`
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/ensemble.rs crates/dsp/src/types.rs
git commit -m "dsp: ParallelDemodulator ensemble + content/offset dedup window"
```

## Task 5: Conformance harness scaffolding (KAT runner, AWGN source)

**Files:**
- Create: `crates/dsp/src/testutil.rs`
- Create: `crates/dsp/tests/kat.rs`
- Create: `crates/dsp/tests/vectors/.gitkeep`

The harness is a **first-class requirement built here** (design §"Conformance testing"). It must exist before the coding blocks so each block lands with a KAT.

- [ ] **Step 1: Write the seeded AWGN source and hex helpers**

`crates/dsp/src/testutil.rs`:

```rust
//! Deterministic test fixtures: a seeded AWGN source and hex/byte helpers.
//! Gated behind `cfg(test)` or the `testutil` feature so production never
//! links it. `Math::random`-free: a fixed-seed xorshift + Box–Muller, so BER
//! sweeps and corpus generation are bit-reproducible across runs and machines.

use crate::types::Sample;

/// Minimal reproducible PRNG (xorshift64*). NOT cryptographic.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self { Rng(seed.max(1)) }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Standard normal via Box–Muller.
    pub fn next_normal(&mut self) -> f32 {
        let u1 = self.next_f32().max(1e-7);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }
}

/// Add white Gaussian noise of standard deviation `sigma` to `signal` in place.
pub fn add_awgn(signal: &mut [Sample], sigma: f32, rng: &mut Rng) {
    for s in signal.iter_mut() {
        *s += sigma * rng.next_normal();
    }
}

/// `sigma` for a target Eb/N0 (dB) given energy per bit and samples per bit.
pub fn sigma_for_ebn0(eb: f32, ebn0_db: f32, samples_per_bit: f32) -> f32 {
    let ebn0 = 10f32.powf(ebn0_db / 10.0);
    let n0 = eb / ebn0;
    // Two-sided noise power N0/2 per sample, spread over samples_per_bit.
    (n0 / 2.0 * samples_per_bit).sqrt()
}

pub fn hex_to_bytes(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

pub fn bytes_to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_for_seed() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 { assert_eq!(a.next_u64(), b.next_u64()); }
    }

    #[test]
    fn awgn_mean_near_zero_variance_near_sigma_sq() {
        let mut rng = Rng::new(7);
        let mut buf = vec![0.0f32; 100_000];
        add_awgn(&mut buf, 0.5, &mut rng);
        let mean: f32 = buf.iter().sum::<f32>() / buf.len() as f32;
        let var: f32 = buf.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / buf.len() as f32;
        assert!(mean.abs() < 0.02, "mean {mean}");
        assert!((var - 0.25).abs() < 0.02, "var {var}");
    }

    #[test]
    fn hex_roundtrips() {
        assert_eq!(bytes_to_hex(&hex_to_bytes("0a ff 10")), "0aff10");
    }
}
```

- [ ] **Step 2: Write the KAT runner skeleton**

`crates/dsp/tests/kat.rs` (each coding-block task adds its `#[test]` here):

```rust
//! Layer-1 conformance: known-answer tests against published/reference vectors.
//! Each coding block contributes a `#[test]` checked against vectors stored in
//! `tests/vectors/` or inline constants traceable to a named reference source.

use omnimodem_dsp::testutil::{bytes_to_hex, hex_to_bytes};

#[test]
fn harness_links() {
    // Sanity: the testutil surface is reachable from an integration test.
    assert_eq!(bytes_to_hex(&hex_to_bytes("dead")), "dead");
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p omnimodem-dsp --features testutil`
Expected: PASS (testutil unit tests + `harness_links`).

- [ ] **Step 4: Commit**

```bash
mkdir -p crates/dsp/tests/vectors && touch crates/dsp/tests/vectors/.gitkeep
git add crates/dsp/src/testutil.rs crates/dsp/tests/kat.rs crates/dsp/tests/vectors/.gitkeep
git commit -m "dsp: conformance harness scaffolding (seeded AWGN, KAT runner)"
```

---

# PART B — Group A: front-end DSP building blocks

> These are pure, independent, and parallelizable. Each lands with a KAT or
> analytic unit test. Reuse Graywolf source as cited; the design's reuse map
> gives exact `file:line` anchors — open them at implementation time.

## Task 6: 256-entry f32 oscillator LUT and the 8-accumulator FIR

**Files:**
- Create: `crates/dsp/src/frontend/mod.rs`
- Create: `crates/dsp/src/frontend/osc.rs`
- Create: `crates/dsp/src/frontend/fir.rs`

Graywolf reference: oscillator LUT is **`f32`** indexed by the top 8 bits of a phase accumulator (`demod_afsk.rs:40-60`); FIR is the SIMD-friendly 8-accumulator form (`demod_afsk.rs:67-107`).

- [ ] **Step 1: `frontend/mod.rs`**

```rust
//! Group A — front-end DSP & waveform building blocks.
pub mod osc;
pub mod fir;
pub mod nco;
pub mod resample;
pub mod stft;
pub mod agc;
pub mod limiter;
pub mod detector;
pub mod noise;
pub mod modulate;
```

- [ ] **Step 2: Write the oscillator test then implementation (`osc.rs`)**

```rust
//! Phase-accumulator oscillator with a 256-entry f32 cos/sin table, indexed by
//! the top 8 bits of a u32 phase accumulator (Graywolf demod_afsk.rs:40-60).

use std::f32::consts::TAU;

pub struct Oscillator {
    phase: u32,
    /// Phase increment per sample = freq/rate * 2^32.
    step: u32,
}

const COS: [f32; 256] = build_cos();
const SIN: [f32; 256] = build_sin();

const fn build_cos() -> [f32; 256] {
    // const-eval cannot call f32::cos; fill at first use instead.
    [0.0; 256]
}
const fn build_sin() -> [f32; 256] { [0.0; 256] }
```

Replace the const-table approach with a `OnceLock`-initialized table (const-eval can't do `cos`). Final `osc.rs`:

```rust
use std::f32::consts::TAU;
use std::sync::OnceLock;

fn tables() -> &'static ([f32; 256], [f32; 256]) {
    static T: OnceLock<([f32; 256], [f32; 256])> = OnceLock::new();
    T.get_or_init(|| {
        let mut cos = [0.0f32; 256];
        let mut sin = [0.0f32; 256];
        for i in 0..256 {
            let a = TAU * i as f32 / 256.0;
            cos[i] = a.cos();
            sin[i] = a.sin();
        }
        (cos, sin)
    })
}

pub struct Oscillator { phase: u32, step: u32 }

impl Oscillator {
    pub fn new(freq_hz: f32, rate_hz: f32) -> Self {
        let step = ((freq_hz / rate_hz) * (1u64 << 32) as f32) as u32;
        Oscillator { phase: 0, step }
    }
    pub fn set_freq(&mut self, freq_hz: f32, rate_hz: f32) {
        self.step = ((freq_hz / rate_hz) * (1u64 << 32) as f32) as u32;
    }
    /// Advance one sample, returning (cos, sin) from the top-8-bit table index.
    pub fn next(&mut self) -> (f32, f32) {
        let idx = (self.phase >> 24) as usize;
        self.phase = self.phase.wrapping_add(self.step);
        let (c, s) = tables();
        (c[idx], s[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_cycle_returns_near_origin_phase() {
        // 1 Hz at 256 Hz => exactly 256 samples per cycle, table-exact.
        let mut o = Oscillator::new(1.0, 256.0);
        let (c0, s0) = o.next();
        assert!((c0 - 1.0).abs() < 1e-6 && s0.abs() < 1e-6);
        for _ in 0..255 { o.next(); }
        let (c, s) = o.next();
        assert!((c - 1.0).abs() < 1e-6 && s.abs() < 1e-6);
    }
}
```

- [ ] **Step 3: Write the FIR test then implementation (`fir.rs`)**

```rust
//! Direct-form FIR with an 8-way accumulator unroll (Graywolf demod_afsk.rs:67-107),
//! plus filter-design helpers (lowpass/bandpass via windowed-sinc; RRC; raised
//! cosine; Gaussian) used by the modulators and the front-end filters.

use std::f32::consts::PI;

pub struct Fir {
    taps: Vec<f32>,
    hist: Vec<f32>,
    pos: usize,
}

impl Fir {
    pub fn new(taps: Vec<f32>) -> Self {
        let n = taps.len();
        Fir { taps, hist: vec![0.0; n], pos: 0 }
    }
    /// Push one sample, return the filtered output. Allocation-free.
    pub fn push(&mut self, x: f32) -> f32 {
        let n = self.taps.len();
        self.hist[self.pos] = x;
        let mut acc = [0.0f32; 8];
        let mut k = 0;
        // Walk taps newest->oldest with 8 independent accumulators.
        while k < n {
            let h = (self.pos + n - k) % n;
            acc[k % 8] += self.taps[k] * self.hist[h];
            k += 1;
        }
        self.pos = (self.pos + 1) % n;
        acc.iter().sum()
    }
    pub fn reset(&mut self) { self.hist.iter_mut().for_each(|h| *h = 0.0); self.pos = 0; }
}

/// Windowed-sinc lowpass (Hamming), `cutoff_hz` normalized by `rate_hz`.
pub fn design_lowpass(num_taps: usize, cutoff_hz: f32, rate_hz: f32) -> Vec<f32> {
    let fc = cutoff_hz / rate_hz; // cycles/sample
    let m = num_taps - 1;
    let mut h = vec![0.0f32; num_taps];
    for (i, hi) in h.iter_mut().enumerate() {
        let n = i as f32 - m as f32 / 2.0;
        let sinc = if n == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * n).sin() / (PI * n) };
        let w = 0.54 - 0.46 * (2.0 * PI * i as f32 / m as f32).cos(); // Hamming
        *hi = sinc * w;
    }
    let sum: f32 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= sum);
    h
}

/// Root-raised-cosine taps (Direwolf AFSK, M17 α=0.5). `sps` samples/symbol.
pub fn design_rrc(num_taps: usize, alpha: f32, sps: f32) -> Vec<f32> {
    let m = num_taps as isize / 2;
    let mut h = vec![0.0f32; num_taps];
    for i in 0..num_taps {
        let t = (i as isize - m) as f32 / sps;
        h[i] = rrc_tap(t, alpha);
    }
    let e: f32 = h.iter().map(|x| x * x).sum::<f32>().sqrt();
    h.iter_mut().for_each(|x| *x /= e);
    h
}

fn rrc_tap(t: f32, a: f32) -> f32 {
    if t.abs() < 1e-6 { return 1.0 - a + 4.0 * a / PI; }
    if (t.abs() - 1.0 / (4.0 * a)).abs() < 1e-4 {
        let s = ((1.0 + 2.0 / PI) * (PI / (4.0 * a)).sin()
               + (1.0 - 2.0 / PI) * (PI / (4.0 * a)).cos()) * a / 2f32.sqrt();
        return s;
    }
    let num = (PI * t * (1.0 - a)).sin() + 4.0 * a * t * (PI * t * (1.0 + a)).cos();
    let den = PI * t * (1.0 - (4.0 * a * t).powi(2));
    num / den
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fir_impulse_response_equals_taps() {
        let taps = vec![0.1, 0.2, 0.3, 0.4];
        let mut f = Fir::new(taps.clone());
        let mut out = vec![f.push(1.0)];
        for _ in 0..3 { out.push(f.push(0.0)); }
        for (o, t) in out.iter().zip(taps.iter()) {
            assert!((o - t).abs() < 1e-6, "got {o}, want {t}");
        }
    }

    #[test]
    fn lowpass_is_unity_dc_gain() {
        let h = design_lowpass(33, 1000.0, 8000.0);
        assert!((h.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn rrc_peak_at_center() {
        let h = design_rrc(65, 0.5, 8.0);
        let mid = h.len() / 2;
        assert!(h[mid] >= h.iter().cloned().fold(f32::MIN, f32::max) - 1e-6);
    }
}
```

- [ ] **Step 4: Run**

Run: `cargo test -p omnimodem-dsp frontend::osc:: frontend::fir::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/frontend/
git commit -m "dsp: oscillator LUT + 8-accumulator FIR + filter design (lowpass/RRC)"
```

## Task 7: NCO / complex down-converter, raised-cosine & Gaussian filter design

**Files:**
- Create: `crates/dsp/src/frontend/nco.rs`
- Modify: `crates/dsp/src/frontend/fir.rs` (add `design_raised_cosine`, `design_gaussian`)

- [ ] **Step 1: NCO test then impl (`nco.rs`)** — down-convert a real tone at `f0` to baseband and assert the residual frequency is ~0.

```rust
//! Tunable NCO + complex down-converter: multiply a real input by e^{-j2πf t}
//! to shift `tune_hz` to DC for passband isolation / click-to-tune.

use crate::frontend::osc::Oscillator;
use crate::types::Cplx;

pub struct DownConverter { cos_osc: Oscillator, rate: f32 }

impl DownConverter {
    pub fn new(tune_hz: f32, rate_hz: f32) -> Self {
        DownConverter { cos_osc: Oscillator::new(tune_hz, rate_hz), rate: rate_hz }
    }
    pub fn retune(&mut self, tune_hz: f32) { self.cos_osc.set_freq(tune_hz, self.rate); }
    /// Real sample -> complex baseband (x * (cos - j sin)).
    pub fn push(&mut self, x: f32) -> Cplx {
        let (c, s) = self.cos_osc.next();
        Cplx::new(x * c, -x * s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn tone_at_tune_freq_becomes_dc() {
        let rate = 8000.0;
        let f0 = 1500.0;
        let mut dc = DownConverter::new(f0, rate);
        // Average of the down-converted tone has large magnitude (energy at DC).
        let mut acc = Cplx::new(0.0, 0.0);
        for n in 0..8000 {
            let x = (TAU * f0 * n as f32 / rate).cos();
            acc += dc.push(x);
        }
        let mag = (acc / 8000.0).norm();
        assert!(mag > 0.4, "DC energy {mag} should be ~0.5 for a unit tone");
    }
}
```

- [ ] **Step 2: Add `design_raised_cosine` (PSK31/Throb envelope) and `design_gaussian` (CPFSK shaping, parametric BT)** to `fir.rs` with tests asserting symmetry and unit DC gain. Gaussian: `h[n] = exp(-(n/σ)²/2)`, with `σ` from BT (`σ = sps·sqrt(ln 2)/(2π·BT)`), normalized. KAT: BT=2.0 (FT8) and BT=1.0 (FT4) produce the documented effective pulse widths.

```rust
/// Raised-cosine *pulse* (not RRC): used for PSK31/Throb/Hell envelopes.
pub fn design_raised_cosine(num_taps: usize, alpha: f32, sps: f32) -> Vec<f32> {
    let m = num_taps as isize / 2;
    let mut h = vec![0.0f32; num_taps];
    for i in 0..num_taps {
        let t = (i as isize - m) as f32 / sps;
        let sinc = if t.abs() < 1e-6 { 1.0 } else { (PI * t).sin() / (PI * t) };
        let denom = 1.0 - (2.0 * alpha * t).powi(2);
        let cos = if denom.abs() < 1e-6 { PI / 4.0 } else { (PI * alpha * t).cos() / denom };
        h[i] = sinc * cos;
    }
    let s: f32 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= s);
    h
}

/// Gaussian shaping pulse for GFSK/CPFSK; `bt` is the bandwidth-time product
/// (FT8 BT=2.0, FT4 BT=1.0). `sps` samples per symbol.
pub fn design_gaussian(num_taps: usize, bt: f32, sps: f32) -> Vec<f32> {
    let sigma = sps * (2f32.ln()).sqrt() / (2.0 * PI * bt);
    let m = num_taps as isize / 2;
    let mut h = vec![0.0f32; num_taps];
    for i in 0..num_taps {
        let t = (i as isize - m) as f32;
        h[i] = (-(t * t) / (2.0 * sigma * sigma)).exp();
    }
    let s: f32 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= s);
    h
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p omnimodem-dsp frontend::nco:: frontend::fir::
git add crates/dsp/src/frontend/
git commit -m "dsp: NCO down-converter + raised-cosine/Gaussian filter design"
```

## Task 8: Polyphase rational resampler (upgrade of the Phase-2 linear one)

**Files:**
- Create: `crates/dsp/src/frontend/resample.rs`

The Phase-2 `audio::resample::RationalResampler` is a linear interpolator and its own doc-comment calls a "polyphase windowed-sinc upgrade a Phase-3 battery." Build that here, operating on `Sample` (`f32`). Leave the Phase-2 audio resampler in place (audio capture path); modes use this one.

- [ ] **Step 1: Test — resample a 1 kHz tone 48k→12k and assert the tone survives with low aliasing (peak FFT bin at 1 kHz, image bins ≥ 40 dB down).** Implement an integer-ratio polyphase FIR (reduce 48000/12000 = 4:1) plus a fractional fallback. Provide `Resampler::new(in_rate, out_rate, taps_per_phase)` and `process(&[Sample]) -> Vec<Sample>`.

- [ ] **Step 2: Property test** — output length within ±1 of `len*out/in`; passthrough when rates equal.

- [ ] **Step 3: Run + commit** (`dsp: polyphase rational resampler for mode working rates`).

## Task 9: Overlapped STFT engine (rustfft)

**Files:**
- Create: `crates/dsp/src/frontend/stft.rs`

Powers the waterfall **and** noncoherent tone detection; configurable window/hop (FT8 ~160 ms window, ~40 ms hop at 12 kHz ⇒ 1920-pt window, 480-pt hop, but use the FT8-standard 1920/960 — confirm against `ft8_lib monitor.c` at implementation time).

- [ ] **Step 1: Test** — feed a pure tone, assert the dominant STFT bin maps to the tone frequency `bin*rate/nfft`. Use a Hann window; assert window-sum normalization.

```rust
//! Overlapped STFT: windowed, hopped real-FFT producing magnitude/complex bins.
use rustfft::{FftPlanner, num_complex::Complex};
use crate::types::Sample;

pub struct Stft {
    nfft: usize,
    hop: usize,
    window: Vec<f32>,
    buf: Vec<f32>,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Stft {
    pub fn new(nfft: usize, hop: usize) -> Self {
        let window: Vec<f32> = (0..nfft)
            .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / nfft as f32).cos())
            .collect();
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(nfft);
        Stft { nfft, hop, window, buf: Vec::with_capacity(nfft * 2),
               fft, scratch: vec![Complex::new(0.0, 0.0); nfft] }
    }
    /// Feed samples; for each complete hop, emit one complex spectrum (len nfft).
    pub fn feed(&mut self, samples: &[Sample]) -> Vec<Vec<Complex<f32>>> {
        self.buf.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.buf.len() >= self.nfft {
            for i in 0..self.nfft {
                self.scratch[i] = Complex::new(self.buf[i] * self.window[i], 0.0);
            }
            self.fft.process(&mut self.scratch);
            out.push(self.scratch.clone());
            self.buf.drain(0..self.hop);
        }
        out
    }
    pub fn bin_hz(&self, bin: usize, rate: f32) -> f32 { bin as f32 * rate / self.nfft as f32 }
}
```

- [ ] **Step 2: Run + commit** (`dsp: overlapped STFT engine`).

## Task 10: AGC (peak-valley and decision-feedback) and the hard-limiter stage

**Files:**
- Create: `crates/dsp/src/frontend/agc.rs`
- Create: `crates/dsp/src/frontend/limiter.rs`

Graywolf reuse: decision-feedback AGC (W7ION) runs **only** in single-slicer mode; multi-slicer uses peak/valley (`demod_afsk.rs:507`). Model as **two distinct types**, selected by the registry, not composed.

- [ ] **Step 1: Peak/valley AGC test** — fast-attack/slow-decay envelope tracker; assert that a step-up tracks within `attack` and decays over `decay`. **Decision-feedback AGC** test — independent mark/space reference tracking converges to the two tone amplitudes given an alternating mark/space input.
- [ ] **Step 2: Hard-limiter** — `sign(x)` preserving zero-crossing timing (`demod_afsk.rs:459-461`); test it maps positive→+1, negative→−1, 0→0 and that a correlator fed limited input still recovers tone phase.
- [ ] **Step 3: Run + commit** (`dsp: peak/valley + decision-feedback AGC and hard-limiter correlator stage`).

## Task 11: FM discriminator, envelope detector + adaptive squelch, noise-floor/SNR reporter

**Files:**
- Create: `crates/dsp/src/frontend/detector.rs`
- Create: `crates/dsp/src/frontend/noise.rs`

- [ ] **Step 1: FM discriminator** — phase-difference detector on complex baseband (`atan2` of `z[n]·conj(z[n-1])`); test a frequency step produces a proportional output (WEFAX/FSK, RTTY). **Envelope detector + adaptive attack/decay squelch** (CW/Hell OOK): test OOK keying recovers the on/off envelope and the squelch gates noise below threshold.
- [ ] **Step 2: Per-bin noise-floor estimator + uniform SNR reporter** — running per-bin median/percentile noise floor; SNR normalized to a 2500 Hz reference BW (WSJT-X-style). Test: a known-SNR synthetic signal (signal power + AWGN floor) reports SNR within ±1 dB (this is the "metrics accuracy" guard from design §Testing, exercised early).
- [ ] **Step 3: Run + commit** (`dsp: FM/envelope detectors + per-bin noise floor and normalized SNR`).

## Task 12: Symmetric modulators — GFSK/CPFSK, M-FSK bank, differential M-PSK, 2-FSK, AFSK, CW keyer

**Files:**
- Create: `crates/dsp/src/frontend/modulate.rs`

Each modulator is the TX twin of a Phase-4 mode's RX detector; each gets an `insta` golden snapshot in Part H so any waveform change is caught in review.

- [ ] **Step 1: GFSK/CPFSK** (FT8/FT4 8-FSK, continuous-phase, Gaussian-shaped, BT param) — test phase continuity (no discontinuity > ε between symbols) and that the instantaneous frequency tracks the symbol tones. **M-FSK tone bank** (MFSK/Olivia/WSPR/4-FSK) — parametric tone count/spacing. **Differential BPSK/QPSK** (PSK31/ARDOP) — raised-cosine envelope, test that `diff_decode(diff_encode(bits)) == bits` through the modulator+a matched demod stub. **2-FSK shift** (RTTY/NAVTEX, selectable shift) — test mark/space tones at `±shift/2`. **AFSK** (Bell 202 1200/2200 Hz) — continuous-phase. **CW keyer** — raised-edge on/off keying at a tone, WPM→element timing.
- [ ] **Step 2: Run + commit** (`dsp: symmetric modulators (GFSK, M-FSK, diff-PSK, 2-FSK, AFSK, CW)`).

---

# PART C — Group B: synchronization & acquisition

## Task 13: DPLL clock recovery and DCD scorer

**Files:**
- Create: `crates/dsp/src/sync/mod.rs`
- Create: `crates/dsp/src/sync/dpll.rs`
- Create: `crates/dsp/src/sync/dcd.rs`

Graywolf reuse: DPLL with locked-vs-searching inertia (`demod_afsk.rs:626-678`); DCD scoring with hysteresis via shift-register popcount on/off thresholds (`demod_afsk.rs:691-723`, currently duplicated in `modem_9600`).

- [ ] **Step 1: `sync/mod.rs`**

```rust
//! Group B — synchronization & acquisition building blocks.
pub mod dpll;
pub mod dcd;
pub mod timing;
pub mod costas;
pub mod costas_array;
pub mod syncword;
pub mod candidate;
```

- [ ] **Step 2: DPLL test** — feed a bitstream at a slightly-off baud rate; assert the recovered bit clock locks (sampling instants converge to mid-bit) and that "locked" inertia resists a single noisy transition. Provide `DpllClockRecovery::new(sps)`, `update(transition: bool) -> Option<bool>` (emits a bit at each sampling instant).
- [ ] **Step 3: DCD test** — shift-register popcount with separate on/off thresholds; assert hysteresis (turns on above `on_thresh`, stays on until below `off_thresh`). Provide `DcdScorer::new(window, on, off)`, `update(good: bool) -> bool`.
- [ ] **Step 4: Run + commit** (`dsp: DPLL clock recovery and DCD scorer (Graywolf reuse)`).

## Task 14: Symbol-timing recovery variants

**Files:**
- Create: `crates/dsp/src/sync/timing.rs`

- [ ] **Step 1: Gardner / early-late** (PSK) — test the timing-error detector drives toward the symbol-optimum sampling phase on a synthetic BPSK signal. **Async start-bit edge** (RTTY/NAVTEX) — detect the space-to-mark start edge and sample the 5 data bits at the baud midpoints. **Transition-minimum** (PSK31) — sample where inter-symbol phase transitions are minimized.
- [ ] **Step 2: Run + commit** (`dsp: symbol-timing recovery (Gardner, start-bit, transition-minimum)`).

## Task 15: Costas-loop carrier recovery + AFC

**Files:**
- Create: `crates/dsp/src/sync/costas.rs`

- [ ] **Step 1: Costas loop test** — lock onto a BPSK carrier with a fixed frequency offset; assert the recovered phase error → 0 and the loop's frequency estimate matches the offset. **AFC** — fldigi-grade frequency-offset estimation + drift tracking with matched-filter re-centering; test it tracks a slow linear drift within tolerance.
- [ ] **Step 2: Run + commit** (`dsp: Costas-loop carrier recovery + AFC`).

## Task 16: Costas-**array** generator + correlator (parametric N)

**Files:**
- Create: `crates/dsp/src/sync/costas_array.rs`

Distinct from the Costas *loop*. FT8 uses the 7×7 array `[3,1,4,0,6,5,2]` repeated ×3 (start/mid/end); FT4 uses 4×4 ×4. Confirm the exact arrays against `ft8_lib constants.c` / WSJT-X at implementation time.

- [ ] **Step 1: KAT** — generator returns the canonical FT8 Costas array `[3,1,4,0,6,5,2]`; correlator's sync metric peaks at the true time/frequency offset of a synthesized FT8 preamble (3 Costas groups at the standard positions). Add to `tests/kat.rs`:

```rust
#[test]
fn ft8_costas_array_is_canonical() {
    use omnimodem_dsp::sync::costas_array::ft8_costas;
    assert_eq!(ft8_costas(), [3, 1, 4, 0, 6, 5, 2]);
}
```

- [ ] **Step 2: Run + commit** (`dsp: Costas-array generator + correlator (parametric N)`).

## Task 17: Sync-word/preamble correlator and the candidate finder

**Files:**
- Create: `crates/dsp/src/sync/syncword.rs`
- Create: `crates/dsp/src/sync/candidate.rs`

- [ ] **Step 1: Sync-word correlator** — generic known-sequence match with **Hamming-distance fuzzy threshold** (FX.25 64-bit CTAG fuzzy match; M17 16-bit sync; IL2P 24-bit `0xF15E48`). KAT: exact match scores 0 distance; a 2-bit-corrupted CTAG still matches under a distance-3 threshold; a random word does not. **Candidate finder** — sweep the passband over an STFT, return a sync-metric-sorted `Vec<Candidate{freq, time, metric}>`; test it finds N injected tones ranked by SNR.
- [ ] **Step 2: Run + commit** (`dsp: sync-word/preamble correlator (fuzzy) + candidate finder`).

---

# PART D — Group C: FEC & coding (soft-decision)

## Task 18: Parametric CRC library — CRC-16/X.25 and CRC-14 `0x6757`

**Files:**
- Create: `crates/dsp/src/fec/mod.rs`
- Create: `crates/dsp/src/fec/crc.rs`

- [ ] **Step 1: `fec/mod.rs`**

```rust
//! Group C — FEC & coding (soft-decision throughout). `Llr` is the spine.
pub mod crc;
pub mod nrzi;
pub mod gray;
pub mod scramble;
pub mod rs;
pub mod llr;
pub mod ldpc;
pub mod osd;
pub mod slicer;
```

- [ ] **Step 2: CRC test then impl** — table-free bitwise parametric CRC. **KATs:** CRC-16/X.25 (poly `0x1021`, init `0xFFFF`, reflect in/out, xorout `0xFFFF`) over `"123456789"` = `0x906E`; CRC-14 `0x6757` (FT8/FT4 — confirm reflection/init against `ft8_lib crc.c`, which computes a 14-bit CRC over the 77-bit message with the 91st..96th bits handled per the standard).

`crates/dsp/src/fec/crc.rs`:

```rust
//! Parametric CRC. Bit-at-a-time (no table) — these run off the hot path.

#[derive(Clone, Copy)]
pub struct CrcSpec {
    pub width: u8,
    pub poly: u32,
    pub init: u32,
    pub refin: bool,
    pub refout: bool,
    pub xorout: u32,
}

pub const CRC16_X25: CrcSpec =
    CrcSpec { width: 16, poly: 0x1021, init: 0xFFFF, refin: true, refout: true, xorout: 0xFFFF };

/// FT8/FT4 CRC-14. Confirm refin/refout/init against ft8_lib `crc.c` at impl
/// time (ft8_lib uses poly 0x2757 in 14-bit, non-reflected, init 0); the
/// design names it CRC-14 `0x6757` (17-bit-poly notation). Resolve to ONE spec
/// here and pin the resolved KAT.
pub const CRC14_FT8: CrcSpec =
    CrcSpec { width: 14, poly: 0x2757, init: 0x0000, refin: false, refout: false, xorout: 0x0000 };

fn reflect(mut v: u32, bits: u8) -> u32 {
    let mut r = 0;
    for _ in 0..bits { r = (r << 1) | (v & 1); v >>= 1; }
    r
}

pub fn crc(spec: &CrcSpec, data: &[u8]) -> u32 {
    let topbit = 1u32 << (spec.width - 1);
    let mask = (1u32 << spec.width) - 1;
    let mut reg = spec.init;
    for &b in data {
        let byte = if spec.refin { reflect(b as u32, 8) } else { b as u32 };
        reg ^= (byte << (spec.width - 8)) & mask;
        for _ in 0..8 {
            reg = if reg & topbit != 0 { ((reg << 1) ^ spec.poly) & mask } else { (reg << 1) & mask };
        }
    }
    if spec.refout { reg = reflect(reg, spec.width); }
    (reg ^ spec.xorout) & mask
}
```

KAT in `tests/kat.rs`:

```rust
#[test]
fn crc16_x25_check_value() {
    use omnimodem_dsp::fec::crc::{crc, CRC16_X25};
    assert_eq!(crc(&CRC16_X25, b"123456789"), 0x906E);
}
```

- [ ] **Step 3: Run + commit** (`dsp: parametric CRC library with CRC-16/X.25 and CRC-14 KATs`).

## Task 19: NRZI, Gray-code, and differential codecs

**Files:**
- Create: `crates/dsp/src/fec/nrzi.rs`
- Create: `crates/dsp/src/fec/gray.rs`

- [ ] **Step 1: NRZI** — AX.25 convention: a **0** bit causes a transition, a **1** bit holds. Test `nrzi_decode(nrzi_encode(bits)) == bits` and a hand KAT against a known transition pattern. **Gray + differential** — `gray_encode`/`gray_decode` round-trip over `0..256`; differential BPSK encode/decode round-trip.
- [ ] **Step 2: Property test** added to `tests/roundtrip.rs`:

```rust
proptest! {
    #[test]
    fn nrzi_roundtrips(bits in proptest::collection::vec(0u8..2, 0..512)) {
        use omnimodem_dsp::fec::nrzi::{nrzi_encode, nrzi_decode};
        prop_assert_eq!(nrzi_decode(&nrzi_encode(&bits)), bits);
    }
}
```

- [ ] **Step 3: Run + commit** (`dsp: NRZI, Gray, differential codecs + round-trip props`).

## Task 20: The three scramblers (G3RUH self-sync, IL2P frame-reset, additive PRBS)

**Files:**
- Create: `crates/dsp/src/fec/scramble.rs`

A common pitfall — three **distinct** primitives (design §"Scramblers"):

- **Self-synchronizing multiplicative LFSR** — G3RUH `x¹⁷+x¹²+1` (no seed; descrambler self-syncs).
- **Frame-reset additive LFSR** — IL2P `x⁹+x⁴+1`, fixed seed, reset per frame.
- **Additive PRBS / decorrelator** — M17 / Olivia whitening.

- [ ] **Step 1: KATs** — `descramble∘scramble = id` for all three; G3RUH descrambler **self-synchronizes** after ≤17 bits given an arbitrary start state (test: corrupt the first 17 output bits, assert the rest descramble correctly). IL2P additive scrambler reproduces the fixed-seed sequence (KAT first 16 bytes against `direwolf il2p_scramble.c`).
- [ ] **Step 2: Property test** — round-trip over random byte vectors for each.
- [ ] **Step 3: Run + commit** (`dsp: G3RUH self-sync + IL2P frame-reset + additive PRBS scramblers`).

## Task 21: GF(256) Reed-Solomon (parametric nroots/fcr/prim)

**Files:**
- Create: `crates/dsp/src/fec/rs.rs`

Must instantiate **fcr=1 (FX.25)** *and* **fcr=0 (IL2P)** over GF(2⁸) with primitive `0x11D`; `nroots ∈ {2,4,6,8,16,32,64}`; shortened/zero-padded blocks. Algorithm: standard GF(256) tables (log/antilog), syndrome → Berlekamp–Massey → Chien search → Forney. Cite `direwolf fx25_*.c` / `il2p_*.c` and the classic `fec` library structure.

- [ ] **Step 1: KAT then impl** — encode a known message, corrupt ≤ `nroots/2` symbols, assert the decoder recovers it; corrupt `> nroots/2`, assert it reports failure (not a wrong correction). KAT a specific IL2P RS(255,239)-shortened block against a vector captured from `direwolf gen_packets`/`il2p_test`, and an FX.25 RS block from a known FX.25 tag/codeblock. Provide:

```rust
pub struct Rs { nroots: usize, fcr: usize, /* gf tables, generator poly */ }
impl Rs {
    pub fn new(nroots: usize, fcr: usize, prim: u8) -> Self { /* build GF(256) + gen poly */ }
    pub fn encode_parity(&self, data: &[u8]) -> Vec<u8>;        // returns nroots parity symbols
    pub fn decode(&self, codeword: &mut [u8]) -> Result<usize, RsError>; // returns #corrected
}
```

- [ ] **Step 2: Property test** — for random data and ≤ t random errors, `decode` corrects to the original; t = `nroots/2`.
- [ ] **Step 3: Run + commit** (`dsp: GF(256) Reed-Solomon (fcr=0 IL2P, fcr=1 FX.25), KAT+prop`).

## Task 22: Soft-LLR demapper — the contract spine

**Files:**
- Create: `crates/dsp/src/fec/llr.rs`

Tone power / phase → per-bit `Llr`, noise-variance scaled (design §"Soft-LLR demapper"). This is the named contract; FT8's LDPC depends on it.

- [ ] **Step 1: Test** — for an M-FSK symbol, `demap_fsk(&tone_powers, noise_var)` returns LLRs whose **sign** matches the max-power tone's bit pattern and whose **magnitude** scales with SNR (higher SNR ⇒ larger `|Llr|`). For BPSK, `demap_bpsk(soft_symbol, noise_var) = 2·soft/noise_var` with the locked sign convention (positive soft ⇒ LLR for bit 0). Assert against hand-computed values.
- [ ] **Step 2: Property** — at very high SNR, `SoftBits::hard()` of the demapped LLRs equals the transmitted bits.
- [ ] **Step 3: Run + commit** (`dsp: soft-LLR demapper (FSK power + PSK phase -> per-bit LLR)`).

## Task 23: LDPC encode + min-sum/BP decode, FT8 (174,91)

**Files:**
- Create: `crates/dsp/src/fec/ldpc.rs`

Parametric H-matrix; ship the FT8/FT4 (174,91) matrices (`kFTX_LDPC_Nm`, `kFTX_LDPC_Mn`, generator `kFTX_LDPC_generator` — lift the tables from `ft8_lib ldpc.c`/`constants.c`; **do not** hand-type them, transcribe and KAT). Min-sum with a scaling factor (the design flags "exact LDPC min-sum scaling" as a constant to confirm against the reference — start at 0.75 and KAT against `ft8_lib`).

- [ ] **Step 1: KAT — encode** a known 91-bit input (77 message + 14 CRC) to the 174-bit codeword and compare against `ft8_lib`'s `encode174`. **Decode** the noiseless codeword's LLRs back to the 91 message bits with zero iterations needed; then add AWGN at a moderate SNR and assert it still decodes (parity-check satisfied). Signature:

```rust
pub struct Ldpc { /* Nm, Mn, generator, n=174, k=91 */ }
impl Ldpc {
    pub fn ft8() -> Self;
    pub fn encode(&self, message_bits: &[u8]) -> Vec<u8>;      // 91 -> 174
    /// Min-sum BP. Returns (decoded 174-bit hard, parity_errors) — caller takes
    /// the first k bits as the message and checks `parity_errors == 0`.
    pub fn decode_minsum(&self, llrs: &[Llr], max_iters: usize) -> (Vec<u8>, usize);
}
```

- [ ] **Step 2: BER smoke test** (cheap; full sweep is Phase-4 gate) — at a fixed SNR over `testutil` AWGN, decode rate over 100 trials exceeds a floor (e.g. >90% at the chosen SNR), proving the decoder works end-to-end with the demapper.
- [ ] **Step 3: Run + commit** (`dsp: LDPC encode + min-sum BP decode with FT8 (174,91) matrices`).

## Task 24: Ordered-statistics decoding (OSD) layer

**Files:**
- Create: `crates/dsp/src/fec/osd.rs`

The "last ~2 dB" after BP (design §"LDPC ... OSD"). OSD-`order` reprocessing over the LDPC generator: sort columns by `|Llr|`, Gaussian-eliminate over GF(2) to find the most-reliable independent positions, re-encode candidate flips, pick the codeword minimizing soft distance.

- [ ] **Step 1: Test** — construct a received word where BP fails to converge but OSD-1 recovers the codeword (assert OSD finds the true codeword at an SNR where `decode_minsum` alone returns parity errors). Signature `osd_decode(&Ldpc, llrs, order) -> Option<Vec<u8>>` (174-bit codeword).
- [ ] **Step 2: Run + commit** (`dsp: OSD reprocessing layer over the LDPC soft output`).

## Task 25: MultiSlicer (geometric space-gain table)

**Files:**
- Create: `crates/dsp/src/fec/slicer.rs`

Graywolf reuse: N slicers (default 9) deciding mark-vs-space at different thresholds via the geometric space-gain table (`demod_afsk.rs:425-433, 530`).

- [ ] **Step 1: Test** — `MultiSlicer::new(9)` produces 9 monotonically-spaced geometric thresholds; `slice(mark, space) -> [bool; N]` decides per threshold; assert a mark-dominant input slices to mark at the center threshold and the geometric spread covers strong/weak space gain. Used by AFSK in Phase 4; tested standalone here.
- [ ] **Step 2: Run + commit** (`dsp: MultiSlicer with geometric space-gain table`).

---

# PART E — Group D: source / message / framing coding

## Task 26: Varicode (PSK) with pluggable table

**Files:**
- Create: `crates/dsp/src/framing/mod.rs`
- Create: `crates/dsp/src/framing/varicode.rs`

- [ ] **Step 1: `framing/mod.rs`**

```rust
//! Group D — source, message and framing coding.
pub mod varicode;
pub mod baudot;
pub mod morse;
pub mod hdlc;
pub mod ax25;
pub mod fx25;
pub mod il2p;
pub mod message77;
```

- [ ] **Step 2: Varicode KAT** — PSK31 Varicode table; encode `"e"` → its canonical codeword, decode the `00`-delimited bitstream back to text. Round-trip property over printable ASCII. Pluggable table (PSK vs MFSK/DominoEX nibble) via a `&VaricodeTable` param.
- [ ] **Step 3: Run + commit** (`dsp: Varicode (pluggable table) encode/decode`).

## Task 27: Baudot/ITA2 (LTRS/FIGS) for RTTY

**Files:**
- Create: `crates/dsp/src/framing/baudot.rs`

- [ ] **Step 1: KAT** — encode `"RYRY 123"` with correct LTRS/FIGS shift insertion; decode back; assert shift state is tracked (a FIGS digit then LTRS letters round-trip). Provide `encode(&str) -> Vec<u8>` (5-bit codes) and a stateful `Decoder`.
- [ ] **Step 2: Run + commit** (`dsp: Baudot/ITA2 with LTRS/FIGS shift`).

## Task 28: Morse + SOM/fuzzy decoder (CW)

**Files:**
- Create: `crates/dsp/src/framing/morse.rs`

- [ ] **Step 1: Test** — encode `"CQ"` to dot/dash element timing; decode a clean element stream back; the **SOM/fuzzy** best-fit decoder recovers the character from slightly-mistimed elements (e.g. ±20% element jitter). Provide a `MorseDecoder` driven by keyed on/off durations.
- [ ] **Step 2: Run + commit** (`dsp: Morse encode + SOM/fuzzy decode`).

## Task 29: HDLC (flag / bit-stuff / destuff / FCS)

**Files:**
- Create: `crates/dsp/src/framing/hdlc.rs`

- [ ] **Step 1: KAT** — `0x7E` flag delimiting; bit-stuff a 0 after five consecutive 1s; FCS = CRC-16/X.25 appended LSB-first. Test `hdlc_frame(payload)` then `hdlc_deframe(bitstream)` recovers the payload and validates the FCS; a single bit flip fails the FCS. Cross-check the framed bytes against a `direwolf gen_packets` capture of the same payload.
- [ ] **Step 2: Property** — `deframe(frame(p)) == [p]` for random payloads; bit-stuffing never emits six consecutive 1s outside a flag.
- [ ] **Step 3: Run + commit** (`dsp: HDLC flag/stuff/destuff/FCS with Direwolf cross-check`).

## Task 30: AX.25 / APRS UI-frame convention

**Files:**
- Create: `crates/dsp/src/framing/ax25.rs`

- [ ] **Step 1: KAT** — build a UI frame (address field with 7-bit-shifted callsigns + SSID, control `0x03`, PID `0xF0`, info); parse it back to `{dest, source, digipeaters, info}`. KAT a real APRS position packet against a Direwolf-decoded reference. Provide `Ax25Frame` struct with `encode()`/`decode()`.
- [ ] **Step 2: Run + commit** (`dsp: AX.25/APRS UI-frame encode/decode`).

## Task 31: FX.25 (CTAG + RS wrap of an intact HDLC frame)

**Files:**
- Create: `crates/dsp/src/framing/fx25.rs`

FX.25 wraps an **intact, legacy-compatible** HDLC frame with a 64-bit CTAG preamble + GF(256) RS (fcr=1) parity, so a non-FX.25 receiver still decodes the inner AX.25. Cite `direwolf fx25_*.c`.

- [ ] **Step 1: KAT** — wrap a known HDLC frame, corrupt a few bytes, RS-recover, and confirm the inner frame is bit-identical; verify the CTAG selects the correct RS block size. Cross-check the wrapped bytes against `direwolf gen_packets -X`.
- [ ] **Step 2: Run + commit** (`dsp: FX.25 CTAG + RS-wrapped HDLC`).

## Task 32: IL2P (sync + transposed header + per-block RS)

**Files:**
- Create: `crates/dsp/src/framing/il2p.rs`

IL2P **replaces** HDLC: 24-bit sync `0xF15E48`, a 6-bit-callsign-transposing header, per-block RS (fcr=0), and the IL2P frame-reset scrambler from Task 20. The `set_field` bit map is flagged in the design as a constant to confirm against the reference — transcribe from `direwolf il2p_header.c`.

- [ ] **Step 1: KAT** — encode an AX.25 frame to IL2P (header transposition + RS + scramble), decode back to the original AX.25; KAT the on-air bytes against a `direwolf il2p_test` vector. Corrupt within RS capacity and confirm recovery.
- [ ] **Step 2: Run + commit** (`dsp: IL2P header transposition + per-block RS + scramble`).

## Task 33: WSJT-X 77-bit message codec + callsign compression + hashing + grid

**Files:**
- Create: `crates/dsp/src/framing/message77.rs`

Type field, standard exchange, free text, telemetry; **28-bit callsign compression + 10/12/22-bit callsign hashing** (shared hash table); **15-bit Maidenhead grid** + power. Transcribe the packers from `ft8_lib pack.c`/`unpack.c`/`text.c`; **do not** reconstruct the bit layout from prose.

- [ ] **Step 1: KAT** — `pack77("CQ K1ABC FN42")` → the canonical 77-bit payload (10 bytes) matching `ft8code "CQ K1ABC FN42"`; `unpack77` of that payload returns the original text. Test the i3/n3 type-field routing for: standard call+grid, free text, and a hashed-callsign message. Callsign hash KAT against `ft8_lib`'s `hash22`/`hash12`/`hash10`.
- [ ] **Step 2: Property** — for a corpus of valid standard messages, `unpack77(pack77(m)) == m`.
- [ ] **Step 3: Run + commit** (`dsp: WSJT-X 77-bit message codec + callsign hashing + grid`).

---

# PART F — Mode-framework wiring into the daemon

## Task 34: Parametric `ModeConfig`, registry, and the `NullMode` fixture

**Files:**
- Modify: `crates/omnimodem/Cargo.toml` (add `omnimodem-dsp`)
- Create: `crates/omnimodem/src/mode/mod.rs`
- Create: `crates/omnimodem/src/mode/registry.rs`
- Modify: `crates/omnimodem/src/lib.rs` (`pub mod mode;`)

This is the design's "one-module mode registry" — adding a mode later touches **one** module, not five `match` arms. Phase 3 ships **no end-user mode**; the registry registers only a `NullMode` framework fixture (passthrough demod/mod) so the wiring is exercised end-to-end without claiming a mode.

- [ ] **Step 1: Add the dependency** to `crates/omnimodem/Cargo.toml`:

```toml
[dependencies]
# ...existing...
omnimodem-dsp = { path = "../dsp" }
```

- [ ] **Step 2: Parametric `ModeConfig` (`mode/mod.rs`)** — the parametric enum the design mandates (`enum ModeConfig { ... }`), with `None` plus the Phase-4 variants stubbed as data-only (no demod impl yet), so the type is stable when Phase 4 fills them in. Parse from the channel's `mode` string.

```rust
//! Mode framework wiring: the parametric per-mode config and the registry that
//! turns a config into a boxed demodulator/modulator. Phase 3 implements only
//! `NullMode`; Phase-4 variants are present as data so the enum is stable.

pub mod registry;

use omnimodem_dsp::mode::ModeCaps;

/// Parametric per-mode configuration (design §"Mode framework model": NOT one
/// flat struct). Variants beyond `None` are data-only until Phase 4.
#[derive(Debug, Clone, PartialEq)]
pub enum ModeConfig {
    None,
    Afsk1200 { tx: bool },
    Ft8,
    Cw { wpm: u16, tone_hz: f32 },
    Rtty { baud: f32, shift_hz: f32 },
    Psk31 { center_hz: f32 },
}

impl ModeConfig {
    /// Parse the channel's `mode` string. Phase 3 only resolves "none"; unknown
    /// strings are rejected so a typo can't silently configure nothing.
    pub fn parse(s: &str) -> Option<ModeConfig> {
        match s {
            "none" | "" => Some(ModeConfig::None),
            _ => None, // Phase 4 extends this; keep strict.
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            ModeConfig::None => "none",
            ModeConfig::Afsk1200 { .. } => "afsk1200",
            ModeConfig::Ft8 => "ft8",
            ModeConfig::Cw { .. } => "cw",
            ModeConfig::Rtty { .. } => "rtty",
            ModeConfig::Psk31 { .. } => "psk31",
        }
    }
}

/// The framework fixture: a passthrough that satisfies the trait surface so the
/// registry, channel wiring, and conformance harness exercise a real demod/mod
/// without shipping an end-user mode.
pub struct NullMode;

impl omnimodem_dsp::mode::Demodulator for NullMode {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: 48_000,
            bandwidth_hz: 0.0,
            tx: false,
            duplex: omnimodem_dsp::mode::Duplex::Half,
            shape: omnimodem_dsp::mode::DemodShape::Streaming,
        }
    }
    fn feed(&mut self, _s: &[omnimodem_dsp::Sample]) -> Vec<omnimodem_dsp::Frame> { vec![] }
    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_is_strict() {
        assert_eq!(ModeConfig::parse("none"), Some(ModeConfig::None));
        assert_eq!(ModeConfig::parse("ft8"), None); // not until Phase 4
        assert_eq!(ModeConfig::parse("bogus"), None);
    }
}
```

- [ ] **Step 3: Registry (`mode/registry.rs`)** — the single dispatch point.

```rust
//! The one-module mode registry. Adding a mode in Phase 4 adds one arm here and
//! its module; nothing else in the daemon learns mode-specific details.

use super::{ModeConfig, NullMode};
use omnimodem_dsp::mode::Demodulator;

/// Build a streaming demodulator for a config, or `None` if the mode has no
/// streaming demod (windowed modes return their `BlockDemodulator` elsewhere).
pub fn build_demod(cfg: &ModeConfig) -> Option<Box<dyn Demodulator>> {
    match cfg {
        ModeConfig::None => Some(Box::new(NullMode)),
        // Phase 4: Afsk1200/Cw/Rtty/Psk31 return their streaming demods; Ft8
        // returns its BlockDemodulator via a separate builder.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn none_builds_nullmode() {
        let d = build_demod(&ModeConfig::None).expect("none builds");
        assert_eq!(d.caps().native_rate, 48_000);
    }
}
```

- [ ] **Step 4: Wire `pub mod mode;` into `lib.rs`, parse the channel mode** — in `supervisor::configure_channel`/`ChannelConfig`, validate the `mode` string via `ModeConfig::parse` and reject unknown modes with a `CoreError` (keeps the gRPC `mode` field meaningful without changing the proto). Keep `ChannelConfig.mode: String` as the persisted form; resolve to `ModeConfig` at use.

- [ ] **Step 5: Run** the daemon test suite to confirm no regression:

Run: `cargo test -p omnimodem mode::`
Expected: PASS (parse strictness + registry tests).

- [ ] **Step 6: Commit**

```bash
git add crates/omnimodem/Cargo.toml crates/omnimodem/src/mode/ crates/omnimodem/src/lib.rs crates/omnimodem/src/supervisor/
git commit -m "omnimodem: parametric ModeConfig + one-module registry + NullMode fixture"
```

---

# PART G — Conformance, properties, golden snapshots

## Task 35: Modulator golden snapshots (`insta`)

**Files:**
- Create: `crates/dsp/tests/snapshots.rs`
- Create: `crates/dsp/tests/vectors/` snapshots

Design §"Modulator golden snapshots": modulate a **fixed** message and diff the symbol stream / waveform against a stored golden vector so any change to on-air output is caught in review.

- [ ] **Step 1: For each modulator** (GFSK, 2-FSK, AFSK, diff-BPSK, CW), modulate a fixed input and snapshot a quantized fingerprint (e.g. first 256 samples rounded to i16, or the symbol-tone sequence) via `insta::assert_json_snapshot!`. Review and accept the initial snapshots.
- [ ] **Step 2: Run + commit**

```bash
cargo test -p omnimodem-dsp --test snapshots
cargo insta accept
git add crates/dsp/tests/snapshots.rs crates/dsp/tests/vectors/
git commit -m "dsp: modulator golden snapshots (insta)"
```

## Task 36: Round-trip property suite consolidation

**Files:**
- Modify: `crates/dsp/tests/roundtrip.rs`

Design §"Layer 3 — property tests": `descramble∘scramble = id`, `decode∘encode = id`, FEC corrects ≤ t and detects > t, NRZI/Gray/interleaver round-trips.

- [ ] **Step 1: Gather** the per-block proptests into one suite: scramblers (×3), NRZI, Gray, RS (corrects ≤ t / fails > t without miscorrection), Varicode, Baudot, HDLC deframe∘frame, 77-bit pack/unpack, LDPC encode→noiseless-decode.
- [ ] **Step 2: Run + commit**

```bash
cargo test -p omnimodem-dsp --test roundtrip
git add crates/dsp/tests/roundtrip.rs
git commit -m "dsp: consolidated round-trip property suite"
```

## Task 37: KAT vector files + reference-binary interop stubs

**Files:**
- Modify: `crates/dsp/tests/kat.rs`
- Create: `crates/dsp/tests/vectors/*.json` (checked-in vectors)

- [ ] **Step 1: Externalize** the captured reference vectors (Direwolf HDLC/AX.25/FX.25/IL2P bytes, ft8_lib LDPC/CRC/77-bit, RS blocks) into `tests/vectors/*.json` with a provenance comment naming the reference binary + version + exact command that produced each. The KAT runner loads and checks them. Where a reference binary is required to regenerate (gated, not on every PR), add an `#[ignore]`d test documenting the regeneration command so the provenance is executable.
- [ ] **Step 2: Run + commit**

```bash
cargo test -p omnimodem-dsp --features testutil --test kat
git add crates/dsp/tests/kat.rs crates/dsp/tests/vectors/
git commit -m "dsp: externalized KAT vectors with reference provenance"
```

## Task 38: Real-time-path allocation/throughput guard (criterion)

**Files:**
- Create: `crates/dsp/benches/hotpath.rs`
- Modify: `crates/dsp/Cargo.toml` (add `criterion` dev-dep + `[[bench]]`)

Design §"Real-time-path guards": the sample loop is benchmarked and asserted allocation-free/bounded — on a real-time modem a perf regression *is* a correctness bug.

- [ ] **Step 1: Benchmark** `Fir::push`, the STFT feed, and `ParallelDemodulator::feed` over the `NullMode`/a stub; add a debug assertion (or a counting global allocator behind a test cfg) that the streaming `feed` performs **zero** heap allocations per sample for a frame-less input.
- [ ] **Step 2: Run + commit**

```bash
cargo bench -p omnimodem-dsp --no-run
git add crates/dsp/benches/hotpath.rs crates/dsp/Cargo.toml
git commit -m "dsp: criterion hot-path benches + allocation guard"
```

---

# PART H — Phase exit criterion

## Task 39: Exit-criterion conformance gate

The design's Phase-3 exit criterion: **ships no end-user mode**, but is validated by the conformance harness against extracted reference stages — "our LDPC/RS/CRC building blocks pass known-answer vectors."

**Files:**
- Modify: `crates/dsp/tests/kat.rs` (a single `phase3_exit_criterion` aggregate test)
- Create/Modify: `docs/design/2026-06-17-omnimodem-design.md` (flip Phase 3 status line)

- [ ] **Step 1: Aggregate gate test** — one `#[test]` that asserts the presence and pass of the contract-critical KATs (CRC-16/X.25 `0x906E`, CRC-14 resolved value, RS correct/detect, LDPC encode-matches-reference + noiseless decode, FT8 Costas array, 77-bit pack/unpack, HDLC Direwolf cross-check). This is the executable definition of "Phase 3 done."

```rust
#[test]
fn phase3_exit_criterion() {
    // Coding-block KATs that gate the phase. Each underlying KAT is its own
    // #[test]; this aggregates the contract-critical subset as the exit gate.
    crc16_x25_check_value();
    ft8_costas_array_is_canonical();
    // ...calls into the RS, LDPC, 77-bit, HDLC KAT helpers...
}
```

- [ ] **Step 2: Full suite green** —

Run: `cargo test --workspace --features omnimodem-dsp/testutil`
Expected: PASS across `omnimodem-dsp` (unit + kat + roundtrip + snapshots) and `omnimodem` (existing Phase-1/2 e2e unaffected; new `mode` tests pass).
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Update the design status line** — change the design doc's Phase 3 from "the deliberate, carefully-considered phase" lead-in's implied "in planning" to reflect implementation, mirroring how Phase 1/2 status lines were updated (`design.md` header + Phase 3 heading). Keep phase headings verbatim so plan cross-references stay valid.

- [ ] **Step 4: Commit + open PR**

```bash
git add crates/dsp/tests/kat.rs docs/design/2026-06-17-omnimodem-design.md
git commit -m "dsp: Phase 3 exit-criterion conformance gate"
```

Open a PR titled **"Phase 3: mode scaffolding & building-blocks toolkit"** attributed to `chrissnell`, body summarizing the new crate, the building-block coverage vs the Phase-4 mode needs, and the conformance gate. Branch name e.g. `feature/phase3-mode-framework`.

---

## Self-Review

**1. Spec coverage (design Phase 3 + the building-block groups it names):**

- Framework: traits (both demod shapes) + `ModeCaps` + parametric `ModeConfig` + one-module registry + `ParallelDemodulator` → Tasks 2–4, 34. ✓
- Soft-LLR contract as spine → `Llr` (Task 2), demapper (Task 22), consumed by LDPC (Task 23). ✓
- Group A front-end DSP → Tasks 6–12 (osc, FIR/filters, NCO, resampler, STFT, AGC, limiter, detectors, noise/SNR, modulators). ✓
- Group B sync → Tasks 13–17 (DPLL, DCD, timing, Costas-loop+AFC, Costas-array, sync-word, candidate finder). ✓
- Streaming C/D stages (NRZI, scramblers, HDLC/FX.25/IL2P, GF(256) RS, dedup window) → Tasks 4 (dedup), 18–21, 25, 29–32. ✓
- FT8 stack (soft-LLR + LDPC-BP/OSD + Costas-array) → Tasks 16, 22–24. ✓
- Source/message (Varicode, Baudot, Morse, 77-bit) → Tasks 26–28, 33. ✓
- Conformance harness usable in Phase 3 (KAT, proptest, snapshots, seeded AWGN) → Tasks 5, 35–37; exit gate Task 39. ✓
- Deferred groups (Viterbi/FHT/QRA/Fano/OFDM/vocoder/ARQ, wideband/SIC/AP) explicitly excluded in Scope. ✓
- "Ships no end-user mode" honored — only `NullMode` fixture; gRPC proto unchanged. ✓

**2. Placeholder scan:** The heavy numerical blocks (RS, LDPC, OSD, FX.25, IL2P, 77-bit, Costas-array) give full **signatures, KAT vectors, algorithm, and a named reference `file` to transcribe constants from** rather than inventing exact floats — this is the design's own instruction ("confirm exact constants against the reference sources at implementation time"; it explicitly lists LDPC min-sum scaling, AP LLR seeding, FT8 sync threshold, IL2P `set_field` as confirm-at-impl). That is a deliberate, design-sanctioned spec, not a "TODO" placeholder; the deterministic blocks (types, ensemble, CRC, NRZI/Gray, scramblers, FIR/osc/NCO/STFT, slicer, HDLC) carry complete code. Tasks that defer code give the exact test to write first and the signature to satisfy.

**3. Type consistency:** `Sample`/`Llr`/`SoftBits`/`Frame`/`FramePayload`/`FrameMeta` defined once (Task 2) and used unchanged in ensemble (Task 4), traits (Task 3), demapper (22), LDPC (23). `Demodulator::feed`/`reset`/`caps` signatures consistent across Tasks 3, 4, 34. `ModeCaps`/`DemodShape`/`Duplex` consistent (Tasks 3, 34). `Rs::{encode_parity,decode}`, `Ldpc::{encode,decode_minsum}`, `crc(&CrcSpec, &[u8])` referenced consistently by their consumers (FX.25/IL2P use `Rs`; FT8 path uses `Ldpc` + `CRC14_FT8`; HDLC uses `CRC16_X25`). The `FramePayload::hash_into` helper added in Task 4 matches its use in `DedupWindow`.
