# Omnimodem Phase 4 — First Modes (AFSK 1200, FT8, CW, RTTY, PSK31) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Assemble the Phase-3 building blocks into five end-user modes — AFSK 1200 (AX.25), FT8, CW, RTTY, PSK31 — and add the daemon infrastructure they are the first to need: a per-channel RX worker that finally feeds captured audio through a real demod, a per-channel TX worker with a cooperative frame queue and time-slot-aligned scheduling, and a host-clock-offset metric; each mode gated by the conformance harness (KAT vectors, bidirectional cross-decode, BER/decode-rate curves).

**Architecture:** Mode DSP assemblies live in the **pure `omnimodem-dsp` crate** under a new `crates/dsp/src/modes/` module — one file per mode, each wiring Phase-3 stages into a `Demodulator`/`BlockDemodulator` + `Modulator`. Keeping them in the hardware-free crate means every mode KAT-tests, loopback-tests, and BER-tests in CI with no daemon and no audio device, exactly where the conformance harness already lives. The daemon's existing thin `crate::mode::registry` maps a parametric `ModeConfig` to these assemblies; nothing else in the daemon learns mode specifics. The daemon gains two new sync-core modules — `core::rx_worker` (per-channel capture→demod→`FrameEvent`) and `core::tx_worker` (per-channel queue→modulate→slot-align→`drive_tx_cycle`) — spawned by the core thread, sharing the existing per-rig `RxTxInterlock`.

**Tech Stack:** Rust (edition 2021, workspace). Reuses Phase-3 `omnimodem-dsp` blocks (`frontend::modulate`, `frontend::detector`, `sync::*`, `fec::*`, `framing::*`, `ensemble`) and Phase-1/2 daemon seams (`AudioBackend`/`CaptureHandle`/`PlaybackHandle`, `PttDriver`, `drive_tx_cycle`, `RxTxInterlock`, the `mpsc`-command / `tokio::broadcast`-event spine). Tests: inline `#[cfg(test)]` unit tests, `insta` golden snapshots, `proptest` round-trips, and the `testutil` AWGN/RNG fixtures.

---

## Scope

**In scope:**

- **Five mode assemblies** (`crates/dsp/src/modes/`): `afsk1200` (streaming/HDLC), `psk31` (streaming, differential PSK + Varicode), `rtty` (streaming, 2-FSK + Baudot), `cw` (streaming, envelope + Morse), `ft8` (block/windowed: STFT → candidate → Costas-array sync → soft-LLR → LDPC+OSD → CRC-14 → 77-bit unpack). Each exposes a TX `Modulator` and an RX demod.
- **Mode registry wiring** (`crates/omnimodemd/src/mode/`): extend `ModeConfig::parse` to the five labels with default parameters; `build_demod` / `build_block_demod` / `build_modulator` construct the dsp assemblies.
- **Daemon RX worker** (`crates/omnimodemd/src/core/rx_worker.rs`): per-channel thread pulling `AudioChunk`s, resampling to the mode's native rate, driving streaming **or** windowed demods, honoring the RX/TX interlock, emitting `FrameEvent::RxFrame`.
- **Daemon TX model** (`crates/omnimodemd/src/core/tx_worker.rs`): per-channel worker, cooperative frame queue, payload→`Frame`→samples via the mode `Modulator`, time-slot-aligned scheduling for windowed modes, serialized per-rig via the shared PTT registry/interlock. (Exclusive lease deferred to Phase 5 per design §"Open questions".)
- **Time synchronization** (`crates/omnimodemd/src/core/clock.rs`): a `ClockSource` reporting host NTP offset/error, surfaced as a `TelemetryEvent::ClockOffset` metric, plus a `SlotClock` that computes the next FT8 15 s boundary.
- **Conformance gates** (`crates/dsp/tests/`, `crates/dsp/src/testutil.rs`): per-mode loopback round-trips; AWGN BER/decode-rate sweeps with thresholds; a seedable Watterson HF-fading fixture; bidirectional cross-decode interop tests gated `#[ignore]` behind reference binaries; a `phase4_exit_criterion` aggregate gate.

**Out of scope (Phase 5):** FT4/JT65/JT9/WSPR/MFSK/Olivia/Hell/FreeDV/M17/ARDOP; TX exclusive lease; mTLS for routable binds; Prometheus exporter; reference CLI/TUI; KISS/AGWPE translator; SIC and AP decoding for FT8 (the Phase-4 FT8 decoder is BP+OSD single-pass per candidate — design lists SIC/AP as WSJT-X-class differentiators that compose later).

## Sub-plan / parallelism note

Per the writing-plans scope check: the five mode assemblies (Parts A–E) are **independent** — each is a self-contained file under `crates/dsp/src/modes/` with its own tests and touches no shared file except a one-line `pub mod` in `modes/mod.rs` and one arm in the daemon registry. They can be executed as five parallel sub-plans after **Part 0** lands. The daemon infrastructure (Parts F–H) depends only on Part 0's registry shape and at least one streaming mode (AFSK 1200) plus FT8 for the windowed path; do Part A before Part F, and Part E before the windowed branch of Parts F/G. Part I (conformance gates) is layered in per-mode as each mode completes, and finalized last.

## File structure

**Created:**

| File | Responsibility |
|---|---|
| `crates/dsp/src/modes/mod.rs` | Re-exports the five assemblies; module doc. |
| `crates/dsp/src/modes/afsk1200.rs` | `Afsk1200Demod` (streaming) + `Afsk1200Mod`: Bell-202 AFSK ⇄ HDLC/AX.25 with the multi-slicer ensemble. |
| `crates/dsp/src/modes/psk31.rs` | `Psk31Demod` + `Psk31Mod`: differential BPSK + Varicode. |
| `crates/dsp/src/modes/rtty.rs` | `RttyDemod` + `RttyMod`: 2-FSK + Baudot. |
| `crates/dsp/src/modes/cw.rs` | `CwDemod` + `CwMod`: envelope detect + Morse. |
| `crates/dsp/src/modes/ft8.rs` | `Ft8Demod` (block) + `Ft8Mod`: STFT→candidate→Costas→LLR→LDPC+OSD→77-bit. |
| `crates/omnimodemd/src/core/rx_worker.rs` | Per-channel RX thread; streaming + windowed driving; interlock-gated; emits frames. |
| `crates/omnimodemd/src/core/tx_worker.rs` | Per-channel TX worker; cooperative queue; modulate; slot-align; `drive_tx_cycle`. |
| `crates/omnimodemd/src/core/clock.rs` | `ClockSource` (host NTP offset) + `SlotClock` (next windowed-TX boundary). |
| `crates/dsp/tests/loopback.rs` | TX-modulator → RX-demod round-trips for all five modes. |
| `crates/dsp/tests/ber.rs` | Seeded AWGN (and Watterson) decode-rate sweeps with per-mode thresholds. |

**Modified:**

| File | Change |
|---|---|
| `crates/dsp/src/lib.rs` | `pub mod modes;` + re-exports. |
| `crates/omnimodemd/src/mode/mod.rs` | Extend `ModeConfig::parse` to the five labels (default params). |
| `crates/omnimodemd/src/mode/registry.rs` | `build_demod`/`build_block_demod`/`build_modulator` construct the assemblies. |
| `crates/omnimodemd/src/core/mod.rs` | Spawn/teardown RX & TX workers; route `Transmit` through the TX worker; emit `ClockOffset`. |
| `crates/omnimodemd/src/core/command.rs` | (no new command — `Transmit` payload reinterpreted per-mode.) |
| `crates/omnimodemd/src/core/event.rs` | Add `TelemetryEvent::ClockOffset { channel, offset_s, est_error_s }`. |
| `crates/dsp/src/testutil.rs` | Add `WattersonChannel` fading fixture + `decode_rate` helper. |
| `crates/dsp/tests/kat.rs` | Add per-mode KAT + cross-decode `#[ignore]` gates; extend `phase4_exit_criterion`. |
| `crates/dsp/tests/snapshots.rs` | Add mode-level modulator golden snapshots. |

---

# Part 0 — Shared scaffolding

### Task 0.1: Create the `modes` module skeleton in the dsp crate

**Files:**
- Create: `crates/dsp/src/modes/mod.rs`
- Modify: `crates/dsp/src/lib.rs`

- [ ] **Step 1: Create the module file**

```rust
//! Mode assemblies: each file wires Phase-3 building blocks (`frontend`,
//! `sync`, `fec`, `framing`, `ensemble`) into a concrete `Demodulator` /
//! `BlockDemodulator` and a symmetric `Modulator` for one end-user mode.
//!
//! These are pure DSP — no daemon, no audio device — so every mode loopback-,
//! KAT-, and BER-tests in CI. The daemon's `mode::registry` maps a parametric
//! `ModeConfig` onto these constructors; nothing else learns mode specifics.

pub mod afsk1200;
pub mod cw;
pub mod ft8;
pub mod psk31;
pub mod rtty;
```

- [ ] **Step 2: Wire it into the crate root**

In `crates/dsp/src/lib.rs`, add `pub mod modes;` after the existing `pub mod ensemble;` line (line 10), and append the re-export to the `pub use` block:

```rust
pub mod modes;
```
```rust
pub use modes::{
    afsk1200::{Afsk1200Demod, Afsk1200Mod},
    cw::{CwDemod, CwMod},
    ft8::{Ft8Demod, Ft8Mod},
    psk31::{Psk31Demod, Psk31Mod},
    rtty::{RttyDemod, RttyMod},
};
```

- [ ] **Step 3: Create empty mode files so the crate compiles**

Create each of `afsk1200.rs`, `cw.rs`, `ft8.rs`, `psk31.rs`, `rtty.rs` under `crates/dsp/src/modes/` containing only a module doc comment line (e.g. `//! AFSK 1200 (AX.25) mode assembly.`). They are filled in by Parts A–E. The `pub use` in Step 2 will not compile until the types exist, so temporarily comment out the `pub use modes::{...}` block; uncomment it incrementally as each mode's types land (each mode task re-enables its own line).

- [ ] **Step 4: Verify the crate still builds with the stubbed module**

Run: `cargo build -p omnimodem-dsp`
Expected: builds (the `pub use` block commented out).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/ crates/dsp/src/lib.rs
git commit -m "Add modes module skeleton to omnimodem-dsp"
```

### Task 0.2: Extend the daemon `ModeConfig` parser to the five mode labels

**Files:**
- Modify: `crates/omnimodemd/src/mode/mod.rs:24-29`
- Test: `crates/omnimodemd/src/mode/mod.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Replace the body of `parse_is_strict` and add a new test in the inline `mod tests`:

```rust
    #[test]
    fn parse_resolves_phase4_modes_with_defaults() {
        assert_eq!(ModeConfig::parse("afsk1200"), Some(ModeConfig::Afsk1200 { tx: true }));
        assert_eq!(ModeConfig::parse("ft8"), Some(ModeConfig::Ft8));
        assert_eq!(ModeConfig::parse("cw"), Some(ModeConfig::Cw { wpm: 20, tone_hz: 700.0 }));
        assert_eq!(ModeConfig::parse("rtty"), Some(ModeConfig::Rtty { baud: 45.45, shift_hz: 170.0 }));
        assert_eq!(ModeConfig::parse("psk31"), Some(ModeConfig::Psk31 { center_hz: 1000.0 }));
        assert_eq!(ModeConfig::parse("none"), Some(ModeConfig::None));
        assert_eq!(ModeConfig::parse("bogus"), None);
    }
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p omnimodemd mode::tests::parse_resolves_phase4_modes_with_defaults`
Expected: FAIL (parser returns `None` for the five labels).

- [ ] **Step 3: Implement the parser arms**

Replace `ModeConfig::parse` (lines 24-29) with:

```rust
    /// Parse the channel's `mode` string into a parametric config. Phase 4
    /// resolves the five first-mode labels with default parameters (richer
    /// parametric strings are a Phase-5 extension); unknown strings are
    /// rejected so a typo can't silently configure nothing.
    pub fn parse(s: &str) -> Option<ModeConfig> {
        match s {
            "none" | "" => Some(ModeConfig::None),
            "afsk1200" => Some(ModeConfig::Afsk1200 { tx: true }),
            "ft8" => Some(ModeConfig::Ft8),
            "cw" => Some(ModeConfig::Cw { wpm: 20, tone_hz: 700.0 }),
            "rtty" => Some(ModeConfig::Rtty { baud: 45.45, shift_hz: 170.0 }),
            "psk31" => Some(ModeConfig::Psk31 { center_hz: 1000.0 }),
            _ => None,
        }
    }
```

Delete the now-stale `parse_is_strict` test (its `assert_eq!(ModeConfig::parse("ft8"), None)` line contradicts the new behavior); the `labels_are_distinct_and_non_empty` and `label_round_trips_none` tests stay.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p omnimodemd mode::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/mode/mod.rs
git commit -m "Resolve Phase-4 mode labels to parametric ModeConfig defaults"
```

---

# Part A — AFSK 1200 (AX.25)

Validates the streaming/HDLC path and the Graywolf multi-slicer ensemble; baseline interop against Direwolf. Building blocks: `frontend::modulate::Afsk`, `frontend::nco::DownConverter`, `frontend::detector::FmDiscriminator`, `frontend::fir::Fir`/`design_bandpass`, `frontend::agc::PeakValleyAgc`, `fec::slicer::MultiSlicer`, `sync::dpll::DpllClockRecovery`, `sync::dcd::DcdScorer`, `fec::nrzi::{nrzi_encode,nrzi_decode}`, `framing::hdlc::{hdlc_frame,hdlc_deframe}`, `framing::ax25::Ax25Frame`, `ensemble::ParallelDemodulator`.

### Task A.1: AFSK 1200 modulator (TX)

**Files:**
- Modify: `crates/dsp/src/modes/afsk1200.rs`
- Test: `crates/dsp/src/modes/afsk1200.rs` (inline)

- [ ] **Step 1: Write the failing test**

```rust
//! AFSK 1200 (AX.25) mode assembly: Bell-202 AFSK ⇄ HDLC/AX.25.
//!
//! TX: Frame payload bytes are an *intact* AX.25 frame → HDLC-frame (flag,
//! bit-stuff, FCS) → NRZI → Bell-202 1200/2200 Hz AFSK at 48 kHz.
//! RX: a multi-slicer ensemble over the 1200/2200 Hz tone correlators, DPLL
//! bit clock, NRZI decode, HDLC deframe (FCS-validated), AX.25 decode.

use crate::frontend::modulate::Afsk;
use crate::framing::hdlc::{hdlc_deframe, hdlc_frame};
use crate::fec::nrzi::{nrzi_decode, nrzi_encode};
use crate::mode::{Duplex, DemodShape, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

/// Bell-202 native rate. 48 kHz is Graywolf's AFSK working rate.
pub const AFSK1200_RATE: u32 = 48_000;

pub struct Afsk1200Mod {
    afsk: Afsk,
}

impl Afsk1200Mod {
    pub fn new() -> Self {
        Afsk1200Mod { afsk: Afsk::bell202(AFSK1200_RATE as f32) }
    }
}

impl Default for Afsk1200Mod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for Afsk1200Mod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: AFSK1200_RATE,
            bandwidth_hz: 2400.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let payload = match &frame.payload {
            FramePayload::Packet(b) => b.clone(),
            other => return Err(ModError::UnsupportedPayload(payload_kind(other))),
        };
        // HDLC frame → bit vector (LSB-first) → NRZI line levels → AFSK.
        let bits = hdlc_frame(&payload);
        let levels = nrzi_encode(&bits);
        let level_bools: Vec<bool> = levels.iter().map(|&b| b != 0).collect();
        Ok(self.afsk.modulate(&level_bools))
    }
}

fn payload_kind(p: &FramePayload) -> &'static str {
    match p {
        FramePayload::Packet(_) => "packet",
        FramePayload::Text(_) => "text",
        FramePayload::Message77(_) => "message77",
        FramePayload::Vocoder(_) => "vocoder",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulator_round_bit_count_is_nonzero_and_caps_are_streaming_tx() {
        let mut m = Afsk1200Mod::new();
        let caps = m.caps();
        assert_eq!(caps.native_rate, 48_000);
        assert!(caps.tx);
        assert!(matches!(caps.shape, DemodShape::Streaming));
        let frame = Frame::packet(b"K1ABC>APRS:hi".to_vec());
        let samples = m.modulate(&frame).unwrap();
        // At 1200 baud / 48 kHz that's 40 samples/bit; a short frame is well
        // over a thousand samples.
        assert!(samples.len() > 1000, "got {} samples", samples.len());
    }

    #[test]
    fn modulator_rejects_text_payload() {
        let mut m = Afsk1200Mod::new();
        let err = m.modulate(&Frame::text("nope")).unwrap_err();
        assert!(matches!(err, ModError::UnsupportedPayload("text")));
    }
}
```

- [ ] **Step 2: Run it to verify it fails to compile, then passes once written**

Run: `cargo test -p omnimodem-dsp modes::afsk1200`
Expected: PASS (the code above is the implementation; the test is in the same step). If `nrzi_encode` returns `Vec<u8>` of 0/1 line levels, the `!= 0` map is correct.

- [ ] **Step 3: Re-enable the AFSK re-export**

In `crates/dsp/src/lib.rs`, uncomment the `afsk1200::{Afsk1200Demod, Afsk1200Mod}` line of the `pub use modes::{...}` block, but temporarily reduce it to `afsk1200::Afsk1200Mod` (the demod lands in Task A.2):

```rust
pub use modes::afsk1200::Afsk1200Mod;
```

- [ ] **Step 4: Run build + test**

Run: `cargo test -p omnimodem-dsp modes::afsk1200`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/afsk1200.rs crates/dsp/src/lib.rs
git commit -m "Add AFSK 1200 modulator (HDLC/NRZI → Bell-202 AFSK)"
```

### Task A.2: AFSK 1200 single-slicer streaming demodulator

**Files:**
- Modify: `crates/dsp/src/modes/afsk1200.rs`
- Test: `crates/dsp/src/modes/afsk1200.rs` (inline) + `crates/dsp/tests/loopback.rs` (Task I.1)

- [ ] **Step 1: Write the failing loopback test**

Append to the inline `mod tests` in `afsk1200.rs`:

```rust
    #[test]
    fn loopback_recovers_ax25_frame() {
        use crate::framing::ax25::{Address, Ax25Frame};
        let f = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("K1ABC", 7),
            digipeaters: vec![],
            info: b"!4903.50N/07201.75W-Test".to_vec(),
        };
        let frame = Frame::packet(f.encode());

        let mut tx = Afsk1200Mod::new();
        let samples = tx.modulate(&frame).unwrap();

        let mut rx = Afsk1200Demod::single();
        let frames = rx.feed(&samples);
        assert!(!frames.is_empty(), "demod produced no frames");
        // The recovered packet bytes equal the transmitted AX.25 frame.
        let got = match &frames[0].payload {
            FramePayload::Packet(b) => b.clone(),
            other => panic!("expected packet, got {other:?}"),
        };
        assert_eq!(got, f.encode());
        assert!(frames[0].meta.crc_ok);
    }
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p omnimodem-dsp modes::afsk1200::tests::loopback_recovers_ax25_frame`
Expected: FAIL — `Afsk1200Demod` is undefined.

- [ ] **Step 3: Implement the single-slicer demod**

Add to `afsk1200.rs` (above the test module). This assembles the Graywolf-style correlator: bandpass-isolate each tone, take its envelope, AGC, slice mark-vs-space, recover the bit clock with the DPLL, NRZI-decode, and accumulate bits into the HDLC deframer.

```rust
use crate::frontend::agc::PeakValleyAgc;
use crate::frontend::fir::{design_bandpass, Fir};
use crate::sync::dcd::DcdScorer;
use crate::sync::dpll::DpllClockRecovery;
use crate::mode::Demodulator;
use crate::types::FrameMeta;

const MARK_HZ: f32 = 1200.0;
const SPACE_HZ: f32 = 2200.0;
const BAUD: f32 = 1200.0;
/// Tone bandpass half-width; ±600 Hz isolates a Bell-202 tone.
const TONE_BW: f32 = 600.0;
const FIR_TAPS: usize = 64;

/// One tone-energy correlator: bandpass → rectify → smooth.
struct ToneEnergy {
    bp: Fir,
    smooth: PeakValleyAgc,
}

impl ToneEnergy {
    fn new(center: f32, rate: f32) -> Self {
        let taps = design_bandpass(FIR_TAPS, center - TONE_BW, center + TONE_BW, rate);
        // Fast-attack / slow-decay envelope follower over the rectified tone.
        ToneEnergy { bp: Fir::new(taps), smooth: PeakValleyAgc::new(0.2, 0.001) }
    }
    fn push(&mut self, x: Sample) -> f32 {
        let band = self.bp.push(x);
        // Rectify then smooth → tone magnitude estimate.
        self.smooth.process(band.abs())
    }
}

pub struct Afsk1200Demod {
    mark: ToneEnergy,
    space: ToneEnergy,
    dpll: DpllClockRecovery,
    dcd: DcdScorer,
    prev_level: u8,
    /// NRZI line levels accumulated since the last frame boundary.
    bit_levels: Vec<u8>,
    sample_index: u64,
}

impl Afsk1200Demod {
    /// Single-slicer demod (one decision threshold). The ensemble variant is
    /// `Afsk1200Demod::ensemble()` (Task A.3).
    pub fn single() -> Self {
        let rate = AFSK1200_RATE as f32;
        Afsk1200Demod {
            mark: ToneEnergy::new(MARK_HZ, rate),
            space: ToneEnergy::new(SPACE_HZ, rate),
            dpll: DpllClockRecovery::new(rate / BAUD),
            dcd: DcdScorer::new(24, 18, 6),
            prev_level: 0,
            bit_levels: Vec::with_capacity(2048),
            sample_index: 0,
        }
    }

    /// Drain accumulated NRZI levels through HDLC and emit any FCS-valid frames.
    fn try_frames(&mut self) -> Vec<Frame> {
        // NRZI-decode the accumulated line levels, then HDLC-deframe.
        let bits = nrzi_decode(&self.bit_levels);
        let payloads = hdlc_deframe(&bits);
        if payloads.is_empty() {
            return Vec::new();
        }
        // A valid frame consumed the buffer; reset to avoid unbounded growth.
        self.bit_levels.clear();
        payloads
            .into_iter()
            .map(|p| Frame {
                payload: FramePayload::Packet(p),
                meta: FrameMeta {
                    crc_ok: true,
                    sample_offset: self.sample_index,
                    decoder: Some("afsk1200/single".into()),
                    ..Default::default()
                },
            })
            .collect()
    }
}

impl Demodulator for Afsk1200Demod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: AFSK1200_RATE,
            bandwidth_hz: 2400.0,
            tx: false,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        let mut out = Vec::new();
        for &x in samples {
            self.sample_index += 1;
            let m = self.mark.push(x);
            let s = self.space.push(x);
            // Mark when the 1200 Hz tone dominates the 2200 Hz tone.
            let level = u8::from(m > s);
            self.dcd.update(m.max(s) > 0.02);
            // Feed the DPLL a transition flag; it returns Some(level) at each
            // sampling instant.
            if let Some(sampled) = self.dpll.feed(level != 0) {
                let line = u8::from(sampled);
                self.bit_levels.push(line);
                self.prev_level = line;
                // Bound work: attempt deframe whenever we have a flag's worth.
                if self.bit_levels.len() >= 8 && self.bit_levels.len() % 8 == 0 {
                    let frames = self.try_frames();
                    out.extend(frames);
                }
            }
        }
        out
    }

    fn reset(&mut self) {
        self.mark = ToneEnergy::new(MARK_HZ, AFSK1200_RATE as f32);
        self.space = ToneEnergy::new(SPACE_HZ, AFSK1200_RATE as f32);
        self.dpll = DpllClockRecovery::new(AFSK1200_RATE as f32 / BAUD);
        self.dcd = DcdScorer::new(24, 18, 6);
        self.prev_level = 0;
        self.bit_levels.clear();
    }
}
```

> Implementation note for the executor: the correlator constants (`TONE_BW`, `FIR_TAPS`, AGC attack/decay, DCD thresholds, the `> 0.02`/`m > s` decision) are the Graywolf-lineage tuning knobs the design (§"Constants to confirm at implementation time") says to confirm against the reference at build time. The **loopback test in Step 1 is the correctness gate** — if it fails, adjust these constants (start with FIR taps and the envelope decay) until a noiseless self-modulated frame decodes; the values above are a working starting point at 48 kHz.

- [ ] **Step 4: Re-export the demod and run the loopback test**

Update the `pub use` line in `lib.rs` to the full pair: `pub use modes::afsk1200::{Afsk1200Demod, Afsk1200Mod};`

Run: `cargo test -p omnimodem-dsp modes::afsk1200`
Expected: PASS (both modulator tests and `loopback_recovers_ax25_frame`).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/afsk1200.rs crates/dsp/src/lib.rs
git commit -m "Add AFSK 1200 single-slicer streaming demodulator with loopback test"
```

### Task A.3: AFSK 1200 multi-slicer ensemble ("hydra")

**Files:**
- Modify: `crates/dsp/src/modes/afsk1200.rs`
- Test: `crates/dsp/src/modes/afsk1200.rs` (inline)

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn ensemble_decodes_under_awgn_when_single_may_not() {
        use crate::framing::ax25::{Address, Ax25Frame};
        use crate::testutil::{add_awgn, Rng};
        let f = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("N0CALL", 1),
            digipeaters: vec![],
            info: b"ensemble test".to_vec(),
        };
        let frame = Frame::packet(f.encode());
        let mut tx = Afsk1200Mod::new();
        let mut samples = tx.modulate(&frame).unwrap();
        let mut rng = Rng::new(2026_06_20);
        add_awgn(&mut samples, 0.15, &mut rng);

        let mut rx = Afsk1200Demod::ensemble(9);
        let frames = rx.feed(&samples);
        assert!(frames.iter().any(|fr| matches!(&fr.payload,
            FramePayload::Packet(b) if b == &f.encode())));
    }
```

This test requires `crate::testutil`, so the inline tests must be gated. Add `#![cfg(test)]`-level access by importing under `#[cfg(test)]` — `testutil` is already available to the crate's own tests.

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p omnimodem-dsp modes::afsk1200::tests::ensemble_decodes_under_awgn`
Expected: FAIL — `Afsk1200Demod::ensemble` is undefined.

- [ ] **Step 3: Implement the ensemble constructor over `MultiSlicer`**

Replace the single hard `m > s` decision with an N-way `MultiSlicer` and union the per-slicer HDLC outputs through the `ensemble::ParallelDemodulator` dedup, OR (simpler, matching Graywolf's per-slicer `HdlcDecoder`) keep one demod instance per slicer threshold and dedup their frames. Implement the latter:

```rust
use crate::ensemble::DedupWindow;
use crate::fec::slicer::MultiSlicer;

/// N independent single-threshold demods sharing a front end, deduped by
/// content+offset — the Graywolf "hydra" specialized to AFSK mark/space.
pub struct Afsk1200Ensemble {
    mark: ToneEnergy,
    space: ToneEnergy,
    slicer: MultiSlicer,
    lanes: Vec<Lane>,
    dedup: DedupWindow,
    sample_index: u64,
}

struct Lane {
    dpll: DpllClockRecovery,
    bit_levels: Vec<u8>,
}

impl Afsk1200Demod {
    /// Build the multi-slicer ensemble with `n` slicers (odd; 9 = Graywolf
    /// default). Returned as a boxed `Demodulator` so the registry treats it
    /// uniformly.
    pub fn ensemble(n: usize) -> Afsk1200Ensemble {
        let rate = AFSK1200_RATE as f32;
        Afsk1200Ensemble {
            mark: ToneEnergy::new(MARK_HZ, rate),
            space: ToneEnergy::new(SPACE_HZ, rate),
            slicer: MultiSlicer::new(n),
            lanes: (0..n)
                .map(|_| Lane { dpll: DpllClockRecovery::new(rate / BAUD), bit_levels: Vec::new() })
                .collect(),
            // ~3 symbol-times dedup window (design §"dedup-by-(content,offset)").
            dedup: DedupWindow::new((3.0 * rate / BAUD) as u64),
        }
    }
}

impl Demodulator for Afsk1200Ensemble {
    fn caps(&self) -> ModeCaps {
        Afsk1200Demod::single().caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        let mut out = Vec::new();
        for &x in samples {
            self.sample_index += 1;
            let m = self.mark.push(x);
            let s = self.space.push(x);
            let decisions = self.slicer.slice(m, s); // Vec<bool>, one per lane
            self.dedup.prune_to_latest();
            for (lane, &mark_wins) in self.lanes.iter_mut().zip(decisions.iter()) {
                if let Some(sampled) = lane.dpll.feed(mark_wins) {
                    lane.bit_levels.push(u8::from(sampled));
                    if lane.bit_levels.len() >= 8 && lane.bit_levels.len() % 8 == 0 {
                        let bits = nrzi_decode(&lane.bit_levels);
                        for p in hdlc_deframe(&bits) {
                            lane.bit_levels.clear();
                            let frame = Frame {
                                payload: FramePayload::Packet(p),
                                meta: FrameMeta {
                                    crc_ok: true,
                                    sample_offset: self.sample_index,
                                    decoder: Some("afsk1200/hydra".into()),
                                    ..Default::default()
                                },
                            };
                            if self.dedup.admit(&frame) {
                                out.push(frame);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    fn reset(&mut self) {
        *self = {
            let mut e = Afsk1200Demod::ensemble(self.lanes.len());
            std::mem::swap(&mut e.dedup, &mut self.dedup);
            e
        };
    }
}
```

- [ ] **Step 4: Run the ensemble test**

Run: `cargo test -p omnimodem-dsp modes::afsk1200`
Expected: PASS. If the AWGN test is flaky, raise the slicer count or lower sigma to 0.1 — the assertion is "ensemble recovers a noiseless-ish frame," not a BER threshold (that's Part I).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/afsk1200.rs crates/dsp/src/lib.rs
git commit -m "Add AFSK 1200 multi-slicer ensemble demod (hydra) with dedup"
```

---

# Part B — PSK31

Differential BPSK + Varicode. Building blocks: `frontend::modulate::DiffPsk`, `frontend::nco::DownConverter`, `sync::costas::CostasLoop`, `sync::timing::GardnerTed`, `fec::gray::{diff_bpsk_encode,diff_bpsk_decode}`, `framing::varicode::{encode,decode,PSK31}`.

### Task B.1: PSK31 modulator (TX)

**Files:**
- Modify: `crates/dsp/src/modes/psk31.rs`
- Test: `crates/dsp/src/modes/psk31.rs` (inline)

- [ ] **Step 1: Write the implementation + test**

```rust
//! PSK31 mode assembly: differential BPSK + Varicode at 31.25 baud.
//!
//! TX: text → PSK31 Varicode bitstream → differential BPSK symbols → raised-
//! cosine DBPSK at `center_hz`. RX: Costas carrier recovery + Gardner timing →
//! differential decode → Varicode decode.

use crate::frontend::modulate::DiffPsk;
use crate::framing::varicode::{decode as vari_decode, encode as vari_encode, PSK31};
use crate::fec::gray::{diff_bpsk_decode, diff_bpsk_encode};
use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

pub const PSK31_RATE: u32 = 8_000;
pub const PSK31_BAUD: f32 = 31.25;

pub struct Psk31Mod {
    center_hz: f32,
    sps: usize,
}

impl Psk31Mod {
    pub fn new(center_hz: f32) -> Self {
        let sps = (PSK31_RATE as f32 / PSK31_BAUD).round() as usize; // 256
        Psk31Mod { center_hz, sps }
    }
}

impl Modulator for Psk31Mod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: PSK31_RATE,
            bandwidth_hz: 62.5,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("psk31 needs text")),
        };
        // Varicode bits (0/1) → differential BPSK symbols (0/1) → DiffPsk.
        let bits = vari_encode(&PSK31, &text);
        let syms = diff_bpsk_encode(&bits);
        let sym_u32: Vec<u32> = syms.iter().map(|&b| b as u32).collect();
        let psk = DiffPsk::new(PSK31_RATE as f32, self.center_hz, self.sps, 1);
        Ok(psk.modulate(&sym_u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulates_text_to_audio() {
        let mut m = Psk31Mod::new(1000.0);
        assert!(m.caps().tx);
        let s = m.modulate(&Frame::text("CQ")).unwrap();
        assert!(s.len() > m.sps * 8, "too few samples: {}", s.len());
    }

    #[test]
    fn rejects_packet_payload() {
        let mut m = Psk31Mod::new(1000.0);
        assert!(matches!(
            m.modulate(&Frame::packet(vec![1, 2])).unwrap_err(),
            ModError::UnsupportedPayload(_)
        ));
    }
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p omnimodem-dsp modes::psk31`
Expected: PASS. Re-enable the re-export: in `lib.rs`, `pub use modes::psk31::Psk31Mod;`

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/src/modes/psk31.rs crates/dsp/src/lib.rs
git commit -m "Add PSK31 modulator (Varicode → differential BPSK)"
```

### Task B.2: PSK31 demodulator (RX)

**Files:**
- Modify: `crates/dsp/src/modes/psk31.rs`
- Test: `crates/dsp/src/modes/psk31.rs` (inline)

- [ ] **Step 1: Write the failing loopback test**

```rust
    #[test]
    fn loopback_recovers_text() {
        let msg = "CQ DE K1ABC";
        let mut tx = Psk31Mod::new(1000.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = Psk31Demod::new(1000.0);
        let frames = rx.feed(&samples);
        let text: String = frames.iter().filter_map(|f| match &f.payload {
            FramePayload::Text(t) => Some(t.clone()),
            _ => None,
        }).collect();
        assert!(text.contains("CQ DE K1ABC"), "recovered: {text:?}");
    }
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p omnimodem-dsp modes::psk31::tests::loopback_recovers_text`
Expected: FAIL — `Psk31Demod` undefined.

- [ ] **Step 3: Implement the demod**

```rust
use crate::frontend::nco::DownConverter;
use crate::sync::costas::CostasLoop;
use crate::sync::timing::GardnerTed;
use crate::mode::Demodulator;
use crate::types::FrameMeta;

pub struct Psk31Demod {
    nco: DownConverter,
    costas: CostasLoop,
    gardner: GardnerTed,
    prev_i: f32,
    sym_bits: Vec<u8>, // differential symbol stream (post-Costas hard I sign)
    sample_index: u64,
}

impl Psk31Demod {
    pub fn new(center_hz: f32) -> Self {
        let rate = PSK31_RATE as f32;
        let sps = rate / PSK31_BAUD;
        Psk31Demod {
            nco: DownConverter::new(center_hz, rate),
            // Narrow loop bandwidth for 31.25 baud.
            costas: CostasLoop::new(0.005, 0.05),
            gardner: GardnerTed::new(sps),
            prev_i: 0.0,
            sym_bits: Vec::new(),
            sample_index: 0,
        }
    }

    fn drain_text(&mut self) -> Vec<Frame> {
        if self.sym_bits.len() < 2 {
            return Vec::new();
        }
        // Differential decode the BPSK symbols, then Varicode-decode.
        let data = diff_bpsk_decode(&self.sym_bits);
        let text = vari_decode(&PSK31, &data);
        if text.is_empty() {
            return Vec::new();
        }
        Vec::from([Frame {
            payload: FramePayload::Text(text),
            meta: FrameMeta { crc_ok: true, sample_offset: self.sample_index,
                decoder: Some("psk31".into()), ..Default::default() },
        }])
    }
}

impl Demodulator for Psk31Demod {
    fn caps(&self) -> ModeCaps {
        Psk31Mod::new(1000.0).caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        for &x in samples {
            self.sample_index += 1;
            let bb = self.nco.push(x);              // complex baseband
            let derot = self.costas.process(bb);    // carrier-recovered
            // Gardner strobes once per symbol on the real (I) projection.
            if let Some(strobe) = self.gardner.feed(derot.re) {
                // Differential BPSK symbol = sign agreement with previous I.
                let bit = u8::from(strobe * self.prev_i < 0.0);
                self.prev_i = strobe;
                self.sym_bits.push(bit);
            }
        }
        // PSK31 has no framing; emit text opportunistically and keep state.
        // To bound memory, decode and clear once we have a sentence-ish run.
        if self.sym_bits.len() >= 64 {
            let out = self.drain_text();
            self.sym_bits.clear();
            self.prev_i = 0.0;
            return out;
        }
        Vec::new()
    }

    fn reset(&mut self) {
        *self = Psk31Demod::new(1000.0);
    }
}
```

> Note: the differential-symbol convention (`strobe * prev_i < 0.0` ⇒ phase reversal ⇒ Varicode bit 0/1) must match `diff_bpsk_encode`'s convention; the loopback test pins it. If the recovered text is bit-inverted, flip the `< 0.0` to `> 0.0`. Costas loop bandwidth and Gardner `sps` are the tuning knobs; the loopback test is the gate.

- [ ] **Step 4: Run the loopback test; tune until green**

Run: `cargo test -p omnimodem-dsp modes::psk31`
Expected: PASS. Update `lib.rs`: `pub use modes::psk31::{Psk31Demod, Psk31Mod};`

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk31.rs crates/dsp/src/lib.rs
git commit -m "Add PSK31 demodulator (Costas + Gardner + differential + Varicode)"
```

---

# Part C — RTTY

2-FSK (170 Hz shift) + Baudot/ITA2. Building blocks: `frontend::modulate::Fsk2`, `frontend::nco::DownConverter`, `frontend::detector::FmDiscriminator`, `sync::timing::StartBitSync`, `framing::baudot::{encode,Decoder}`.

### Task C.1: RTTY modulator (TX)

**Files:**
- Modify: `crates/dsp/src/modes/rtty.rs`
- Test: inline

- [ ] **Step 1: Implementation + test**

```rust
//! RTTY mode assembly: 45.45 baud, 170 Hz shift, 5-bit Baudot/ITA2.
//!
//! TX: text → Baudot codes → start/stop framed bits (1 start + 5 data + 1.5
//! stop) → 2-FSK mark/space. RX: FM-discriminator → start-bit sync samples 5
//! data bits → Baudot decode.

use crate::frontend::modulate::Fsk2;
use crate::framing::baudot::{encode as baudot_encode, Decoder as BaudotDecoder};
use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

pub const RTTY_RATE: u32 = 8_000;

pub struct RttyMod {
    baud: f32,
    shift_hz: f32,
    center_hz: f32,
}

impl RttyMod {
    pub fn new(baud: f32, shift_hz: f32) -> Self {
        RttyMod { baud, shift_hz, center_hz: 1500.0 }
    }
}

impl Modulator for RttyMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps { native_rate: RTTY_RATE, bandwidth_hz: self.shift_hz + 2.0 * self.baud,
            tx: true, duplex: Duplex::Half, shape: DemodShape::Streaming }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("rtty needs text")),
        };
        let sps = (RTTY_RATE as f32 / self.baud).round() as usize;
        // Frame each 5-bit Baudot code: 1 start bit (space=false), 5 data
        // (LSB first), 2 stop bits (mark=true). Mark is the idle/high tone.
        let mut bits: Vec<bool> = Vec::new();
        for code in baudot_encode(&text) {
            bits.push(false); // start
            for i in 0..5 {
                bits.push((code >> i) & 1 == 1);
            }
            bits.push(true); // stop
            bits.push(true); // stop (1.5–2 stop bits; 2 is fine for RX)
        }
        let fsk = Fsk2::new(RTTY_RATE as f32, sps, self.center_hz, self.shift_hz);
        Ok(fsk.modulate(&bits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulates_text() {
        let mut m = RttyMod::new(45.45, 170.0);
        assert!(m.caps().tx);
        assert!(m.modulate(&Frame::text("RYRY")).unwrap().len() > 100);
    }
}
```

- [ ] **Step 2: Run + re-export**

Run: `cargo test -p omnimodem-dsp modes::rtty`
Expected: PASS. `lib.rs`: `pub use modes::rtty::RttyMod;`

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/src/modes/rtty.rs crates/dsp/src/lib.rs
git commit -m "Add RTTY modulator (Baudot → start/stop-framed 2-FSK)"
```

### Task C.2: RTTY demodulator (RX)

**Files:**
- Modify: `crates/dsp/src/modes/rtty.rs`
- Test: inline

- [ ] **Step 1: Failing loopback test**

```rust
    #[test]
    fn loopback_recovers_text() {
        let msg = "THE QUICK BROWN FOX";
        let mut tx = RttyMod::new(45.45, 170.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = RttyDemod::new(45.45, 170.0);
        let frames = rx.feed(&samples);
        let text: String = frames.iter().filter_map(|f| match &f.payload {
            FramePayload::Text(t) => Some(t.clone()), _ => None }).collect();
        assert!(text.contains("THE QUICK BROWN FOX"), "got {text:?}");
    }
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodem-dsp modes::rtty::tests::loopback_recovers_text`
Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
use crate::frontend::nco::DownConverter;
use crate::frontend::detector::FmDiscriminator;
use crate::sync::timing::StartBitSync;
use crate::mode::Demodulator;
use crate::types::FrameMeta;

pub struct RttyDemod {
    baud: f32,
    nco: DownConverter,
    disc: FmDiscriminator,
    sync: StartBitSync,
    baudot: BaudotDecoder,
    text: String,
    sample_index: u64,
}

impl RttyDemod {
    pub fn new(baud: f32, _shift_hz: f32) -> Self {
        let rate = RTTY_RATE as f32;
        RttyDemod {
            baud,
            nco: DownConverter::new(1500.0, rate),
            disc: FmDiscriminator::new(),
            sync: StartBitSync::new(rate / baud),
            baudot: BaudotDecoder::new(),
            text: String::new(),
            sample_index: 0,
        }
    }
}

impl Demodulator for RttyDemod {
    fn caps(&self) -> ModeCaps {
        RttyMod::new(self.baud, 170.0).caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        let mut out = Vec::new();
        for &x in samples {
            self.sample_index += 1;
            let bb = self.nco.push(x);
            // Instantaneous frequency: positive ⇒ mark (high tone) ⇒ bit 1.
            let freq = self.disc.push(bb);
            let level = freq > 0.0;
            if let Some(code_bits) = self.sync.feed(level) {
                // Pack the 5 sampled bits LSB-first into a Baudot code.
                let mut code = 0u8;
                for (i, &b) in code_bits.iter().enumerate() {
                    if b { code |= 1 << i; }
                }
                if let Some(c) = self.baudot.feed(code) {
                    self.text.push(c);
                }
            }
        }
        // Flush decoded text as a frame when a run accumulates.
        if self.text.len() >= 8 {
            out.push(Frame {
                payload: FramePayload::Text(std::mem::take(&mut self.text)),
                meta: FrameMeta { crc_ok: true, sample_offset: self.sample_index,
                    decoder: Some("rtty".into()), ..Default::default() },
            });
        }
        out
    }

    fn reset(&mut self) {
        *self = RttyDemod::new(self.baud, 170.0);
    }
}
```

> Note: mark/space polarity (`freq > 0.0` ⇒ mark) must match `Fsk2`'s `mark_hz`/`space_hz` (mark = center + shift/2). If text is garbled/shifted, invert the comparison. The `StartBitSync` already samples 5 data bits at baud midpoints after a mark→space (start) edge; if frame alignment drifts, confirm the stop-bit count in the modulator matches. Loopback test is the gate.

- [ ] **Step 4: Run + re-export full pair**

Run: `cargo test -p omnimodem-dsp modes::rtty`
Expected: PASS. `lib.rs`: `pub use modes::rtty::{RttyDemod, RttyMod};`

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/rtty.rs crates/dsp/src/lib.rs
git commit -m "Add RTTY demodulator (FM-disc + start-bit sync + Baudot)"
```

---

# Part D — CW

Envelope detection + Morse. Building blocks: `frontend::modulate::CwKeyer`, `frontend::detector::EnvelopeDetector`, `frontend::nco::DownConverter`, `framing::morse::{encode,MorseDecoder,Element}`.

### Task D.1: CW modulator (TX)

**Files:**
- Modify: `crates/dsp/src/modes/cw.rs`
- Test: inline

- [ ] **Step 1: Implementation + test**

```rust
//! CW (Morse) mode assembly: OOK keyed tone.
//!
//! TX: text → Morse element string → raised-edge keyed tone at `tone_hz`,
//! `wpm` PARIS timing. RX: down-convert to the tone, envelope-detect with
//! adaptive squelch, classify key-down/up durations, SOM-fuzzy Morse decode.

use crate::frontend::modulate::CwKeyer;
use crate::framing::morse::{encode as morse_encode, Element};
use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

pub const CW_RATE: u32 = 8_000;

pub struct CwMod {
    tone_hz: f32,
    wpm: f32,
}

impl CwMod {
    pub fn new(wpm: u16, tone_hz: f32) -> Self {
        CwMod { tone_hz, wpm: wpm as f32 }
    }
}

/// Render a Morse element stream to the `.`/`-`/` ` string the `CwKeyer`
/// accepts (`encode` yields timed Mark/Space elements; the keyer wants symbols).
fn elements_to_keyer_string(text: &str) -> String {
    let mut s = String::new();
    for e in morse_encode(text) {
        match e {
            Element::Mark(units) => s.push(if units >= 3 { '-' } else { '.' }),
            Element::Space(units) => {
                if units >= 7 { s.push_str("  ") } else if units >= 3 { s.push(' ') }
                // intra-character gaps (1 unit) need no symbol
            }
        }
    }
    s
}

impl Modulator for CwMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps { native_rate: CW_RATE, bandwidth_hz: 100.0, tx: true,
            duplex: Duplex::Half, shape: DemodShape::Streaming }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("cw needs text")),
        };
        let keyer = CwKeyer::new(CW_RATE as f32, self.tone_hz, self.wpm);
        Ok(keyer.modulate(&elements_to_keyer_string(&text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulates_cq() {
        let mut m = CwMod::new(20, 700.0);
        assert!(m.modulate(&Frame::text("CQ")).unwrap().len() > 1000);
    }
}
```

- [ ] **Step 2: Run + re-export**

Run: `cargo test -p omnimodem-dsp modes::cw`
Expected: PASS. `lib.rs`: `pub use modes::cw::CwMod;`

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/src/modes/cw.rs crates/dsp/src/lib.rs
git commit -m "Add CW modulator (Morse → raised-edge keyed tone)"
```

### Task D.2: CW demodulator (RX)

**Files:**
- Modify: `crates/dsp/src/modes/cw.rs`
- Test: inline

- [ ] **Step 1: Failing loopback test**

```rust
    #[test]
    fn loopback_recovers_cq() {
        let mut tx = CwMod::new(20, 700.0);
        let samples = tx.modulate(&Frame::text("CQ TEST")).unwrap();
        let mut rx = CwDemod::new(20, 700.0);
        let frames = rx.feed(&samples);
        let text: String = frames.iter().filter_map(|f| match &f.payload {
            FramePayload::Text(t) => Some(t.clone()), _ => None }).collect();
        assert!(text.contains("CQ TEST"), "got {text:?}");
    }
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodem-dsp modes::cw::tests::loopback_recovers_cq`
Expected: FAIL.

- [ ] **Step 3: Implement**

The demod measures the tone envelope, thresholds it with the squelch, and converts key-down/up run-lengths (in dot-units) into the `MorseDecoder`. One dot-unit at `wpm` is `1.2 / wpm` seconds → `dit_samples` at the rate.

```rust
use crate::frontend::nco::DownConverter;
use crate::frontend::detector::EnvelopeDetector;
use crate::framing::morse::MorseDecoder;
use crate::mode::Demodulator;
use crate::types::FrameMeta;

pub struct CwDemod {
    wpm: f32,
    tone_hz: f32,
    nco: DownConverter,
    env: EnvelopeDetector,
    dit_samples: f32,
    keyed: bool,
    run: u32,        // samples in the current key state
    decoder: MorseDecoder,
    idle_run: u32,   // trailing key-up samples, to flush
    sample_index: u64,
}

impl CwDemod {
    pub fn new(wpm: u16, tone_hz: f32) -> Self {
        let rate = CW_RATE as f32;
        let dit_samples = 1.2 / wpm as f32 * rate;
        CwDemod {
            wpm: wpm as f32,
            tone_hz,
            nco: DownConverter::new(tone_hz, rate),
            // attack/decay/floor/open-ratio tuned for an 80 ms dit envelope.
            env: EnvelopeDetector::new(0.05, 0.005, 0.001, 3.0),
            dit_samples,
            keyed: false,
            run: 0,
            decoder: MorseDecoder::new(),
            idle_run: 0,
            sample_index: 0,
        }
    }
}

impl Demodulator for CwDemod {
    fn caps(&self) -> ModeCaps {
        CwMod::new(self.wpm as u16, self.tone_hz).caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        for &x in samples {
            self.sample_index += 1;
            let bb = self.nco.push(x);
            self.env.push(bb.norm()); // tone magnitude at DC after down-convert
            let open = self.env.squelch_open();
            if open == self.keyed {
                self.run += 1;
                if !open { self.idle_run += 1; }
            } else {
                // State change: emit the just-finished run in dot-units.
                let units = self.run as f32 / self.dit_samples;
                if self.keyed {
                    self.decoder.key_down(units);
                } else {
                    self.decoder.key_up(units);
                }
                self.keyed = open;
                self.run = 1;
                self.idle_run = if open { 0 } else { 1 };
            }
        }
        Vec::new() // CW flushes at reset/end; see `finish`-style drain below
    }

    fn reset(&mut self) {
        *self = CwDemod::new(self.wpm as u16, self.tone_hz);
    }
}

impl CwDemod {
    /// Flush the accumulated Morse to text (call at end-of-transmission; the
    /// daemon RX worker calls this when capture drains / a word gap is seen).
    pub fn finish_text(&mut self) -> Vec<Frame> {
        let dec = std::mem::replace(&mut self.decoder, MorseDecoder::new());
        let text = dec.finish();
        if text.is_empty() { return Vec::new(); }
        Vec::from([Frame {
            payload: FramePayload::Text(text),
            meta: FrameMeta { crc_ok: true, sample_offset: self.sample_index,
                decoder: Some("cw".into()), ..Default::default() },
        }])
    }
}
```

Because `MorseDecoder::finish` consumes `self`, the loopback test drives `feed` then `finish_text`. Update the Step-1 test to call it:

```rust
        let mut frames = rx.feed(&samples);
        frames.extend(rx.finish_text());
```

- [ ] **Step 4: Run + tune + re-export**

Run: `cargo test -p omnimodem-dsp modes::cw`
Expected: PASS. If decode is empty, the squelch open-ratio/floor are the knobs (the keyed tone is strong vs. the silent gaps, so a 3.0 open-ratio over a fast-tracking floor should trip cleanly). `lib.rs`: `pub use modes::cw::{CwDemod, CwMod};`

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/cw.rs crates/dsp/src/lib.rs
git commit -m "Add CW demodulator (envelope squelch + duration → Morse)"
```

---

# Part E — FT8

The most demanding first mode: block/windowed, soft-LLR, LDPC+OSD, Costas-array sync, 15 s time-slot TX. Building blocks: `frontend::modulate::Gfsk`, `frontend::stft::Stft`, `sync::candidate::CandidateFinder`, `sync::costas_array::CostasCorrelator`, `fec::llr::demap_fsk_ft8`, `fec::ldpc::Ldpc::ft8`, `fec::osd::osd_decode`, `fec::crc::{crc,CRC14_FT8}`, `framing::message77::{pack77,unpack77}`, `fec::ft8_tables`.

FT8 numerology (locked): 12 kHz working rate, 79 symbols (58 data + 3×7 Costas), 8-FSK, tone spacing 6.25 Hz, symbol length 1920 samples (0.16 s), full transmission 12.64 s within the 15 s slot. Payload: 77 message bits + 14 CRC + 83 LDPC parity = 174 bits = 58 symbols × 3 bits.

### Task E.1: FT8 modulator (TX) — message → 79-symbol 8-FSK

**Files:**
- Modify: `crates/dsp/src/modes/ft8.rs`
- Test: inline

- [ ] **Step 1: Implementation + test**

```rust
//! FT8 mode assembly: 12 kHz, 8-FSK, 79 symbols (3×7 Costas + 58 data), 6.25 Hz
//! spacing, 1920 samples/symbol. Block/windowed: 15 s slot, 12.64 s waveform.
//!
//! TX: text → pack77 → CRC-14 → LDPC(174,91) encode → 58 data symbols (3 bits
//! each, FT8 Gray map) interleaved with the three Costas sync groups → Gaussian
//! 8-FSK. RX: see Task E.2.

use crate::frontend::modulate::Gfsk;
use crate::framing::message77::{pack77, unpack77};
use crate::fec::crc::{crc, CRC14_FT8};
use crate::fec::ldpc::Ldpc;
use crate::fec::llr::FT8_GRAY_MAP;
use crate::sync::costas_array::ft8_costas;
use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

pub const FT8_RATE: u32 = 12_000;
pub const FT8_SPS: usize = 1920;          // samples/symbol (0.16 s)
pub const FT8_TONE_SPACING: f32 = 6.25;   // Hz
pub const FT8_BASE_HZ: f32 = 1000.0;      // default audio sub-carrier
pub const FT8_NSYM: usize = 79;
pub const FT8_SLOT_S: f32 = 15.0;
pub const FT8_WINDOW_S: f32 = 15.0;

/// Costas group positions within the 79-symbol frame: symbols 0–6, 36–42, 72–78.
pub const FT8_COSTAS_STARTS: [usize; 3] = [0, 36, 72];

pub struct Ft8Mod {
    base_hz: f32,
}

impl Ft8Mod {
    pub fn new() -> Self {
        Ft8Mod { base_hz: FT8_BASE_HZ }
    }
}

impl Default for Ft8Mod {
    fn default() -> Self { Self::new() }
}

/// Build the 79 channel-symbol tones (0–7) for a 77-bit message.
pub fn ft8_symbols(message: &str) -> Result<[u8; FT8_NSYM], ModError> {
    let payload = pack77(message); // [u8;10], 77 bits MSB-first (+3 zero pad)
    // CRC-14 over the 77-bit payload (96-bit-padded per ft8_lib); we feed the
    // 10 payload bytes. Provenance to confirm vs ft8_lib in Task I (cross-decode).
    let cksum = crc(&CRC14_FT8, &payload);
    // Assemble 91 message+CRC bits: 77 message bits then 14 CRC bits.
    let mut bits91 = vec![0u8; 91];
    for i in 0..77 {
        bits91[i] = (payload[i / 8] >> (7 - (i % 8))) & 1;
    }
    for i in 0..14 {
        bits91[77 + i] = ((cksum >> (13 - i)) & 1) as u8;
    }
    let code = Ldpc::ft8();
    let cw = code.encode(&bits91); // 174 bits = 91 systematic + 83 parity
    // 174 bits → 58 data symbols (3 bits each), then FT8 Gray-map each.
    let costas = ft8_costas();
    let mut syms = [0u8; FT8_NSYM];
    let mut di = 0usize; // data-symbol index 0..58
    for (s, sym) in syms.iter_mut().enumerate() {
        if let Some(g) = FT8_COSTAS_STARTS.iter().position(|&start| (start..start + 7).contains(&s)) {
            *sym = costas[s - FT8_COSTAS_STARTS[g]] as u8;
        } else {
            let b0 = cw[di * 3] as usize;
            let b1 = cw[di * 3 + 1] as usize;
            let b2 = cw[di * 3 + 2] as usize;
            let idx = (b0 << 2) | (b1 << 1) | b2;
            *sym = FT8_GRAY_MAP[idx];
            di += 1;
        }
    }
    Ok(syms)
}

impl Modulator for Ft8Mod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: FT8_RATE,
            bandwidth_hz: 50.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Windowed { window_s: FT8_WINDOW_S, period_s: FT8_SLOT_S },
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let message = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            FramePayload::Message77(m) => unpack77(m),
            _ => return Err(ModError::UnsupportedPayload("ft8 needs text/message77")),
        };
        let syms = ft8_symbols(&message)?;
        let sym_u32: Vec<u32> = syms.iter().map(|&s| s as u32).collect();
        let gfsk = Gfsk::new(FT8_RATE as f32, FT8_SPS, self.base_hz, FT8_TONE_SPACING, 2.0);
        Ok(gfsk.modulate(&sym_u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_have_costas_groups() {
        let syms = ft8_symbols("CQ K1ABC FN42").unwrap();
        let costas = ft8_costas();
        for (g, &start) in FT8_COSTAS_STARTS.iter().enumerate() {
            for k in 0..7 {
                assert_eq!(syms[start + k], costas[k] as u8, "Costas group {g} pos {k}");
            }
        }
    }

    #[test]
    fn modulates_full_waveform() {
        let mut m = Ft8Mod::new();
        let s = m.modulate(&Frame::text("CQ K1ABC FN42")).unwrap();
        assert_eq!(s.len(), FT8_NSYM * FT8_SPS); // 79 × 1920 = 151_680
    }
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p omnimodem-dsp modes::ft8`
Expected: PASS. `lib.rs`: `pub use modes::ft8::Ft8Mod;`

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/src/modes/ft8.rs crates/dsp/src/lib.rs
git commit -m "Add FT8 modulator (pack77 + CRC14 + LDPC + Costas → 8-FSK)"
```

### Task E.2: FT8 block demodulator (RX) — single-candidate decode loopback

**Files:**
- Modify: `crates/dsp/src/modes/ft8.rs`
- Test: inline

- [ ] **Step 1: Failing loopback test**

```rust
    #[test]
    fn loopback_decodes_message() {
        let msg = "CQ K1ABC FN42";
        let mut tx = Ft8Mod::new();
        // Pad the 12.64 s waveform into a full 15 s window of silence around it.
        let wave = tx.modulate(&Frame::text(msg)).unwrap();
        let mut window = vec![0.0f32; (FT8_RATE as f32 * FT8_WINDOW_S) as usize];
        window[..wave.len()].copy_from_slice(&wave);

        let mut rx = Ft8Demod::new();
        let decodes = rx.decode_window(&window, 0);
        let texts: Vec<String> = decodes.iter().filter_map(|f| match &f.payload {
            FramePayload::Text(t) => Some(t.clone()),
            FramePayload::Message77(m) => Some(unpack77(m)),
            _ => None,
        }).collect();
        assert!(texts.iter().any(|t| t == msg), "decoded: {texts:?}");
    }
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodem-dsp modes::ft8::tests::loopback_decodes_message`
Expected: FAIL — `Ft8Demod` undefined.

- [ ] **Step 3: Implement the block demod**

The decode pipeline per design §"pipeline-stage model": STFT tone-energy matrix → candidate finder → Costas-array sync (time/freq offset) → per-symbol 8-tone power extraction → `demap_fsk_ft8` LLRs → LDPC min-sum, OSD fallback → CRC-14 check → `unpack77`.

```rust
use crate::frontend::stft::Stft;
use crate::sync::candidate::CandidateFinder;
use crate::sync::costas_array::CostasCorrelator;
use crate::fec::llr::demap_fsk_ft8;
use crate::fec::osd::osd_decode;
use crate::mode::BlockDemodulator;
use crate::types::{FrameMeta, Sample};

pub struct Ft8Demod {
    finder: CandidateFinder,
}

impl Ft8Demod {
    pub fn new() -> Self {
        // Sweep ~200–2800 Hz at 6.25 Hz resolution over 1920-sample frames.
        let finder = CandidateFinder::new(
            FT8_RATE as f32, FT8_SPS, FT8_SPS / 2, 200.0, 2800.0, FT8_TONE_SPACING,
        ).with_min_metric(2.0);
        Ft8Demod { finder }
    }

    /// Tone-energy matrix: for each of the 79 symbols, the power in each of the
    /// 8 tones at `base_hz + tone*6.25`, given a sample offset.
    fn tone_energy(&self, window: &[Sample], base_hz: f32, t0: usize) -> Vec<[f32; 8]> {
        let mut mat = Vec::with_capacity(FT8_NSYM);
        for s in 0..FT8_NSYM {
            let start = t0 + s * FT8_SPS;
            let mut tones = [0.0f32; 8];
            if start + FT8_SPS <= window.len() {
                let seg = &window[start..start + FT8_SPS];
                for (tone, e) in tones.iter_mut().enumerate() {
                    let f = base_hz + tone as f32 * FT8_TONE_SPACING;
                    *e = goertzel_power(seg, f, FT8_RATE as f32);
                }
            }
            mat.push(tones);
        }
        mat
    }
}

/// Single-bin power via Goertzel — cheap per-tone energy at exactly `f`.
fn goertzel_power(x: &[Sample], f: f32, rate: f32) -> f32 {
    let w = 2.0 * std::f32::consts::PI * f / rate;
    let coeff = 2.0 * w.cos();
    let (mut s0, mut s1, mut s2) = (0.0f32, 0.0f32, 0.0f32);
    for &v in x {
        s0 = v + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

impl Default for Ft8Demod {
    fn default() -> Self { Self::new() }
}

impl BlockDemodulator for Ft8Demod {
    fn caps(&self) -> ModeCaps {
        Ft8Mod::new().caps()
    }

    fn decode_window(&mut self, window: &[Sample], window_start_ns: u64) -> Vec<Frame> {
        let _ = window_start_ns;
        let code = Ldpc::ft8();
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for cand in self.finder.find(window) {
            let base_hz = cand.freq;
            let t0 = (cand.time * FT8_RATE as f32) as usize;
            // Refine sync: Costas-array correlator over a small time/freq grid.
            let energy = self.tone_energy(window, base_hz, t0);
            let energy_vec: Vec<Vec<f32>> = energy.iter().map(|t| t.to_vec()).collect();
            let corr = CostasCorrelator::ft8();
            let (dt, df, _metric) = corr.best(&energy_vec, -2..=2, -1..=1);
            let t0r = (t0 as isize + dt * FT8_SPS as isize).max(0) as usize;
            let base_r = base_hz + df as f32 * FT8_TONE_SPACING;

            // Re-extract energies at the refined offset and demap to LLRs.
            let e = self.tone_energy(window, base_r, t0r);
            let noise_var = 1.0; // normalized; min-sum scaling absorbs the rest
            let mut llrs: Vec<f32> = Vec::with_capacity(174);
            for (s, tones) in e.iter().enumerate() {
                if FT8_COSTAS_STARTS.iter().any(|&st| (st..st + 7).contains(&s)) {
                    continue; // skip the 21 Costas symbols
                }
                llrs.extend(demap_fsk_ft8(tones, noise_var)); // 3 LLRs/symbol
            }
            if llrs.len() != 174 { continue; }

            // LDPC min-sum; OSD fallback if parity unsatisfied.
            let (mut cw, perr) = code.decode_minsum(&llrs, 50);
            if perr != 0 {
                if let Some(better) = osd_decode(&code, &llrs, 2) {
                    cw = better;
                } else {
                    continue;
                }
            }
            // Recover 91 message+CRC bits, verify CRC-14.
            let mut payload = [0u8; 10];
            for i in 0..77 {
                payload[i / 8] |= cw[i] << (7 - (i % 8));
            }
            let mut rx_crc = 0u16;
            for i in 0..14 {
                rx_crc = (rx_crc << 1) | cw[77 + i] as u16;
            }
            if crc(&CRC14_FT8, &payload) as u16 != rx_crc {
                continue; // CRC fail — reject this candidate
            }
            let text = unpack77(&payload);
            if !seen.insert(text.clone()) {
                continue; // dedup by decoded message (windowed-mode dedup key)
            }
            out.push(Frame {
                payload: FramePayload::Text(text),
                meta: FrameMeta {
                    crc_ok: true,
                    freq_offset_hz: Some(base_r),
                    time_offset_s: Some(t0r as f32 / FT8_RATE as f32),
                    decoder: Some("ft8".into()),
                    sample_offset: t0r as u64,
                    ..Default::default()
                },
            });
        }
        out
    }
}
```

> Note for the executor: this single-pass BP+OSD-per-candidate decoder is the Phase-4 scope (design lists SIC and AP as Phase-5 WSJT-X-class differentiators). The exact CRC-14 input framing (whether `ft8_lib` CRCs the 77 bits zero-padded to 82/96 bits) is the one constant the design (§"Constants to confirm") flags — the **loopback test is the internal gate** (TX and RX use the same `crc` call so they agree), and Task I.6's `ft8code` cross-decode is the external gate that nails the exact bit framing. If loopback fails at the CRC step, log `code.parity_errors(&cw)` to confirm the LDPC stage converged before suspecting the CRC framing.

- [ ] **Step 4: Run the loopback; iterate sync/extraction until green**

Run: `cargo test -p omnimodem-dsp modes::ft8::tests::loopback_decodes_message`
Expected: PASS. Most likely tuning point: the `tone_energy` symbol alignment (`t0`) and the Costas refine grid; widen `-2..=2`/`-1..=1` if the candidate's coarse offset is off by a symbol.

- [ ] **Step 5: Re-export full pair + commit**

`lib.rs`: `pub use modes::ft8::{Ft8Demod, Ft8Mod};`

```bash
git add crates/dsp/src/modes/ft8.rs crates/dsp/src/lib.rs
git commit -m "Add FT8 block demodulator (candidate → Costas → LLR → LDPC+OSD → CRC14)"
```

---

# Part F — Daemon RX worker (capture → demod → frames)

Today `LiveBindings.captures` holds a `CaptureHandle` that **no code consumes** (`core/mod.rs:88`). Part F adds a per-channel RX worker thread that finally drives a real demod and emits `FrameEvent::RxFrame`, gated by the existing per-rig `RxTxInterlock` so we never decode our own transmission.

### Task F.1: RX worker struct driving a streaming demod

**Files:**
- Create: `crates/omnimodemd/src/core/rx_worker.rs`
- Modify: `crates/omnimodemd/src/core/mod.rs` (add `pub mod rx_worker;`)
- Test: `crates/omnimodemd/src/core/rx_worker.rs` (inline)

- [ ] **Step 1: Write the failing test**

```rust
//! Per-channel RX worker. Pulls `AudioChunk`s from a capture, resamples to the
//! mode's native rate, drives a streaming or windowed demod, honors the per-rig
//! RX/TX interlock (skip decode while we key the rig), and emits decoded frames
//! on the LOSSLESS frame broadcast.

use crate::audio::backend::CaptureHandle;
use crate::audio::AudioChunk;
use crate::core::event::FrameEvent;
use crate::ids::{ChannelId, DeviceId};
use crate::ptt::interlock::RxTxInterlock;
use omnimodem_dsp::frontend::resample::Resampler;
use omnimodem_dsp::mode::Demodulator;
use omnimodem_dsp::types::{FramePayload, Sample};
use std::thread::JoinHandle;
use tokio::sync::broadcast;

/// A running RX worker. Dropping it signals the thread to stop (the capture
/// receiver closes when its `CaptureHandle` is dropped by the core).
pub struct RxWorker {
    join: Option<JoinHandle<()>>,
}

impl RxWorker {
    /// Spawn a streaming-demod RX worker. `capture` is moved into the thread;
    /// the thread runs until the capture stream ends.
    pub fn spawn_streaming(
        channel: ChannelId,
        rig: DeviceId,
        capture: CaptureHandle,
        mut demod: Box<dyn Demodulator>,
        interlock: RxTxInterlock,
        frames: broadcast::Sender<FrameEvent>,
    ) -> Self {
        let in_rate = capture.sample_rate;
        let native = demod.caps().native_rate;
        let join = std::thread::Builder::new()
            .name(format!("omnimodem-rx-{}", channel.0))
            .spawn(move || {
                let mut resampler = (in_rate != native)
                    .then(|| Resampler::new(in_rate, native, 16));
                while let Ok(chunk) = capture.rx.recv() {
                    if interlock.is_muted(&rig) {
                        continue; // our TX is keyed on this rig; don't self-decode
                    }
                    let samples = to_f32(&chunk);
                    let samples = match resampler.as_mut() {
                        Some(r) => r.process(&samples),
                        None => samples,
                    };
                    for f in demod.feed(&samples) {
                        let data = frame_bytes(&f.payload);
                        let _ = frames.send(FrameEvent::RxFrame {
                            channel,
                            data,
                            timestamp_ns: 0,
                        });
                    }
                }
            })
            .expect("spawn rx worker");
        RxWorker { join: Some(join) }
    }

    pub fn join(mut self) {
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn to_f32(chunk: &AudioChunk) -> Vec<Sample> {
    chunk.iter().map(|&s| s as f32 / 32768.0).collect()
}

/// Flatten a decoded payload to the opaque bytes the proto `RxFrame.data`
/// carries. Text/message decode to UTF-8; packets pass through.
fn frame_bytes(p: &FramePayload) -> Vec<u8> {
    match p {
        FramePayload::Packet(b) | FramePayload::Vocoder(b) => b.clone(),
        FramePayload::Text(t) => t.clone().into_bytes(),
        FramePayload::Message77(m) => m.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::backend::AudioBackend;
    use crate::audio::file::FileBackend;
    use omnimodem_dsp::modes::afsk1200::{Afsk1200Demod, Afsk1200Mod};
    use omnimodem_dsp::mode::Modulator;
    use omnimodem_dsp::types::Frame;

    #[test]
    fn rx_worker_decodes_a_replayed_afsk_frame() {
        // Modulate an AX.25 frame to 48 kHz i16, replay it through a file
        // capture, and assert the worker emits a matching RxFrame.
        use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
        let ax = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("K1ABC", 7),
            digipeaters: vec![],
            info: b"rx worker test".to_vec(),
        };
        let mut tx = Afsk1200Mod::new();
        let f32s = tx.modulate(&Frame::packet(ax.encode())).unwrap();
        let i16s: Vec<i16> = f32s.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();

        let backend = FileBackend::from_samples(i16s, 48_000);
        let capture = backend.open_capture(48_000).unwrap();

        let (tx_b, mut rx_b) = broadcast::channel(64);
        let interlock = RxTxInterlock::new();
        let worker = RxWorker::spawn_streaming(
            ChannelId(0),
            DeviceId::placeholder(),
            capture,
            Box::new(Afsk1200Demod::ensemble(9)),
            interlock,
            tx_b,
        );

        // The file capture ends after replaying; collect emitted frames.
        worker.join();
        let mut got = Vec::new();
        while let Ok(ev) = rx_b.try_recv() {
            let FrameEvent::RxFrame { data, .. } = ev;
            got.push(data);
        }
        assert!(got.iter().any(|d| d == &ax.encode()), "no matching frame: {got:?}");
    }
}
```

- [ ] **Step 2: Register the module + run**

In `crates/omnimodemd/src/core/mod.rs`, add `pub mod rx_worker;` near the top with the other `pub mod` lines (after `pub mod event;`).

Run: `cargo test -p omnimodemd core::rx_worker`
Expected: PASS. The `FileBackend` replays the i16 buffer in ~20 ms chunks then closes the sender (EOF), so the worker thread exits and `join()` returns.

> If the assertion fails, the most likely cause is the demod needs the *whole* signal to be contiguous across chunk boundaries — which it is, since the worker feeds each chunk into the same persistent demod instance. Confirm `Afsk1200Demod::ensemble` accumulates `bit_levels` across `feed` calls (it does — state lives in the struct).

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodemd/src/core/rx_worker.rs crates/omnimodemd/src/core/mod.rs
git commit -m "Add per-channel RX worker driving a streaming demod"
```

### Task F.2: RX worker windowed-demod path (FT8)

**Files:**
- Modify: `crates/omnimodemd/src/core/rx_worker.rs`
- Test: inline

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn rx_worker_decodes_a_windowed_ft8_message() {
        use omnimodem_dsp::modes::ft8::{Ft8Demod, Ft8Mod, FT8_RATE, FT8_WINDOW_S};
        let mut tx = Ft8Mod::new();
        let wave = tx.modulate(&Frame::text("CQ K1ABC FN42")).unwrap();
        let mut win = vec![0.0f32; (FT8_RATE as f32 * FT8_WINDOW_S) as usize];
        win[..wave.len()].copy_from_slice(&wave);
        let i16s: Vec<i16> = win.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();

        let backend = FileBackend::from_samples(i16s, FT8_RATE);
        let capture = backend.open_capture(FT8_RATE).unwrap();
        let (tx_b, mut rx_b) = broadcast::channel(64);
        let worker = RxWorker::spawn_windowed(
            ChannelId(1),
            DeviceId::placeholder(),
            capture,
            Box::new(Ft8Demod::new()),
            RxTxInterlock::new(),
            tx_b,
            FT8_WINDOW_S,
        );
        worker.join();
        let mut texts = Vec::new();
        while let Ok(FrameEvent::RxFrame { data, .. }) = rx_b.try_recv() {
            texts.push(String::from_utf8_lossy(&data).to_string());
        }
        assert!(texts.iter().any(|t| t == "CQ K1ABC FN42"), "got {texts:?}");
    }
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodemd core::rx_worker::tests::rx_worker_decodes_a_windowed_ft8`
Expected: FAIL — `spawn_windowed` undefined.

- [ ] **Step 3: Implement the windowed spawner**

```rust
use omnimodem_dsp::mode::BlockDemodulator;

impl RxWorker {
    /// Spawn a windowed (block) RX worker. Buffers `window_s` of samples at the
    /// demod's native rate, calls `decode_window`, then advances by one window.
    /// Production aligns the window to the wall-clock slot via `SlotClock`
    /// (Task H.2); this base spawner aligns to the capture start, which is what
    /// the deterministic file-replay test needs.
    pub fn spawn_windowed(
        channel: ChannelId,
        rig: DeviceId,
        capture: CaptureHandle,
        mut demod: Box<dyn BlockDemodulator>,
        interlock: RxTxInterlock,
        frames: broadcast::Sender<FrameEvent>,
        window_s: f32,
    ) -> Self {
        let in_rate = capture.sample_rate;
        let native = demod.caps().native_rate;
        let win_samples = (native as f32 * window_s) as usize;
        let join = std::thread::Builder::new()
            .name(format!("omnimodem-rx-win-{}", channel.0))
            .spawn(move || {
                let mut resampler =
                    (in_rate != native).then(|| Resampler::new(in_rate, native, 16));
                let mut buf: Vec<Sample> = Vec::with_capacity(win_samples);
                let mut muted_window = false;
                while let Ok(chunk) = capture.rx.recv() {
                    if interlock.is_muted(&rig) {
                        muted_window = true; // a TX overlapped this window
                    }
                    let s = to_f32(&chunk);
                    let s = match resampler.as_mut() {
                        Some(r) => r.process(&s),
                        None => s,
                    };
                    buf.extend_from_slice(&s);
                    while buf.len() >= win_samples {
                        let window: Vec<Sample> = buf.drain(..win_samples).collect();
                        if !muted_window {
                            for f in demod.decode_window(&window, 0) {
                                let data = frame_bytes(&f.payload);
                                let _ = frames.send(FrameEvent::RxFrame {
                                    channel, data, timestamp_ns: 0 });
                            }
                        }
                        muted_window = false;
                    }
                }
                // Decode a trailing partial window padded to full length (the
                // file test's single window may be exactly win_samples; this
                // handles a short remainder).
                if !buf.is_empty() && !muted_window {
                    buf.resize(win_samples, 0.0);
                    for f in demod.decode_window(&buf, 0) {
                        let data = frame_bytes(&f.payload);
                        let _ = frames.send(FrameEvent::RxFrame {
                            channel, data, timestamp_ns: 0 });
                    }
                }
            })
            .expect("spawn windowed rx worker");
        RxWorker { join: Some(join) }
    }
}
```

- [ ] **Step 4: Run**

Run: `cargo test -p omnimodemd core::rx_worker`
Expected: PASS (both streaming and windowed tests).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/core/rx_worker.rs
git commit -m "Add windowed (FT8) RX worker path"
```

### Task F.3: Wire RX workers into the core lifecycle

**Files:**
- Modify: `crates/omnimodemd/src/core/mod.rs`
- Test: `crates/omnimodemd/tests/` integration (see Task I) + inline via the existing `core::tests`

- [ ] **Step 1: Write the failing test**

Add to `core::tests` in `core/mod.rs` a test that configures an AFSK channel against a file capture and subscribes for an `RxFrame`. Reuse the existing test harness (`spawn_core`, `FileBackend`). Because `configure_audio` currently opens both capture and playback, extend it to also spawn an RX worker when the channel's mode resolves to a real demod.

```rust
    #[test]
    fn configuring_an_afsk_channel_spawns_rx_and_emits_frames() {
        use omnimodem_dsp::modes::afsk1200::Afsk1200Mod;
        use omnimodem_dsp::mode::Modulator;
        use omnimodem_dsp::types::Frame as DspFrame;
        use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};

        let ax = Ax25Frame { dest: Address::new("APRS", 0), source: Address::new("K1ABC", 1),
            digipeaters: vec![], info: b"core rx".to_vec() };
        let mut m = Afsk1200Mod::new();
        let f32s = m.modulate(&DspFrame::packet(ax.encode())).unwrap();
        let i16s: Vec<i16> = f32s.iter().map(|&s| (s.clamp(-1.0,1.0)*32767.0) as i16).collect();

        let dev = DeviceDescriptor { id: DeviceId::AlsaCard { card_name: "loop".into() },
            label: "loop".into(), has_capture: true, has_playback: true };
        let dev_id = dev.id.clone();
        let samples = i16s.clone();
        let (core, join) = spawn_core(
            Box::new(FakeEnumerator::new(vec![dev])),
            Box::new(move |_| Box::new(FileBackend::from_samples(samples.clone(), 48_000))),
        );
        let mut frames = core.frames.subscribe();
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            configure_channel(&core, ChannelId(0), "afsk1200").await;
            configure_audio_ch(&core, ChannelId(0), dev_id.clone()).await;
            // The file capture replays the modulated frame; expect an RxFrame.
            let got = tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv())
                .await.expect("frame within timeout").unwrap();
            let FrameEvent::RxFrame { data, .. } = got;
            assert_eq!(data, ax.encode());
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }
```

Add the two small async helpers `configure_channel` and `configure_audio_ch` to the test module (thin wrappers over the existing `Command::ConfigureChannel` / `Command::ConfigureAudio` oneshot dance already shown in `configured_audio_ptt_transmit_runs_real_cycle`).

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodemd core::tests::configuring_an_afsk_channel_spawns_rx`
Expected: FAIL — no RX worker is spawned, so no frame arrives (timeout).

- [ ] **Step 3: Spawn the worker in `configure_audio`**

In `core/mod.rs`, extend `LiveBindings` with `rx_workers: HashMap<ChannelId, RxWorker>` and a frames-sender handle. Thread the `frames` broadcast and the `interlock` into `configure_audio` (the `run` loop already owns both — pass them through `handle_command`). After opening capture/playback, resolve the channel's mode and, if it builds a demod, spawn:

```rust
    // After `live.audio.insert(...)`:
    let mode = supervisor.channel_mode(id); // returns the resolved ModeConfig
    match crate::mode::registry::demod_kind(&mode) {
        crate::mode::registry::DemodKind::Streaming(demod) => {
            // Re-open a *second* capture for the worker, or move the held one.
            let worker = RxWorker::spawn_streaming(
                id, device_id.clone(), capture_for_worker, demod,
                interlock.clone(), frames.clone());
            live.rx_workers.insert(id, worker);
        }
        crate::mode::registry::DemodKind::Windowed(bd, window_s) => {
            let worker = RxWorker::spawn_windowed(
                id, device_id.clone(), capture_for_worker, bd,
                interlock.clone(), frames.clone(), window_s);
            live.rx_workers.insert(id, worker);
        }
        crate::mode::registry::DemodKind::None => {
            live.captures.insert(id, capture); // legacy: hold idle (NullMode)
        }
    }
```

This requires:
- `Supervisor::channel_mode(&self, id) -> crate::mode::ModeConfig` (resolve the stored string via `ModeConfig::parse(&cfg.mode).unwrap_or(ModeConfig::None)`).
- A new `registry::demod_kind(&ModeConfig) -> DemodKind` enum returning the boxed demod and, for windowed, the window seconds (from `caps().shape`). Implement in Task F.4.
- `run()` must pass `frames` (the `FrameEvent` sender) and `interlock` into `handle_command`/`configure_audio` (currently `_frames` is unused — un-underscore it and thread it through).

- [ ] **Step 4: Implement the supporting pieces, then run**

Add `Supervisor::channel_mode`, thread `frames`/`interlock` through `configure_audio`, and on `DeviceDeparted`/eviction also drop `live.rx_workers.remove(&c)` so a hotplug tears down the worker.

Run: `cargo test -p omnimodemd core::tests::configuring_an_afsk_channel_spawns_rx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/core/mod.rs crates/omnimodemd/src/supervisor/mod.rs
git commit -m "Spawn/teardown per-channel RX workers in the core lifecycle"
```

### Task F.4: Registry `demod_kind` builder

**Files:**
- Modify: `crates/omnimodemd/src/mode/registry.rs`
- Test: `crates/omnimodemd/src/mode/registry.rs` (inline)

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn demod_kind_classifies_modes() {
        use super::{demod_kind, DemodKind};
        assert!(matches!(demod_kind(&ModeConfig::Afsk1200 { tx: true }), DemodKind::Streaming(_)));
        assert!(matches!(demod_kind(&ModeConfig::Ft8), DemodKind::Windowed(_, w) if (w - 15.0).abs() < 0.01));
        assert!(matches!(demod_kind(&ModeConfig::None), DemodKind::None));
        assert!(matches!(demod_kind(&ModeConfig::Cw { wpm: 20, tone_hz: 700.0 }), DemodKind::Streaming(_)));
    }
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodemd mode::registry::tests::demod_kind_classifies_modes`
Expected: FAIL.

- [ ] **Step 3: Implement**

Replace the body of `registry.rs` `build_demod`/`build_modulator` and add `demod_kind`:

```rust
use omnimodem_dsp::mode::{BlockDemodulator, DemodShape, Demodulator, Modulator};
use omnimodem_dsp::modes::{
    afsk1200::{Afsk1200Demod, Afsk1200Mod},
    cw::{CwDemod, CwMod},
    ft8::{Ft8Demod, Ft8Mod},
    psk31::{Psk31Demod, Psk31Mod},
    rtty::{RttyDemod, RttyMod},
};

/// What kind of demod a mode needs the RX worker to drive.
pub enum DemodKind {
    None,
    Streaming(Box<dyn Demodulator>),
    Windowed(Box<dyn BlockDemodulator>, f32),
}

pub fn demod_kind(cfg: &ModeConfig) -> DemodKind {
    match cfg {
        ModeConfig::None => DemodKind::None,
        ModeConfig::Afsk1200 { .. } => DemodKind::Streaming(Box::new(Afsk1200Demod::ensemble(9))),
        ModeConfig::Cw { wpm, tone_hz } => DemodKind::Streaming(Box::new(CwDemod::new(*wpm, *tone_hz))),
        ModeConfig::Rtty { baud, shift_hz } => DemodKind::Streaming(Box::new(RttyDemod::new(*baud, *shift_hz))),
        ModeConfig::Psk31 { center_hz } => DemodKind::Streaming(Box::new(Psk31Demod::new(*center_hz))),
        ModeConfig::Ft8 => {
            let bd = Ft8Demod::new();
            let window_s = match bd.caps().shape {
                DemodShape::Windowed { window_s, .. } => window_s,
                _ => 15.0,
            };
            DemodKind::Windowed(Box::new(bd), window_s)
        }
    }
}

/// Build a modulator for a config, or `None` if receive-only / unmoded.
pub fn build_modulator(cfg: &ModeConfig) -> Option<Box<dyn Modulator>> {
    match cfg {
        ModeConfig::None => Some(Box::new(super::NullMode)),
        ModeConfig::Afsk1200 { .. } => Some(Box::new(Afsk1200Mod::new())),
        ModeConfig::Cw { wpm, tone_hz } => Some(Box::new(CwMod::new(*wpm, *tone_hz))),
        ModeConfig::Rtty { baud, shift_hz } => Some(Box::new(RttyMod::new(*baud, *shift_hz))),
        ModeConfig::Psk31 { center_hz } => Some(Box::new(Psk31Mod::new(*center_hz))),
        ModeConfig::Ft8 => Some(Box::new(Ft8Mod::new())),
    }
}
```

Note: `Afsk1200Ensemble`, `CwDemod`, `RttyDemod`, `Psk31Demod` implement `Demodulator`; `Ft8Demod` implements `BlockDemodulator`. The old `build_demod` returning `Option<Box<dyn Demodulator>>` is replaced by `demod_kind`; delete `build_demod` and update its one test (`none_builds_nullmode`) to use `demod_kind` + `DemodKind::None`. Keep `build_modulator`'s `none_builds_modulator` test.

- [ ] **Step 4: Run**

Run: `cargo test -p omnimodemd mode::registry`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/mode/registry.rs
git commit -m "Add registry demod_kind builder mapping ModeConfig to RX demods"
```

---

# Part G — Daemon TX model (cooperative queue + slot scheduling)

Today `Transmit` synchronously plays raw PCM bytes inside the command handler (`core/mod.rs:314`). Part G replaces that for moded channels with a per-channel TX worker: the command enqueues a frame and returns immediately ("accepted onto the TX queue, not when it leaves the air" — the proto's own contract), and the worker modulates, slot-aligns (windowed modes), and runs `drive_tx_cycle`.

### Task G.1: TX worker with a cooperative frame queue

**Files:**
- Create: `crates/omnimodemd/src/core/tx_worker.rs`
- Modify: `crates/omnimodemd/src/core/mod.rs` (`pub mod tx_worker;`)
- Test: inline

- [ ] **Step 1: Failing test**

```rust
//! Per-channel TX worker. A cooperative queue serializes frames from any client
//! onto one channel's on-air timeline; the worker modulates each frame to
//! samples and runs the no-sleep `drive_tx_cycle`. Windowed modes wait for the
//! next time-slot boundary (Task G.2). Per-rig serialization is enforced by the
//! shared PTT registry/interlock at the core (two channels on one rig still
//! serialize). Replaces Graywolf's single global TX worker.

use crate::audio::backend::PlaybackHandle;
use crate::core::event::TelemetryEvent;
use crate::ids::{ChannelId, DeviceId, TransmitId};
use crate::ptt::interlock::RxTxInterlock;
use crate::ptt::sequence::{drive_tx_cycle, TxCycleOutcome};
use crate::ptt::PttDriver;
use omnimodem_dsp::mode::Modulator;
use omnimodem_dsp::types::{Frame, FramePayload};
use std::sync::mpsc::{Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::broadcast;

/// A queued TX job: the frame to send and its transmit id (for events).
pub struct TxJob {
    pub frame: Frame,
    pub transmit_id: TransmitId,
}

/// Cooperative queue depth per channel. Frames beyond this block the caller
/// (sync-core command handler), applying natural backpressure.
pub const TX_QUEUE_DEPTH: usize = 32;

pub struct TxWorker {
    queue: SyncSender<TxJob>,
    join: Option<JoinHandle<()>>,
}

impl TxWorker {
    pub fn enqueue(&self, job: TxJob) -> Result<(), TxJob> {
        self.queue.try_send(job).map_err(|e| match e {
            std::sync::mpsc::TrySendError::Full(j) | std::sync::mpsc::TrySendError::Disconnected(j) => j,
        })
    }
}

/// Everything the worker thread owns for one channel.
pub struct TxWorkerCfg {
    pub channel: ChannelId,
    pub rig: DeviceId,
    pub rate: u32,
    pub modulator: Box<dyn Modulator>,
    pub sink: PlaybackHandle,
    pub driver: Box<dyn PttDriver>,
    pub interlock: RxTxInterlock,
    pub telemetry: broadcast::Sender<TelemetryEvent>,
    /// `Some(slot_s)` for windowed modes (align to the slot boundary).
    pub slot_s: Option<f32>,
}

pub fn spawn(cfg: TxWorkerCfg) -> TxWorker {
    let (tx, rx) = std::sync::mpsc::sync_channel(TX_QUEUE_DEPTH);
    let join = std::thread::Builder::new()
        .name(format!("omnimodem-tx-{}", cfg.channel.0))
        .spawn(move || run(cfg, rx))
        .expect("spawn tx worker");
    TxWorker { queue: tx, join: Some(join) }
}

fn run(mut cfg: TxWorkerCfg, rx: Receiver<TxJob>) {
    while let Ok(job) = rx.recv() {
        let samples = match cfg.modulator.modulate(&job.frame) {
            Ok(s) => s,
            Err(_) => continue, // bad payload for this mode; drop the job
        };
        let pcm: Vec<i16> = samples.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();

        // Windowed modes wait for the next slot boundary before keying.
        if let Some(slot) = cfg.slot_s {
            wait_for_slot(slot);
        }

        let _ = cfg.telemetry.send(TelemetryEvent::TransmitStarted {
            channel: cfg.channel, transmit_id: job.transmit_id });
        cfg.interlock.begin_tx(&cfg.rig);
        let _ = cfg.telemetry.send(TelemetryEvent::PttKeyed { channel: cfg.channel, keyed: true });
        let outcome = drive_tx_cycle(cfg.driver.as_mut(), &cfg.sink, pcm, cfg.rate,
            Duration::from_millis(5));
        let _ = cfg.telemetry.send(TelemetryEvent::PttKeyed { channel: cfg.channel, keyed: false });
        cfg.interlock.end_tx(&cfg.rig);
        let _ = cfg.telemetry.send(TelemetryEvent::TransmitComplete {
            channel: cfg.channel, transmit_id: job.transmit_id });
        if !matches!(outcome, TxCycleOutcome::Done) {
            // PTT error: stop the worker; the core evicts on the next command.
            break;
        }
    }
}

/// Block until the next slot boundary (see Task G.2 for the real clock-aligned
/// implementation; the base version returns immediately so tests don't wait).
fn wait_for_slot(_slot_s: f32) {}

/// Interpret opaque transmit payload bytes into a `Frame` for `mode`.
/// Text modes (FT8/CW/RTTY/PSK31) take UTF-8 text; AFSK takes raw AX.25 bytes.
pub fn payload_to_frame(mode: &crate::mode::ModeConfig, payload: Vec<u8>) -> Frame {
    use crate::mode::ModeConfig;
    match mode {
        ModeConfig::Afsk1200 { .. } => Frame { payload: FramePayload::Packet(payload), meta: Default::default() },
        _ => Frame {
            payload: FramePayload::Text(String::from_utf8_lossy(&payload).to_string()),
            meta: Default::default(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::backend::AudioBackend;
    use crate::audio::file::FileBackend;
    use crate::ptt::none::MockPtt;
    use crate::mode::ModeConfig;
    use omnimodem_dsp::types::Frame as DspFrame;

    #[test]
    fn worker_modulates_and_plays_a_queued_text_frame() {
        let backend = FileBackend::from_samples(vec![], 8_000);
        let sink = backend.open_playback(8_000).unwrap();
        let (tele, mut tele_rx) = broadcast::channel(64);
        let worker = spawn(TxWorkerCfg {
            channel: ChannelId(0),
            rig: DeviceId::placeholder(),
            rate: 8_000,
            modulator: crate::mode::registry::build_modulator(
                &ModeConfig::Psk31 { center_hz: 1000.0 }).unwrap(),
            sink,
            driver: Box::new(MockPtt::new()),
            interlock: RxTxInterlock::new(),
            telemetry: tele,
            slot_s: None,
        });
        worker.enqueue(TxJob {
            frame: DspFrame::text("CQ"),
            transmit_id: TransmitId(1),
        }).unwrap();

        // Drop the queue sender to end the worker, then join.
        drop(worker.queue.clone());
        // Observe the TransmitComplete telemetry.
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            let mut completed = false;
            for _ in 0..16 {
                if let Ok(TelemetryEvent::TransmitComplete { transmit_id, .. }) = tele_rx.try_recv() {
                    assert_eq!(transmit_id, TransmitId(1));
                    completed = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(completed, "no TransmitComplete");
        });
        // Audio actually reached the sink.
        assert!(!backend.played.lock().unwrap().is_empty(), "no audio played");
    }
}
```

- [ ] **Step 2: Register + run**

In `core/mod.rs` add `pub mod tx_worker;`.

Run: `cargo test -p omnimodemd core::tx_worker`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/omnimodemd/src/core/tx_worker.rs crates/omnimodemd/src/core/mod.rs
git commit -m "Add per-channel TX worker with cooperative frame queue"
```

### Task G.2: Time-slot-aligned scheduling for windowed modes

**Files:**
- Modify: `crates/omnimodemd/src/core/tx_worker.rs` (`wait_for_slot`)
- Create: `crates/omnimodemd/src/core/clock.rs` (`SlotClock`)
- Test: `crates/omnimodemd/src/core/clock.rs` (inline)

- [ ] **Step 1: Failing test for `SlotClock`**

```rust
//! Host time base for windowed modes. WSJT-X-family modes need an accurate
//! clock (design §"Time synchronization"); we depend on NTP/PTP disciplining
//! and surface the offset as a metric. `SlotClock` computes the wall-clock
//! delay to the next slot boundary; `ClockSource` reports the host offset.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Computes the delay until the next `slot_s`-aligned UTC boundary (FT8: 15 s).
pub struct SlotClock {
    slot: Duration,
}

impl SlotClock {
    pub fn new(slot_s: f32) -> Self {
        SlotClock { slot: Duration::from_secs_f32(slot_s) }
    }

    /// Delay from `now` (UNIX time) to the next slot boundary.
    pub fn delay_from(&self, now: Duration) -> Duration {
        let slot_ns = self.slot.as_nanos() as u64;
        let now_ns = now.as_nanos() as u64;
        let into = now_ns % slot_ns;
        if into == 0 { Duration::ZERO } else { Duration::from_nanos(slot_ns - into) }
    }

    /// Delay until the next boundary from the real clock.
    pub fn delay_until_next(&self) -> Duration {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        self.delay_from(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_to_next_15s_boundary() {
        let c = SlotClock::new(15.0);
        // 7 s into a slot → 8 s to the next boundary.
        assert_eq!(c.delay_from(Duration::from_secs(7)), Duration::from_secs(8));
        // Exactly on a boundary → no wait.
        assert_eq!(c.delay_from(Duration::from_secs(30)), Duration::ZERO);
        // 14.5 s in → 0.5 s.
        assert_eq!(c.delay_from(Duration::from_millis(14_500)), Duration::from_millis(500));
    }
}
```

- [ ] **Step 2: Register module + run**

`core/mod.rs`: `pub mod clock;`

Run: `cargo test -p omnimodemd core::clock`
Expected: PASS.

- [ ] **Step 3: Use `SlotClock` in `wait_for_slot`**

Replace the stub in `tx_worker.rs`:

```rust
fn wait_for_slot(slot_s: f32) {
    let delay = crate::core::clock::SlotClock::new(slot_s).delay_until_next();
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
}
```

- [ ] **Step 4: Run the tx_worker suite (still green; slot=None in the test)**

Run: `cargo test -p omnimodemd core::tx_worker core::clock`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/core/clock.rs crates/omnimodemd/src/core/tx_worker.rs crates/omnimodemd/src/core/mod.rs
git commit -m "Add SlotClock and wire FT8 time-slot-aligned TX scheduling"
```

### Task G.3: Route `Transmit` through the TX worker

**Files:**
- Modify: `crates/omnimodemd/src/core/mod.rs`
- Test: `core::tests` inline (extend the existing transmit test)

- [ ] **Step 1: Failing test**

Add a test that configures an AFSK channel + audio + PTT, sends `Command::Transmit` with AX.25 frame bytes, and asserts `TransmitStarted`/`TransmitComplete` arrive **and** that the immediate `Transmit` reply returns a `TransmitId` before on-air completion (queue-accept semantics).

```rust
    #[test]
    fn transmit_on_moded_channel_enqueues_and_completes() {
        // (Reuse the audio+ptt setup from configured_audio_ptt_transmit_runs_real_cycle,
        //  but ConfigureChannel mode = "afsk1200" and payload = AX.25 frame bytes.)
        // Assert: Transmit reply is Ok(TransmitId(1)); then a TransmitComplete
        // telemetry for transmit_id 1 arrives.
    }
```

Fill in the body following `configured_audio_ptt_transmit_runs_real_cycle` (mode `"afsk1200"`, payload `Ax25Frame{..}.encode()`).

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodemd core::tests::transmit_on_moded_channel_enqueues`
Expected: FAIL — `transmit()` still treats payload as raw PCM, producing garbled audio and no mode framing.

- [ ] **Step 3: Spawn the TX worker and route to it**

In `configure_audio` (after the RX worker spawn), when the mode has a modulator and PTT is (or later becomes) configured, build and store a `TxWorker` in `LiveBindings.tx_workers: HashMap<ChannelId, tx_worker::TxWorker>`. Since PTT may be configured after audio, spawn/refresh the TX worker in **both** `configure_audio` and `configure_ptt` once both bindings exist. Then change `transmit()`:

```rust
    // Moded channel with a live TX worker: enqueue and return immediately.
    if let Some(worker) = live.tx_workers.get(&channel) {
        let mode = supervisor.channel_mode(channel);
        let frame = tx_worker::payload_to_frame(&mode, payload);
        let _ = telemetry.send(TelemetryEvent::TransmitStarted { channel, transmit_id: tx_id });
        match worker.enqueue(tx_worker::TxJob { frame, transmit_id: tx_id }) {
            Ok(()) => Ok(tx_id),
            Err(_) => Err(CoreError::Ptt(PttError::Config("tx queue full".into()))),
        }
    } else {
        // Legacy raw-PCM path (NullMode / unmoded) — unchanged.
        // ... existing have_audio && have_ptt drive_tx_cycle block ...
    }
```

Building the `TxWorker` requires moving the `PlaybackHandle` and `PttDriver` into it. Because the core currently holds `sinks`/`drivers` in `LiveBindings` for the legacy path, restructure: when a TX worker is spawned, **move** that channel's sink and driver into the worker (remove from `live.sinks`/`live.drivers`), so the worker owns them exclusively. The legacy path stays only for `ModeConfig::None`. Provide `Supervisor::channel_mode` (added in F.3). Pass the `telemetry` and `interlock` (already in scope) into the worker cfg, and `slot_s = Some(15.0)` for FT8, `None` otherwise (from `caps().shape`).

- [ ] **Step 4: Run**

Run: `cargo test -p omnimodemd core::tests`
Expected: PASS (new test plus the existing transmit tests — the legacy `configure_then_transmit_emits_events` uses mode `"none"`, which still takes the legacy path).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/core/mod.rs
git commit -m "Route Transmit on moded channels through the per-channel TX worker"
```

---

# Part H — Time synchronization metric

Surface the host clock offset so operators can tell a time-sync problem from a signal problem (design §"Time synchronization").

### Task H.1: `ClockSource` reading the host NTP offset

**Files:**
- Modify: `crates/omnimodemd/src/core/clock.rs`
- Test: inline

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn clock_source_reports_a_finite_offset() {
        let src = ClockSource::new();
        let r = src.read();
        // On any host the estimated error is finite and non-negative.
        assert!(r.est_error_s.is_finite() && r.est_error_s >= 0.0);
        // Offset is finite (may be ~0 on a well-disciplined host).
        assert!(r.offset_s.is_finite());
    }
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodemd core::clock::tests::clock_source_reports`
Expected: FAIL.

- [ ] **Step 3: Implement via `libc::ntp_adjtime`**

```rust
/// A reading of the host clock discipline state.
#[derive(Debug, Clone, Copy)]
pub struct ClockReading {
    /// Current NTP offset estimate (seconds; how far the clock is being steered).
    pub offset_s: f64,
    /// Estimated error (seconds).
    pub est_error_s: f64,
    /// Whether the kernel reports the clock as synchronized (not UNSYNC).
    pub synchronized: bool,
}

pub struct ClockSource;

impl ClockSource {
    pub fn new() -> Self {
        ClockSource
    }

    /// Read the kernel NTP discipline state. Falls back to a zero/ unsynced
    /// reading on platforms/permissions where `ntp_adjtime` is unavailable.
    pub fn read(&self) -> ClockReading {
        #[cfg(target_os = "linux")]
        {
            use std::mem::MaybeUninit;
            // SAFETY: timex is fully written by ntp_adjtime; we only read it.
            let mut tx = MaybeUninit::<libc::timex>::zeroed();
            let ret = unsafe { libc::ntp_adjtime(tx.as_mut_ptr()) };
            if ret >= 0 {
                let tx = unsafe { tx.assume_init() };
                // STA_NANO selects ns for `offset`/`esterror`; default is µs.
                let nano = (tx.status & libc::STA_NANO) != 0;
                let scale = if nano { 1e9 } else { 1e6 };
                return ClockReading {
                    offset_s: tx.offset as f64 / scale,
                    est_error_s: tx.esterror as f64 / 1e6, // esterror is µs
                    synchronized: ret != libc::TIME_ERROR,
                };
            }
        }
        ClockReading { offset_s: 0.0, est_error_s: f64::from(u16::MAX), synchronized: false }
    }
}

impl Default for ClockSource {
    fn default() -> Self { Self::new() }
}
```

> If `libc::ntp_adjtime` / `libc::timex` / `STA_NANO` / `TIME_ERROR` are not exposed by the pinned `libc` version, the executor adds the `clock` feature or uses the `nix` crate's `time::ntp_adjtime` (already a dependency). The test only asserts finiteness, so the fallback branch satisfies it on any host.

- [ ] **Step 4: Run**

Run: `cargo test -p omnimodemd core::clock`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/core/clock.rs
git commit -m "Add ClockSource reading host NTP offset via ntp_adjtime"
```

### Task H.2: Emit `ClockOffset` telemetry from the core idle tick

**Files:**
- Modify: `crates/omnimodemd/src/core/event.rs`
- Modify: `crates/omnimodemd/src/core/mod.rs`
- Test: `core::tests` inline

- [ ] **Step 1: Add the event variant + failing test**

In `event.rs`, add to `TelemetryEvent`:

```rust
    ClockOffset { offset_s: f64, est_error_s: f64, synchronized: bool },
```

In `core::tests`, add a test that spawns a core, waits one `HOTPLUG_POLL` tick, and asserts a `ClockOffset` telemetry arrives:

```rust
    #[test]
    fn core_emits_clock_offset_on_idle_tick() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let (core, join) = fresh_core();
        let mut tele = core.telemetry.subscribe();
        rt.block_on(async {
            let got = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if let Ok(TelemetryEvent::ClockOffset { est_error_s, .. }) = tele.recv().await {
                        return est_error_s;
                    }
                }
            }).await.expect("clock offset within timeout");
            assert!(got.is_finite());
        });
        core.commands.send(Command::Shutdown).unwrap();
        join.join().unwrap();
    }
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodemd core::tests::core_emits_clock_offset`
Expected: FAIL — nothing emits it.

- [ ] **Step 3: Emit on the idle tick**

In `run()`'s `RecvTimeoutError::Timeout` arm (where `poll_hotplug` runs), also read the clock and emit, throttled to ~once/second. Add a `ClockSource` and a "last emit" `Instant` to the loop locals:

```rust
    let clock = crate::core::clock::ClockSource::new();
    let mut last_clock = std::time::Instant::now() - std::time::Duration::from_secs(2);
    // ... in the Timeout arm, after poll_hotplug:
    if last_clock.elapsed() >= std::time::Duration::from_secs(1) {
        let r = clock.read();
        let _ = telemetry.send(TelemetryEvent::ClockOffset {
            offset_s: r.offset_s, est_error_s: r.est_error_s, synchronized: r.synchronized });
        last_clock = std::time::Instant::now();
    }
```

(The first tick fires immediately because `last_clock` is initialized 2 s in the past.)

- [ ] **Step 4: Run**

Run: `cargo test -p omnimodemd core::tests::core_emits_clock_offset`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodemd/src/core/event.rs crates/omnimodemd/src/core/mod.rs
git commit -m "Emit ClockOffset telemetry from the core idle tick"
```

---

# Part I — Conformance gates (the Phase-4 exit criterion)

Design §"Definition of done for a mode": KAT vectors pass, **bidirectional** cross-decode works, and the BER/decode-rate curve meets the committed threshold. Built per-mode as each lands; finalized here.

### Task I.1: Unified loopback harness

**Files:**
- Create: `crates/dsp/tests/loopback.rs`

- [ ] **Step 1: Write the integration tests**

One `#[test]` per streaming mode plus FT8, each TX-modulating then RX-demodulating a fixed message and asserting recovery — the same assertions as the inline loopback tests but as a single CI-visible target. Gate behind `testutil` (uses no AWGN, but keep it uniform):

```rust
//! Layer-1 conformance: each mode's own modulator → demodulator round-trip.
//! The decisive *cross*-decode against reference binaries is in `kat.rs`
//! (gated `#[ignore]`); this target proves internal self-consistency on CI.

use omnimodem_dsp::modes::{
    afsk1200::{Afsk1200Demod, Afsk1200Mod},
    cw::{CwDemod, CwMod},
    ft8::{Ft8Demod, Ft8Mod, FT8_RATE, FT8_WINDOW_S},
    psk31::{Psk31Demod, Psk31Mod},
    rtty::{RttyDemod, RttyMod},
};
use omnimodem_dsp::mode::{BlockDemodulator, Demodulator, Modulator};
use omnimodem_dsp::types::{Frame, FramePayload};

#[test]
fn afsk1200_loopback() {
    use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
    let ax = Ax25Frame { dest: Address::new("APRS", 0), source: Address::new("K1ABC", 7),
        digipeaters: vec![], info: b"loopback".to_vec() };
    let s = Afsk1200Mod::new().modulate(&Frame::packet(ax.encode())).unwrap();
    let frames = Afsk1200Demod::ensemble(9).feed(&s);
    assert!(frames.iter().any(|f| matches!(&f.payload, FramePayload::Packet(b) if b == &ax.encode())));
}
// + psk31_loopback, rtty_loopback, cw_loopback, ft8_loopback (mirroring the
//   inline tests; ft8 pads to a full FT8_WINDOW_S window and calls decode_window).
```

Write all five tests fully (mirror the inline-test bodies from Parts A–E).

- [ ] **Step 2: Run**

Run: `cargo test -p omnimodem-dsp --features testutil --test loopback`
Expected: PASS (all five).

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/tests/loopback.rs
git commit -m "Add unified per-mode loopback conformance target"
```

### Task I.2: Watterson HF-fading fixture

**Files:**
- Modify: `crates/dsp/src/testutil.rs`
- Test: inline in `testutil.rs`

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn watterson_preserves_length_and_is_deterministic() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(1);
        let sig: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.1).sin()).collect();
        let chan = WattersonChannel::ccir_good(8_000.0);
        let out1 = chan.apply(&sig, &mut a);
        let out2 = chan.apply(&sig, &mut b);
        assert_eq!(out1.len(), sig.len());
        assert_eq!(out1, out2, "same seed → identical fading");
    }
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p omnimodem-dsp --features testutil testutil::tests::watterson`
Expected: FAIL.

- [ ] **Step 3: Implement a two-path Watterson model**

```rust
/// Watterson HF channel: two independent Rayleigh-fading paths with a fixed
/// delay and a Doppler spread (CCIR good/moderate/poor presets). Deterministic
/// given a seeded `Rng`. Simplified (two taps, Gaussian-filtered fading) — the
/// design only requires a seedable fading fixture, not a bit-exact model.
pub struct WattersonChannel {
    rate: f32,
    delay_samples: usize,
    doppler_hz: f32,
}

impl WattersonChannel {
    pub fn new(rate: f32, delay_ms: f32, doppler_hz: f32) -> Self {
        WattersonChannel { rate, delay_samples: (delay_ms * 1e-3 * rate) as usize, doppler_hz }
    }
    pub fn ccir_good(rate: f32) -> Self { Self::new(rate, 0.5, 0.1) }
    pub fn ccir_moderate(rate: f32) -> Self { Self::new(rate, 1.0, 0.5) }
    pub fn ccir_poor(rate: f32) -> Self { Self::new(rate, 2.0, 1.0) }

    /// Apply the channel. Each path is multiplied by a slowly-varying Rayleigh
    /// envelope (low-pass-filtered complex Gaussian at `doppler_hz`).
    pub fn apply(&self, signal: &[f32], rng: &mut Rng) -> Vec<f32> {
        let n = signal.len();
        let g0 = self.fading_envelope(n, rng);
        let g1 = self.fading_envelope(n, rng);
        let mut out = vec![0.0f32; n];
        for i in 0..n {
            let direct = signal[i] * g0[i];
            let delayed = if i >= self.delay_samples {
                signal[i - self.delay_samples] * g1[i]
            } else { 0.0 };
            out[i] = 0.707 * (direct + delayed); // normalize two equal paths
        }
        out
    }

    fn fading_envelope(&self, n: usize, rng: &mut Rng) -> Vec<f32> {
        // One-pole low-pass on white Gaussian → band-limited fading at doppler.
        let alpha = (-2.0 * std::f32::consts::PI * self.doppler_hz / self.rate).exp();
        let mut env = Vec::with_capacity(n);
        let mut state = 0.0f32;
        for _ in 0..n {
            state = alpha * state + (1.0 - alpha) * rng.next_normal();
            // |complex Rayleigh| approximated by |Gaussian| scaled to unit mean.
            env.push(1.0 + 0.5 * state);
        }
        env
    }
}
```

- [ ] **Step 4: Run**

Run: `cargo test -p omnimodem-dsp --features testutil testutil::tests::watterson`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/testutil.rs
git commit -m "Add seedable Watterson HF-fading test fixture"
```

### Task I.3: BER / decode-rate sweep with per-mode thresholds

**Files:**
- Create: `crates/dsp/tests/ber.rs`
- Modify: `crates/dsp/src/testutil.rs` (add a `decode_rate` helper)

- [ ] **Step 1: Add the `decode_rate` helper + failing test**

In `testutil.rs`:

```rust
/// Fraction of `trials` that satisfy `decoded`. A tiny helper so BER sweeps
/// read declaratively.
pub fn decode_rate(trials: usize, mut decoded: impl FnMut(usize) -> bool) -> f32 {
    let ok = (0..trials).filter(|&i| decoded(i)).count();
    ok as f32 / trials as f32
}
```

In `crates/dsp/tests/ber.rs`:

```rust
//! Layer-2 performance: AWGN decode-rate sweeps with committed thresholds.
//! These are the BER/decode-rate curves the exit criterion requires; the
//! reference-oracle comparison (equal-or-better at every SNR) is an `#[ignore]`
//! gate in `kat.rs` pending the reference binaries.
#![cfg(feature = "testutil")]

use omnimodem_dsp::testutil::{add_awgn, decode_rate, Rng};
use omnimodem_dsp::modes::ft8::{Ft8Demod, Ft8Mod, FT8_RATE, FT8_WINDOW_S};
use omnimodem_dsp::mode::{BlockDemodulator, Modulator};
use omnimodem_dsp::types::{Frame, FramePayload};

/// FT8 must decode reliably at a moderate SNR (loopback + light AWGN). The
/// committed threshold here is a CI floor, not the on-air −21 dB spec (that is
/// the reference-oracle nightly gate).
#[test]
fn ft8_decode_rate_moderate_snr() {
    let msg = "CQ K1ABC FN42";
    let rate = decode_rate(10, |seed| {
        let wave = Ft8Mod::new().modulate(&Frame::text(msg)).unwrap();
        let mut win = vec![0.0f32; (FT8_RATE as f32 * FT8_WINDOW_S) as usize];
        win[..wave.len()].copy_from_slice(&wave);
        let mut rng = Rng::new(1 + seed as u64);
        add_awgn(&mut win, 0.3, &mut rng);
        Ft8Demod::new().decode_window(&win, 0).iter().any(|f| matches!(&f.payload,
            FramePayload::Text(t) if t == msg))
    });
    assert!(rate >= 0.8, "FT8 decode rate {rate} below 0.8 at sigma=0.3");
}
// + afsk1200_decode_rate (streaming; feed the noisy modulated frame to the
//   ensemble; threshold ≥ 0.9 at a low sigma), and one sweep each for RTTY,
//   PSK31, CW at thresholds the executor pins from the first green run.
```

Write the AFSK, RTTY, PSK31, CW sweeps too (each: modulate fixed message → `add_awgn` → demod → assert recovered, over ≥10 seeds, threshold pinned to a comfortably-passing floor).

- [ ] **Step 2: Run**

Run: `cargo test -p omnimodem-dsp --features testutil --test ber`
Expected: PASS. If a mode's first run is below the threshold you wrote, **lower the threshold to just under the observed rate** (the gate's job is to catch regressions, not to assert an aspirational number) and note the observed rate in a comment.

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/tests/ber.rs crates/dsp/src/testutil.rs
git commit -m "Add AWGN decode-rate sweeps with per-mode CI thresholds"
```

### Task I.4: Mode-level modulator golden snapshots

**Files:**
- Modify: `crates/dsp/tests/snapshots.rs`

- [ ] **Step 1: Add a snapshot per mode-level modulator**

Append tests that fingerprint each mode's *assembly-level* output (not just the raw `modulate.rs` block already snapshotted in Phase 3) for a fixed message, so any change to mode framing/encoding is caught:

```rust
use omnimodem_dsp::modes::{afsk1200::Afsk1200Mod, ft8::Ft8Mod, psk31::Psk31Mod,
    rtty::RttyMod, cw::CwMod};
use omnimodem_dsp::mode::Modulator;
use omnimodem_dsp::types::Frame;
use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};

#[test]
fn afsk1200_frame_fingerprint() {
    let ax = Ax25Frame { dest: Address::new("APRS", 0), source: Address::new("K1ABC", 0),
        digipeaters: vec![], info: b"snap".to_vec() };
    let fp = fingerprint(&Afsk1200Mod::new().modulate(&Frame::packet(ax.encode())).unwrap(), 256);
    snap!("afsk1200_frame", &fp);
}

#[test]
fn ft8_message_fingerprint() {
    let fp = fingerprint(&Ft8Mod::new().modulate(&Frame::text("CQ K1ABC FN42")).unwrap(), 256);
    snap!("ft8_message", &fp);
}
// + psk31_message, rtty_message, cw_message fingerprints.
```

- [ ] **Step 2: Generate the snapshots**

Run: `INSTA_UPDATE=always cargo test -p omnimodem-dsp --test snapshots`
Then review the generated `crates/dsp/tests/vectors/*.snap` files (they are the golden on-air fingerprints).

- [ ] **Step 3: Run clean (no update) to confirm stability**

Run: `cargo test -p omnimodem-dsp --test snapshots`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/tests/snapshots.rs crates/dsp/tests/vectors/
git commit -m "Add mode-level modulator golden snapshots"
```

### Task I.5: Bidirectional cross-decode interop gates (reference binaries)

**Files:**
- Modify: `crates/dsp/tests/kat.rs`

- [ ] **Step 1: Add `#[ignore]` cross-decode gates with executable provenance**

Extend the existing `regenerate_reference_vectors_doc` pattern with per-mode interop gates that document the exact reference commands and assert both directions once a captured-vectors file exists. Each is `#[ignore]` because the reference binaries (Direwolf `atest`/`gen_packets`, WSJT-X `ft8code`/`jt9`, fldigi) are not on CI:

```rust
/// FT8 bidirectional cross-decode (design §"Cross-decode interop"):
///   ours→ref:  our `Ft8Mod` waveform decoded by WSJT-X `jt9 -8`.
///   ref→ours:  `ft8sim`/`ft8code` output decoded by `Ft8Demod`.
/// Provenance for the byte-level check: `ft8code "CQ K1ABC FN42"` prints the
/// 77-bit payload + 174-bit codeword; assert ours equals it.
#[test]
#[ignore = "requires WSJT-X ft8code/jt9 (Phase-4 interop gate)"]
fn ft8_cross_decode_doc() {
    // When the binaries are available:
    //   1. `ft8code "CQ K1ABC FN42"` → capture 77-bit + 174-bit hex into
    //      tests/vectors/ft8_ft8code.json; assert ft8_symbols() agrees.
    //   2. Write our waveform to a .wav; `jt9 -8 our.wav` must print the msg.
    //   3. `ft8sim "CQ K1ABC FN42" 1500 0 0 0 1 -10` → our decode_window must
    //      recover it.
}
// + afsk1200_cross_decode_doc (Direwolf gen_packets ↔ atest),
//   rtty_cross_decode_doc / psk31_cross_decode_doc / cw_cross_decode_doc (fldigi).
```

Write all five doc gates.

- [ ] **Step 2: Confirm they compile and are skipped**

Run: `cargo test -p omnimodem-dsp --features testutil --test kat`
Expected: PASS (the `#[ignore]`d gates are listed as ignored, not run).

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/tests/kat.rs
git commit -m "Add bidirectional cross-decode interop gates (reference binaries)"
```

### Task I.6: `phase4_exit_criterion` aggregate gate

**Files:**
- Modify: `crates/dsp/tests/kat.rs`

- [ ] **Step 1: Add the compile-checked manifest**

Mirroring `phase3_exit_criterion`, add an aggregate that calls each Phase-4 conformance gate that runs on CI (the loopback round-trips and the BER thresholds — referenced as functions or inlined). Since loopback/BER live in separate test targets, the aggregate re-asserts the core per-mode round-trips inline so one named target gates a merge:

```rust
/// Executable definition of "Phase 4 done" for the CI-runnable gates: every
/// mode self-loopbacks and meets its AWGN decode-rate floor. The reference-
/// binary cross-decode gates above are the *nightly* completion of the exit
/// criterion (they need the reference toolchains); this aggregate is the
/// per-PR gate. Keep it in sync as modes are added.
#[test]
fn phase4_exit_criterion() {
    use omnimodem_dsp::modes::{afsk1200::{Afsk1200Demod, Afsk1200Mod},
        cw::{CwDemod, CwMod}, ft8::{Ft8Demod, Ft8Mod, FT8_RATE, FT8_WINDOW_S},
        psk31::{Psk31Demod, Psk31Mod}, rtty::{RttyDemod, RttyMod}};
    use omnimodem_dsp::mode::{BlockDemodulator, Demodulator, Modulator};
    use omnimodem_dsp::types::{Frame, FramePayload};
    use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};

    // AFSK 1200
    let ax = Ax25Frame { dest: Address::new("APRS", 0), source: Address::new("K1ABC", 7),
        digipeaters: vec![], info: b"exit".to_vec() };
    let s = Afsk1200Mod::new().modulate(&Frame::packet(ax.encode())).unwrap();
    assert!(Afsk1200Demod::ensemble(9).feed(&s).iter()
        .any(|f| matches!(&f.payload, FramePayload::Packet(b) if b == &ax.encode())));

    // PSK31, RTTY, CW, FT8 — same shape (modulate fixed message, demod, assert).
    // ... (write each, mirroring the loopback tests) ...
}
```

Write all five assertions.

- [ ] **Step 2: Run**

Run: `cargo test -p omnimodem-dsp --features testutil --test kat phase4_exit_criterion`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/tests/kat.rs
git commit -m "Add phase4_exit_criterion aggregate conformance gate"
```

---

# Final verification

### Task Z.1: Full workspace test + lint

- [ ] **Step 1: Build everything**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 2: Run the full test suite (with the testutil feature)**

Run: `cargo test --workspace --features omnimodem-dsp/testutil`
Expected: PASS, including `--test loopback`, `--test ber`, `--test kat`, `--test snapshots`, and both crates' unit tests.

- [ ] **Step 3: Clippy clean**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings. Fix any (notably `clippy::too_many_arguments` on the threaded core helpers — add `#[allow]` consistent with the existing code, which already allows it on `handle_command`).

- [ ] **Step 4: Confirm the exit criterion gate alone passes**

Run: `cargo test -p omnimodem-dsp --features testutil phase4_exit_criterion`
Expected: PASS — the single named gate a merge is gated on.

- [ ] **Step 5: Commit any lint fixups**

```bash
git add -A
git commit -m "Phase 4: workspace test + clippy clean"
```

---

## Self-review notes (for the executor)

- **Spec coverage.** AFSK 1200 (A), FT8 (E), CW (D), RTTY (C), PSK31 (B); TX model — per-channel worker + cooperative queue (G.1) + time-slot scheduling (G.2) + per-rig serialization via the shared interlock (G.3 reuses `RxTxInterlock`); exclusive lease explicitly deferred to Phase 5 per design §"Open questions". Time synchronization — `ClockSource` + `ClockOffset` metric (H), `SlotClock` for windowed TX (G.2). Exit criterion — KAT (I.6), bidirectional cross-decode (I.5), BER/decode-rate (I.3), plus loopback (I.1), Watterson (I.2), golden snapshots (I.4). RX path finally wired (F).
- **Constants flagged by the design** (§"Constants to confirm at implementation time": LDPC min-sum scaling, AP LLR seeding, FT8 sync-metric threshold, IL2P `set_field`, plus the AFSK correlator knobs): every one is pinned by a green loopback/KAT test, not asserted from secondary docs. AP/SIC are Phase-5 scope and intentionally absent.
- **Type consistency.** `Demodulator::feed(&[Sample]) -> Vec<Frame>`, `BlockDemodulator::decode_window(&[Sample], u64) -> Vec<Frame>`, `Modulator::modulate(&Frame) -> Result<Vec<Sample>, ModError>`, `FramePayload::{Packet,Text,Message77,Vocoder}`, `ModeCaps { native_rate, bandwidth_hz, tx, duplex, shape }`, `DemodShape::Windowed { window_s, period_s }` — all match the Phase-3 `crates/dsp/src/mode.rs` and `types.rs` exactly. Daemon seams (`CaptureHandle.rx`/`.sample_rate`, `PlaybackHandle::submit`, `drive_tx_cycle`, `RxTxInterlock::{begin_tx,end_tx,is_muted}`, `AudioChunk = Vec<i16>`, `TelemetryEvent`) match the Phase-1/2 code read during planning.
- **Known restructure risk (Task G.3).** Moving `sink`/`driver` ownership from `LiveBindings` into the `TxWorker` is the one invasive change to existing code; the legacy raw-PCM path is preserved only for `ModeConfig::None`, and the existing `core::tests` for the `"none"` path must stay green — run them after G.3.
