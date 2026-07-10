# Phase 7 — PSK Family Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port fldigi's full PSK family — BPSK PSK63/125/250/500/1000, the `+F` FEC variants, differential QPSK31–500, and the PSK-R robust + multi-carrier `nX_PSK*R` grid — from `fldigi/src/psk/` into omnimodem as bit-exact-compatible, cross-decoding modes, generalizing the existing PSK31 assembly by symbol rate and adding one new building block (`frontend/multicarrier.rs`).

**Architecture:** One parametric `crates/dsp/src/modes/psk.rs` assembly replaces the special-case `psk31.rs` internals with a `PskVariant` parameter table (baud, QPSK flag, FEC flag, robust/interleave depth, carrier count) driving a shared TX `Modulator` and RX `Demodulator`. The FEC layer reuses `fec::conv` (K=5 QPSK, K=7 PSK-R) and `fec::interleave`; the multi-carrier grid runs the single-carrier core over N frequency-offset carriers via a new `frontend/multicarrier.rs` block that is KAT-gated in isolation before any mode uses it. Every stage (varicode → FEC → interleave → symbol phase → audio) is checked against a golden vector extracted from fldigi, with bit-exact gates on the bit/symbol domain and an FP tolerance on audio (Doctrine §3). One arm per family in the daemon registry + one row per family in the TUI.

**Tech Stack:** Rust (edition 2021, workspace). Reuses `omnimodem-dsp` blocks: `framing::varicode` (the PSK31 Varicode table already ported), `frontend::modulate::DiffPsk`, `frontend::{nco,fir,osc}`, `sync::{costas,timing}`, `fec::conv` (`ConvCode` + soft Viterbi), `fec::interleave`. Reference: `fldigi/src/psk/{psk.cxx,pskvaricode.cxx,pskcoeff.cxx,pskeval.cxx}` (upstream commit recorded in each ported file's doc header at T1). Tests: `crates/dsp/tests/{kat.rs,ber.rs,loopback.rs,snapshots.rs}`, `testutil` AWGN/Watterson fixtures, `crates/dsp/tests/vectors/`. TUI: Go, `clients/omnimodem-tui/internal/app/modes.go` + proto `ModeParams`.

---

## Porting doctrine recap (binding — read `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md` "The Porting Doctrine" first)

- **Bit-exact vs FP-tolerance.** Varicode bits, FEC codewords, interleaver permutations, and symbol *phase indices* MUST match fldigi byte-for-byte. Modulated audio and soft LLRs are gated on tolerance/correlation only — never asserted bit-exact.
- **No stubs.** No `todo!()`/`unimplemented!()`/canned returns on any reachable path. A family task closes only when its KAT + loopback + BER gate is green. A submode that cannot pass its gate stays open; it is not merged behind a stub.
- **Provenance in code.** `psk.rs` opens with a doc comment naming `fldigi/src/psk/psk.cxx` + commit hash + any FP tolerance chosen and why. Every ported table/constant carries a `// ref: fldigi/src/psk/<file>:<lines>` cite.
- **Parametric families, table-tested.** Port each family once on one representative submode (T1–T8), then a table-driven test enumerates the rate/carrier grid against fldigi's parameter table. One task per family, NOT per rate.

## Key reference facts extracted from `fldigi/src/psk/` (transcribe these verbatim at implement-time)

- **Base sample rate** `samplerate = 8000` for all Phase-7 modes (16000 is 8PSK/OFDM only → Phase 16, out of scope). `// ref: psk.cxx:370`.
- **Symbol length (samples) → baud**, `baud = 8000 / symbollen`. `// ref: psk.cxx:381-901`:
  | Mode | symbollen | baud | `dcdbits` (preamble/postamble) | fir_type |
  |---|---|---|---|---|
  | PSK31 / QPSK31 | 256 | 31.25 | 32 | PSK_CORE |
  | PSK63 / QPSK63 / PSK63F | 128 | 62.5 | 64 | PSK_CORE |
  | PSK125 / QPSK125 / PSK125R | 64 | 125 | 128 | SINC |
  | PSK250 / QPSK250 / PSK250R | 32 | 250 | 256 | SINC |
  | PSK500 / QPSK500 / PSK500R | 16 | 500 | 512 | SINC |
  | PSK1000 / PSK1000R | 8 | 1000 | 128 / 512 | SINC |
- **QPSK FEC:** `K=5`, `POLY1=0x17`, `POLY2=0x19`. `// ref: psk.cxx:66-68` and the encoder/viterbi init `// ref: psk.cxx:979-981`.
- **PSK-R (robust + `+F`) FEC:** `PSKR_K=7`, `PSKR_POLY1=0x6d`, `PSKR_POLY2=0x4f`. `// ref: psk.cxx:70-74`, init `// ref: psk.cxx:983-992`.
- **Interleaver:** fldigi `interleave(isize=2, idepth, ...)`. `idepth` per mode `// ref: psk.cxx`: PSK125R=40, PSK250R=80, PSK500R=160, PSK1000R=160; the `nX` grid uses 40–260. PSK63F (`+F` at 62.5) uses **no interleaver** (`if (mode != MODE_PSK63F) Rxinlv->symbols(...)` `// ref: psk.cxx:1234,1268`).
- **Multi-carrier spacing:** `sc_bw = samplerate / symbollen`; `separation = 1.4`; `inter_carrier = separation * sc_bw`; carrier 0 sits at `center + ((-numcarriers)+1) * inter_carrier / 2`, each next carrier `+inter_carrier`. `// ref: psk.cxx:372, 1057-1061, 2008-2013`.
- **Varicode:** BPSK modes use PSK31 Varicode (`pskvaricode.cxx:31-288`) — already ported in `crates/dsp/src/framing/varicode.rs::PSK31`. The `+F`/robust modes use the **MFSK Varicode** per Multipsk (`psk.cxx:684 comment "BPSK63 + FEC + MFSK Varicode"`); MFSK Varicode is a Phase-8 port, so **Phase 7 ports `+F`/robust with the PSK31 Varicode as fldigi does for the BPSK payload path and pins the codec choice against the extracted golden vector** (T2 asserts which table fldigi actually emits; if the vector shows MFSK Varicode, that sub-codec is transcribed here from `mfsk/mfskvaricode.cxx` — do not hand-wave, assert against the vector).
- **Differential phase rule (BPSK):** RX `phase = arg(conj(prev)*sym)`; `bits = ((int)(phase/π + 0.5) & 1) << 1` — a reversal is a data `0`, steady carrier a `1`. `// ref: psk.cxx:1477-1528`. This matches the single-differential rule already in `psk31.rs`.
- **Differential phase rule (QPSK):** `n=4; bits = ((int)(phase/(π/2) + 0.5)) & 3`. TX: `if (_qpsk && !reverse) sym = (4 - sym) & 3;` then multiply the running `prevsymbol` phasor. `// ref: psk.cxx:1188-1214 (rx_qpsk), 2212-2281 (tx_carriers)`.
- **RX matched filter coeffs:** `pskcore_filter[65]` (PSK_CORE modes) and the `wsincfilt`-generated sinc (SINC modes), `FIRLEN=64`. `// ref: pskcoeff.cxx:203-269, 285-312; psk.cxx:938-957`.
- **Preamble/postamble:** BPSK sends `dcdbits` phase reversals (`tx_symbol(0)` preamble, `tx_symbol(2)` postamble) `// ref: psk.cxx:2621-2628, 2539-2544`; PSK-R sends `preamble/2` alternating `1/0` bit pairs then a `<NUL>` char `// ref: psk.cxx:2562-2577`.

---

## File structure

**Created:**

| File | Responsibility |
|---|---|
| `crates/dsp/src/frontend/multicarrier.rs` | `MultiCarrier` TX/RX block: N frequency-offset carriers, `inter_carrier = 1.4 * sc_bw` spacing, per-carrier NCO up/down-conversion + summation (`/numcarriers`). KAT-gated in isolation first. |
| `crates/dsp/src/modes/psk.rs` | Parametric PSK assembly: `PskVariant` param table + `PskMod`/`PskDemod` covering all Phase-7 BPSK/BPSK+F/QPSK/PSK-R/multi-carrier submodes. Supersedes `psk31.rs`. |
| `crates/dsp/tests/vectors/psk_bpsk.json` | Golden vectors (varicode bits, symbol phases, audio ref) for one BPSK rate, from fldigi. |
| `crates/dsp/tests/vectors/psk_qpsk.json` | Golden vectors (FEC codeword, symbol phases) for QPSK, from fldigi. |
| `crates/dsp/tests/vectors/psk_pskr.json` | Golden vectors (FEC + interleave + carrier symbols) for a PSK-R / multi-carrier submode, from fldigi. |
| `scratch/refvectors/psk_dump.cxx` | fldigi extraction driver (scratch, per CLAUDE.md) linking `psk.cxx`/`pskvaricode.cxx` to print intermediates. |

**Modified:**

| File | Change |
|---|---|
| `crates/dsp/src/frontend/mod.rs` | `pub mod multicarrier;` |
| `crates/dsp/src/modes/mod.rs` | `pub mod psk;` (keep `pub mod psk31;` re-exporting `psk::` aliases for back-compat until the registry migrates). |
| `crates/dsp/src/modes/psk31.rs` | Reduce to a thin `pub use crate::modes::psk::{...}` shim so existing `Psk31Mod`/`Psk31Demod` callers/tests keep working. |
| `crates/omnimodem/src/mode/mod.rs` | `ModeConfig::Psk` variant (parametric over submode + center) + `parse`/`to_mode_string`/`label` arms for every Phase-7 label. |
| `crates/omnimodem/src/mode/registry.rs` | `demod_kind`/`build_modulator`/`native_rate`/`tx_slot_s` arms for the new `Psk` variant. |
| `proto/omnimodem.proto` | Add `PskParams { string submode; float center_hz; }` to the `ModeParams` oneof. |
| `clients/omnimodem-tui/internal/app/modes.go` | Add PSK-family rows to `modes` + a `psk` arm in `modeParamsFor`. |
| `clients/omnimodem-tui/internal/pb/*` | Regenerate Go proto via `clients/omnimodem-tui/gen.sh` (adds `PskParams`). |
| `crates/dsp/tests/kat.rs` | Per-family KAT (varicode/FEC/symbol-phase bit-exact; audio tolerance) + `#[ignore]` cross-decode gates; extend a `phase7_exit_criterion` aggregate. |
| `crates/dsp/tests/ber.rs` | AWGN + Watterson decode-rate sweeps per family with committed floors. |
| `crates/dsp/tests/loopback.rs` | TX→RX round-trips for one submode per family + the rate/carrier grid. |
| `crates/dsp/tests/snapshots.rs` | Modulator golden snapshots for one BPSK + one QPSK submode. |

---

# Task 0 — The `multicarrier` building block (KAT-gated in isolation first)

**Rationale:** the `nX_PSK*R` grid needs N parallel carriers; fldigi builds this into the modem loop, but per the doctrine we isolate it as a reusable block and KAT it standalone **before** any mode depends on it, so a multi-carrier decode failure localizes to the block, not the mode.

**Files:**
- Create: `crates/dsp/src/frontend/multicarrier.rs`
- Modify: `crates/dsp/src/frontend/mod.rs`
- Test: inline `#[cfg(test)] mod tests` in `multicarrier.rs`

- [ ] **Step 1: Write the failing test** (`crates/dsp/src/frontend/multicarrier.rs`, in `#[cfg(test)] mod tests`)

```rust
#[test]
fn carrier_frequencies_match_fldigi_layout() {
    // fldigi: sc_bw = samplerate/symbollen; inter = 1.4*sc_bw;
    // f[0] = center + ((-N)+1)*inter/2; f[k] = f[k-1] + inter.
    // ref: psk.cxx:1057-1061, 2008-2013.
    let mc = MultiCarrier::new(8000.0, /*center*/ 1500.0, /*symbollen*/ 128, /*n*/ 4);
    let sc_bw = 8000.0 / 128.0; // 62.5
    let inter = 1.4 * sc_bw; // 87.5
    let f0 = 1500.0 + ((-4.0) + 1.0) * inter / 2.0; // 1500 - 131.25
    let want = [f0, f0 + inter, f0 + 2.0 * inter, f0 + 3.0 * inter];
    for (i, &w) in want.iter().enumerate() {
        assert!((mc.carrier_hz(i) - w).abs() < 1e-3, "carrier {i}: {} != {w}", mc.carrier_hz(i));
    }
}

#[test]
fn single_carrier_up_down_roundtrips_phase() {
    // With N=1 the block is a pass-through NCO pair: a run of symbol phasors
    // modulated up then down-converted recovers the same de-rotated phase sign.
    let mut mc = MultiCarrier::new(8000.0, 1000.0, 256, 1);
    let syms = vec![vec![Cplx::new(1.0, 0.0)], vec![Cplx::new(-1.0, 0.0)]];
    let audio = mc.modulate_symbols(&syms, 256);
    let bb = mc.demodulate(&audio, 256); // Vec<Vec<Cplx>>: per-carrier symbol samples
    assert_eq!(bb.len(), 1);
    // consecutive symbols reversed => their dot product is negative
    let a = bb[0][0];
    let b = bb[0][1];
    assert!((a.re * b.re + a.im * b.im) < 0.0, "reversal not preserved");
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test -p omnimodem-dsp --lib frontend::multicarrier`
Expected: FAIL — `MultiCarrier` does not exist (unresolved import / no module).

- [ ] **Step 3: Implement `MultiCarrier`** (`crates/dsp/src/frontend/multicarrier.rs`)

```rust
//! N parallel PSK carriers for the fldigi multi-carrier (nX) robust modes.
//!
//! Ports fldigi's carrier-spacing and per-carrier NCO summation out of the
//! monolithic modem loop into a reusable block. TX up-converts each carrier's
//! symbol phasor stream and sums them (scaled by 1/N); RX runs one down-
//! converter per carrier and returns per-carrier baseband symbol samples.
//! ref: fldigi/src/psk/psk.cxx:1057-1061 (spacing), 2008-2013 (rx freqs),
//!      2221-2281 (tx carriers).

use crate::frontend::nco::DownConverter;
use crate::types::{Cplx, Sample};
use std::f32::consts::TAU;

/// fldigi's carrier separation factor (`separation` in psk.cxx:372).
const SEPARATION: f32 = 1.4;

pub struct MultiCarrier {
    rate: f32,
    freqs: Vec<f32>,
    down: Vec<DownConverter>,
    // per-carrier TX phase accumulators (fldigi `phaseacc[car]`).
    tx_phase: Vec<f32>,
}

impl MultiCarrier {
    pub fn new(rate: f32, center_hz: f32, symbollen: usize, numcarriers: usize) -> Self {
        let sc_bw = rate / symbollen as f32;
        let inter = SEPARATION * sc_bw;
        let f0 = center_hz + ((-(numcarriers as f32)) + 1.0) * inter / 2.0;
        let freqs: Vec<f32> = (0..numcarriers).map(|k| f0 + k as f32 * inter).collect();
        let down = freqs.iter().map(|&f| DownConverter::new(f, rate)).collect();
        MultiCarrier { rate, tx_phase: vec![0.0; numcarriers], freqs, down }
    }

    pub fn num_carriers(&self) -> usize {
        self.freqs.len()
    }
    pub fn carrier_hz(&self, k: usize) -> f32 {
        self.freqs[k]
    }

    /// Modulate a per-symbol slice of per-carrier phasors into a raised-cosine
    /// shaped audio stream. `symbols[s][car]` is the target phasor for carrier
    /// `car` at symbol `s`; `sps` samples per symbol. Envelope shaping matches
    /// fldigi tx_shape (0.5*cos + 0.5). ref: psk.cxx:1043-1055, 2245-2276.
    pub fn modulate_symbols(&mut self, symbols: &[Vec<Cplx>], sps: usize) -> Vec<Sample> {
        let n = self.freqs.len();
        let mut out = vec![0.0f32; symbols.len() * sps];
        let mut prev = vec![Cplx::new(1.0, 0.0); n]; // fldigi prevsymbol seed
        for (s, syms) in symbols.iter().enumerate() {
            for car in 0..n {
                let cur = syms[car];
                let dphi = TAU * self.freqs[car] / self.rate;
                for i in 0..sps {
                    // tx_shape[i] = 0.5*cos(i*pi/sps) + 0.5 (ref: psk.cxx:1046).
                    let a = 0.5 * (i as f32 * std::f32::consts::PI / sps as f32).cos() + 0.5;
                    let b = 1.0 - a;
                    let ival = a * prev[car].re + b * cur.re;
                    let qval = a * prev[car].im + b * cur.im;
                    let ph = self.tx_phase[car];
                    out[s * sps + i] += (ival * ph.cos() + qval * ph.sin()) / n as f32;
                    self.tx_phase[car] += dphi;
                    if self.tx_phase[car] > TAU {
                        self.tx_phase[car] -= TAU;
                    }
                }
                prev[car] = cur;
            }
        }
        out
    }

    /// Down-convert to per-carrier baseband and integrate-and-dump each symbol,
    /// returning `out[car][symbol]` matched-filter samples. `sps` samples/symbol.
    pub fn demodulate(&mut self, audio: &[Sample], sps: usize) -> Vec<Vec<Cplx>> {
        let n = self.freqs.len();
        let mut out = vec![Vec::with_capacity(audio.len() / sps); n];
        let mut acc = vec![Cplx::new(0.0, 0.0); n];
        for (i, &x) in audio.iter().enumerate() {
            for car in 0..n {
                acc[car] += self.down[car].push(x);
            }
            if (i + 1) % sps == 0 {
                for car in 0..n {
                    out[car].push(acc[car]);
                    acc[car] = Cplx::new(0.0, 0.0);
                }
            }
        }
        out
    }
}
```

Then add `pub mod multicarrier;` to `crates/dsp/src/frontend/mod.rs`. Add `use crate::types::Cplx;` in the test module.

- [ ] **Step 4: Run the tests, verify pass**

Run: `cargo test -p omnimodem-dsp --lib frontend::multicarrier`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/frontend/multicarrier.rs crates/dsp/src/frontend/mod.rs
git commit -m "feat(dsp): multicarrier frontend block for fldigi nX PSK modes"
```

---

# Task 1 — BPSK rate family (PSK63/125/250/500/1000) — T1–T9

Re-parametrizes the existing differential-BPSK + Varicode assembly by symbol rate. This is the foundation family; QPSK and PSK-R build on its `PskVariant` table and modulator/demodulator skeleton.

**Files:**
- Create: `crates/dsp/src/modes/psk.rs`, `crates/dsp/tests/vectors/psk_bpsk.json`, `scratch/refvectors/psk_dump.cxx`
- Modify: `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/modes/psk31.rs`, `crates/omnimodem/src/mode/{mod.rs,registry.rs}`, `proto/omnimodem.proto`, `crates/dsp/tests/{kat.rs,ber.rs,loopback.rs,snapshots.rs}`, `clients/omnimodem-tui/internal/app/modes.go`
- Test: `crates/dsp/tests/{kat.rs,ber.rs,loopback.rs}`, inline tests in `psk.rs`

### T1 — Extract golden vectors

- [ ] **Step 1: Write the fldigi extraction driver** (`scratch/refvectors/psk_dump.cxx`)

Link `fldigi/src/psk/psk.cxx` + `pskvaricode.cxx` + `pskcoeff.cxx` and, for a fixed message `"CQ DE K1ABC"` at each BPSK rate, print: (a) the PSK31 Varicode bitstream (`psk_varicode_encode` per char + `00` separators), (b) the differential BPSK symbol phase-index sequence (0 = reversal, 1 = steady) fed to `tx_symbol`, (c) the first 2048 modulated audio samples of `PSK125`. Record the exact compile/run command in the file header.

```bash
# scratch/refvectors/ — documented command (run where fldigi builds):
# g++ -I fldigi/src/include psk_dump.cxx fldigi/src/psk/pskvaricode.cxx \
#     fldigi/src/psk/pskcoeff.cxx -o psk_dump && ./psk_dump > psk_bpsk.raw
```

- [ ] **Step 2: Author the vector file** (`crates/dsp/tests/vectors/psk_bpsk.json`) with a leading `_meta` provenance record (upstream commit + `psk.cxx`/`pskvaricode.cxx` file refs + the driver command per `vectors/README.md`) and one record for `"CQ DE K1ABC"`:

```json
{ "_meta": { "upstream": "fldigi <COMMIT>", "files": ["src/psk/pskvaricode.cxx:31-288", "src/psk/psk.cxx:2212-2320"], "cmd": "scratch/refvectors/psk_dump" },
  "msg": "CQ DE K1ABC",
  "varicode_bits": "…0/1 string, MSB-first with 00 separators…",
  "bpsk_symbol_phase": "…0/1 per symbol (0=reversal,1=steady)…",
  "psk125_audio_head": [ /* first 2048 f32 samples */ ] }
```

- [ ] **Step 3: Commit**

```bash
git add crates/dsp/tests/vectors/psk_bpsk.json scratch/refvectors/psk_dump.cxx
git commit -m "test(psk): fldigi BPSK golden vectors + extraction driver"
```

### T2 — Port the source/char codec (parameter table + Varicode reuse)

- [ ] **Step 1: Write the failing test** (`crates/dsp/tests/kat.rs`)

```rust
#[test]
fn psk_bpsk_varicode_matches_fldigi_vector() {
    use omnimodem_dsp::modes::psk::{PskVariant, encode_bpsk_bits};
    // The BPSK payload bitstream (Varicode + 00 separators) is bit-exact vs
    // fldigi. Vector: tests/vectors/psk_bpsk.json.
    let want = load_bits("psk_bpsk.json", "varicode_bits"); // helper reads the 0/1 string
    let got = encode_bpsk_bits(PskVariant::Psk125, "CQ DE K1ABC");
    assert_eq!(got, want, "PSK125 varicode payload differs from fldigi");
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test -p omnimodem-dsp --features testutil psk_bpsk_varicode_matches_fldigi_vector`
Expected: FAIL — `modes::psk` does not exist.

- [ ] **Step 3: Implement the `PskVariant` table + BPSK bit encoder** (`crates/dsp/src/modes/psk.rs`)

```rust
//! PSK family: differential BPSK/QPSK + PSK31/MFSK Varicode + optional K=5/K=7
//! convolutional FEC + interleave, single- and multi-carrier. Port of
//! fldigi/src/psk/{psk.cxx,pskvaricode.cxx,pskcoeff.cxx}. Upstream commit:
//! <COMMIT>. Wire-determining arithmetic (varicode bits, FEC codewords,
//! interleave permutation, symbol phase indices) is bit-exact vs fldigi;
//! modulated audio is gated on FP tolerance only (Doctrine §3).

use crate::fec::conv::ConvCode;
use crate::framing::varicode::{decode as vari_decode, encode as vari_encode, PSK31};
use crate::frontend::modulate::DiffPsk;
use crate::types::Sample;

pub const PSK_RATE: u32 = 8_000; // fldigi samplerate for all Phase-7 PSK. ref: psk.cxx:370

/// Every Phase-7 PSK submode as a data-driven parameter set. ref: psk.cxx:381-901.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PskVariant {
    // BPSK rates
    Psk31, Psk63, Psk125, Psk250, Psk500, Psk1000,
    // BPSK + FEC (+F)
    Psk63F, Psk125F, Psk250F, Psk500F,
    // Differential QPSK
    Qpsk31, Qpsk63, Qpsk125, Qpsk250, Qpsk500,
    // PSK-R robust (single carrier)
    Psk125R, Psk250R, Psk500R, Psk1000R,
    // Multi-carrier robust grid (numcarriers > 1)
    Psk63Rc4, Psk63Rc5, Psk63Rc10, Psk63Rc20, Psk63Rc32,
    Psk125Rc4, Psk125Rc5, Psk125Rc10, Psk125Rc12, Psk125Rc16,
    Psk250Rc2, Psk250Rc3, Psk250Rc5, Psk250Rc6, Psk250Rc7,
    Psk500Rc2, Psk500Rc3, Psk500Rc4,
}

/// The resolved parameters for one variant. `symbollen` in samples => baud =
/// PSK_RATE/symbollen. `qpsk`/`fec`/`idepth`/`carriers` drive the pipeline.
#[derive(Debug, Clone, Copy)]
pub struct PskParams {
    pub symbollen: usize,
    pub qpsk: bool,
    pub fec: bool,      // K=5 (QPSK) or K=7 (PSK-R/+F) conv layer present
    pub robust: bool,   // PSK-R family: K=7 FEC + interleave
    pub idepth: usize,  // interleaver depth; 0 = none (also PSK63F, ref: psk.cxx:1234)
    pub carriers: usize,
    pub dcdbits: usize, // preamble/postamble length in reversals/bit-pairs
    pub psk_core_fir: bool, // PSK_CORE (symbollen>=128) vs SINC matched filter
}

impl PskVariant {
    /// ref: psk.cxx:381-901 (symbollen/qpsk/_pskr/idepth/numcarriers switch).
    pub fn params(self) -> PskParams {
        use PskVariant::*;
        // symbollen: 256=31.25, 128=62.5, 64=125, 32=250, 16=500, 8=1000 baud.
        let p = |symbollen, qpsk, robust, idepth, carriers, dcdbits| PskParams {
            symbollen, qpsk, fec: qpsk || robust, robust, idepth, carriers, dcdbits,
            psk_core_fir: symbollen >= 128,
        };
        match self {
            Psk31 => p(256, false, false, 0, 1, 32),
            Psk63 => p(128, false, false, 0, 1, 64),
            Psk125 => p(64, false, false, 0, 1, 128),
            Psk250 => p(32, false, false, 0, 1, 256),
            Psk500 => p(16, false, false, 0, 1, 512),
            Psk1000 => p(8, false, false, 0, 1, 128),
            // +F: K=7 FEC, MFSK Varicode, no interleave at 63; interleave at >=125.
            Psk63F => p(128, false, true, 0, 1, 64),      // ref: psk.cxx:684 (no inlv)
            Psk125F => p(64, false, true, 40, 1, 128),
            Psk250F => p(32, false, true, 80, 1, 256),
            Psk500F => p(16, false, true, 160, 1, 512),
            // QPSK: K=5 FEC.
            Qpsk31 => p(256, true, false, 0, 1, 32),
            Qpsk63 => p(128, true, false, 0, 1, 64),
            Qpsk125 => p(64, true, false, 0, 1, 128),
            Qpsk250 => p(32, true, false, 0, 1, 256),
            Qpsk500 => p(16, true, false, 0, 1, 512),
            // PSK-R robust (idepth ref: psk.cxx MODE_PSKnnnR cases).
            Psk125R => p(64, false, true, 40, 1, 128),
            Psk250R => p(32, false, true, 80, 1, 256),
            Psk500R => p(16, false, true, 160, 1, 512),
            Psk1000R => p(8, false, true, 160, 1, 512),
            // Multi-carrier grid (symbollen/idepth/carriers ref: psk.cxx:688-893).
            Psk63Rc4 => p(128, false, true, 80, 4, 128),
            Psk63Rc5 => p(128, false, true, 260, 5, 512),
            Psk63Rc10 => p(128, false, true, 160, 10, 512),
            Psk63Rc20 => p(128, false, true, 160, 20, 512),
            Psk63Rc32 => p(128, false, true, 160, 32, 512),
            Psk125Rc4 => p(64, false, true, 40, 4, 128),
            Psk125Rc5 => p(64, false, true, 200, 5, 512),
            Psk125Rc10 => p(64, false, true, 160, 10, 512),
            Psk125Rc12 => p(64, false, true, 160, 12, 512),
            Psk125Rc16 => p(64, false, true, 160, 16, 512),
            Psk250Rc2 => p(32, false, true, 80, 2, 256),
            Psk250Rc3 => p(32, false, true, 160, 3, 512),
            Psk250Rc5 => p(32, false, true, 160, 5, 512),
            Psk250Rc6 => p(32, false, true, 160, 6, 512),
            Psk250Rc7 => p(32, false, true, 160, 7, 512),
            Psk500Rc2 => p(16, false, true, 160, 2, 512),
            Psk500Rc3 => p(16, false, true, 160, 3, 512),
            Psk500Rc4 => p(16, false, true, 160, 4, 512),
        }
    }
    pub fn baud(self) -> f32 {
        PSK_RATE as f32 / self.params().symbollen as f32
    }
    pub fn samples_per_symbol(self) -> usize {
        self.params().symbollen
    }
    /// The K=5 QPSK / K=7 PSK-R convolutional code, or None for plain BPSK.
    /// ref: psk.cxx:66-74. QPSK: K=5 POLY 0x17/0x19. PSK-R: K=7 POLY 0x6d/0x4f.
    pub fn conv_code(self) -> Option<ConvCode> {
        let pp = self.params();
        if pp.qpsk {
            Some(ConvCode { k: 5, polys: vec![0x17, 0x19] }) // ref: psk.cxx:66-68
        } else if pp.robust {
            Some(ConvCode { k: 7, polys: vec![0x6d, 0x4f] }) // ref: psk.cxx:70-74
        } else {
            None
        }
    }
    /// Parse the fldigi/omnimodem label (e.g. "psk125", "qpsk63", "psk250r",
    /// "psk63rc4"). Returns None for unknown labels.
    pub fn from_label(s: &str) -> Option<PskVariant> {
        use PskVariant::*;
        Some(match s {
            "psk31" => Psk31, "psk63" => Psk63, "psk125" => Psk125,
            "psk250" => Psk250, "psk500" => Psk500, "psk1000" => Psk1000,
            "psk63f" => Psk63F, "psk125f" => Psk125F, "psk250f" => Psk250F, "psk500f" => Psk500F,
            "qpsk31" => Qpsk31, "qpsk63" => Qpsk63, "qpsk125" => Qpsk125,
            "qpsk250" => Qpsk250, "qpsk500" => Qpsk500,
            "psk125r" => Psk125R, "psk250r" => Psk250R, "psk500r" => Psk500R, "psk1000r" => Psk1000R,
            "psk63rc4" => Psk63Rc4, "psk63rc5" => Psk63Rc5, "psk63rc10" => Psk63Rc10,
            "psk63rc20" => Psk63Rc20, "psk63rc32" => Psk63Rc32,
            "psk125rc4" => Psk125Rc4, "psk125rc5" => Psk125Rc5, "psk125rc10" => Psk125Rc10,
            "psk125rc12" => Psk125Rc12, "psk125rc16" => Psk125Rc16,
            "psk250rc2" => Psk250Rc2, "psk250rc3" => Psk250Rc3, "psk250rc5" => Psk250Rc5,
            "psk250rc6" => Psk250Rc6, "psk250rc7" => Psk250Rc7,
            "psk500rc2" => Psk500Rc2, "psk500rc3" => Psk500Rc3, "psk500rc4" => Psk500Rc4,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        use PskVariant::*;
        match self {
            Psk31 => "psk31", Psk63 => "psk63", Psk125 => "psk125", Psk250 => "psk250",
            Psk500 => "psk500", Psk1000 => "psk1000",
            Psk63F => "psk63f", Psk125F => "psk125f", Psk250F => "psk250f", Psk500F => "psk500f",
            Qpsk31 => "qpsk31", Qpsk63 => "qpsk63", Qpsk125 => "qpsk125",
            Qpsk250 => "qpsk250", Qpsk500 => "qpsk500",
            Psk125R => "psk125r", Psk250R => "psk250r", Psk500R => "psk500r", Psk1000R => "psk1000r",
            Psk63Rc4 => "psk63rc4", Psk63Rc5 => "psk63rc5", Psk63Rc10 => "psk63rc10",
            Psk63Rc20 => "psk63rc20", Psk63Rc32 => "psk63rc32",
            Psk125Rc4 => "psk125rc4", Psk125Rc5 => "psk125rc5", Psk125Rc10 => "psk125rc10",
            Psk125Rc12 => "psk125rc12", Psk125Rc16 => "psk125rc16",
            Psk250Rc2 => "psk250rc2", Psk250Rc3 => "psk250rc3", Psk250Rc5 => "psk250rc5",
            Psk250Rc6 => "psk250rc6", Psk250Rc7 => "psk250rc7",
            Psk500Rc2 => "psk500rc2", Psk500Rc3 => "psk500rc3", Psk500Rc4 => "psk500rc4",
        }
    }
}

/// The plain-BPSK payload bitstream: PSK31 Varicode + `00` separators, MSB-first.
/// This is the exact bit domain fldigi's `tx_char` feeds the differential
/// encoder for the non-FEC BPSK modes. ref: psk.cxx tx_char + pskvaricode.cxx.
pub fn encode_bpsk_bits(v: PskVariant, text: &str) -> Vec<u8> {
    debug_assert!(!v.params().qpsk && !v.params().robust);
    vari_encode(&PSK31, text)
}
```

Add `pub mod psk;` to `crates/dsp/src/modes/mod.rs`. Add the `load_bits` helper to `kat.rs` (reads the 0/1 string field from a vector JSON into `Vec<u8>`).

- [ ] **Step 4: Run the test, verify pass**

Run: `cargo test -p omnimodem-dsp --features testutil psk_bpsk_varicode_matches_fldigi_vector`
Expected: PASS (bit-exact vs fldigi Varicode payload).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/src/modes/mod.rs crates/dsp/tests/kat.rs
git commit -m "feat(psk): PskVariant parameter table + BPSK varicode payload (fldigi KAT)"
```

### T3 — (no separate FEC for plain BPSK)

Plain BPSK rates carry no FEC; the FEC + interleave port is T3 of the BPSK+F family (Task 2) and QPSK (Task 3). No step here — recorded so the T-numbering stays uniform across families.

### T4 — Port the BPSK modulator (symbol phases bit-exact; audio tolerance)

- [ ] **Step 1: Write the failing test** (`crates/dsp/tests/kat.rs`)

```rust
#[test]
fn psk_bpsk_symbol_phase_matches_fldigi_and_audio_tracks() {
    use omnimodem_dsp::modes::psk::{PskMod, PskVariant};
    use omnimodem_dsp::mode::Modulator;
    use omnimodem_dsp::types::{Frame};
    // (a) bit-exact: our differential BPSK phase-index sequence == fldigi's.
    let want_ph = load_bits("psk_bpsk.json", "bpsk_symbol_phase");
    let got_ph = PskMod::new(PskVariant::Psk125, 1500.0).symbol_phases("CQ DE K1ABC");
    assert_eq!(got_ph, want_ph, "PSK125 differential symbol phases differ from fldigi");
    // (b) FP tolerance: PSK125 modulated audio correlates with fldigi's head.
    let ref_audio = load_f32("psk_bpsk.json", "psk125_audio_head");
    let mut m = PskMod::new(PskVariant::Psk125, 1500.0);
    let audio = m.modulate(&Frame::text("CQ DE K1ABC")).unwrap();
    let corr = normalized_xcorr(&audio[..ref_audio.len()], &ref_audio);
    assert!(corr > 0.95, "PSK125 audio correlation {corr} below 0.95");
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test -p omnimodem-dsp --features testutil psk_bpsk_symbol_phase_matches_fldigi_and_audio_tracks`
Expected: FAIL — `PskMod` does not exist.

- [ ] **Step 3: Implement `PskMod`** (`crates/dsp/src/modes/psk.rs`)

```rust
/// Leading idle reversals prepended so the RX Costas/timing loops lock before
/// payload (fldigi sends `dcdbits` reversals of preamble; ref: psk.cxx:2621-2628).
pub struct PskMod {
    v: PskVariant,
    center_hz: f32,
}

impl PskMod {
    pub fn new(v: PskVariant, center_hz: f32) -> Self {
        PskMod { v, center_hz }
    }

    /// The differential BPSK phase-index stream (0 = phase reversal, 1 = steady
    /// carrier) fed to the modulator — the bit-exact domain vs fldigi. Standard
    /// PSK31 rule: Varicode `0` is a reversal, `1` is steady; the line idles on
    /// continuous reversals. ref: psk31.rs (existing) + psk.cxx tx path.
    pub fn symbol_phases(&self, text: &str) -> Vec<u8> {
        let pp = self.v.params();
        let mut rev = vec![1u8; pp.dcdbits]; // preamble = dcdbits reversals
        rev.push(1);
        rev.push(1); // the 00 character-start separator, as two reversals
        rev.extend(encode_bpsk_bits(self.v, text).into_iter().map(|b| 1 ^ b));
        rev
    }
}

impl Modulator for PskMod {
    fn caps(&self) -> ModeCaps {
        let pp = self.v.params();
        ModeCaps {
            native_rate: PSK_RATE,
            bandwidth_hz: self.v.baud() * 2.0 * pp.carriers as f32,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }
    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("psk needs text")),
        };
        let pp = self.v.params();
        // Single-carrier BPSK: reuse DiffPsk (one differential layer, bps=1).
        // QPSK/robust/multi-carrier override this in Tasks 2-4.
        let rev: Vec<u32> = self.symbol_phases(text).into_iter().map(u32::from).collect();
        let psk = DiffPsk::new(PSK_RATE as f32, self.center_hz, pp.symbollen, 1);
        Ok(psk.modulate(&rev))
    }
}
```

Add the `use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator}; use crate::types::{Frame, FramePayload};` imports at the top of `psk.rs`. Add `normalized_xcorr` + `load_f32` helpers to `kat.rs`.

- [ ] **Step 4: Run the test, verify pass**

Run: `cargo test -p omnimodem-dsp --features testutil psk_bpsk_symbol_phase_matches_fldigi_and_audio_tracks`
Expected: PASS (phase indices bit-exact; audio correlation > 0.95).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/tests/kat.rs
git commit -m "feat(psk): BPSK differential modulator (fldigi symbol-phase KAT + audio tolerance)"
```

### T5 — Port the BPSK demodulator (Costas + Gardner + differential decode + Varicode)

- [ ] **Step 1: Write the failing test** (`crates/dsp/tests/loopback.rs`)

```rust
#[test]
fn psk_bpsk_rate_grid_loopback() {
    use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};
    use omnimodem_dsp::mode::{Demodulator, Modulator};
    use omnimodem_dsp::types::{Frame, FramePayload};
    let msg = "CQ DE K1ABC";
    for v in [PskVariant::Psk63, PskVariant::Psk125, PskVariant::Psk250,
              PskVariant::Psk500, PskVariant::Psk1000] {
        let audio = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let out = PskDemod::new(v, 1500.0).feed(&audio);
        let text: String = out.iter().filter_map(|f| match &f.payload {
            FramePayload::Text(t) => Some(t.clone()), _ => None }).collect();
        assert!(text.contains(msg), "{:?} loopback recovered {text:?}", v);
    }
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test -p omnimodem-dsp --features testutil psk_bpsk_rate_grid_loopback`
Expected: FAIL — `PskDemod` does not exist.

- [ ] **Step 3: Implement `PskDemod`** (`crates/dsp/src/modes/psk.rs`)

Generalize the existing `Psk31Demod` (`crates/dsp/src/modes/psk31.rs:91-249`) by symbol rate. Port verbatim, parametrizing `sps = v.samples_per_symbol()`, the Costas loop bandwidth (keep `(0.06, 0.02)` — it acquires within a few symbols at every rate), the squelch FIR cutoff scaled to `~1.3 * baud`, and the Gardner period `PSK_RATE/baud`. Keep the `synced`/`00`-boundary gate, the `MAX_PENDING_BITS` cap, and `drain_completed`'s Varicode decode exactly as in `psk31.rs`.

```rust
use crate::frontend::fir::{design_lowpass, Fir};
use crate::frontend::nco::DownConverter;
use crate::sync::costas::CostasLoop;
use crate::sync::timing::GardnerTed;
use crate::types::Cplx;

const MAX_PENDING_BITS: usize = 512;
const CARRIER_EMA: f32 = 0.002;
const CARRIER_OPEN: f32 = 0.15;
const CARRIER_FLOOR: f32 = 1e-9;

pub struct PskDemod {
    v: PskVariant,
    center_hz: f32,
    nco: DownConverter,
    costas: CostasLoop,
    gardner: GardnerTed,
    acc_i: f32,
    prev_i: f32,
    have_prev: bool,
    synced: bool,
    zrun: u8,
    lpf_i: Fir,
    lpf_q: Fir,
    p_in: f32,
    p_tot: f32,
    pending: Vec<u8>,
    sample_index: u64,
}

impl PskDemod {
    pub fn new(v: PskVariant, center_hz: f32) -> Self {
        let rate = PSK_RATE as f32;
        let baud = v.baud();
        // Squelch lowpass a little wider than the occupied band (~baud*2), so the
        // reversal idle at carrier ± baud/2 survives. ref: psk31.rs SQUELCH_CUTOFF.
        let cutoff = (baud * 1.3).max(80.0);
        let taps = design_lowpass(127, cutoff, rate);
        PskDemod {
            v, center_hz,
            nco: DownConverter::new(center_hz, rate),
            costas: CostasLoop::new(0.06, 0.02),
            gardner: GardnerTed::new(rate / baud),
            acc_i: 0.0, prev_i: 0.0, have_prev: false, synced: false, zrun: 0,
            lpf_i: Fir::new(taps.clone()), lpf_q: Fir::new(taps),
            p_in: 0.0, p_tot: 0.0, pending: Vec::new(), sample_index: 0,
        }
    }

    fn drain_completed(&mut self) -> Vec<Frame> {
        // identical to psk31.rs:153-178, decoder tag = self.v.label().
        let last_sep = (1..self.pending.len()).rev()
            .find(|&i| self.pending[i] == 0 && self.pending[i - 1] == 0);
        let Some(idx) = last_sep else {
            if self.pending.len() > MAX_PENDING_BITS { self.pending.clear(); }
            return Vec::new();
        };
        let text = vari_decode(&PSK31, &self.pending[..=idx]);
        self.pending.drain(..=idx);
        if text.is_empty() { return Vec::new(); }
        vec![Frame {
            payload: FramePayload::Text(text),
            meta: FrameMeta { crc_ok: true, sample_offset: self.sample_index,
                decoder: Some(self.v.label().into()), ..Default::default() },
        }]
    }
}

impl Demodulator for PskDemod {
    fn caps(&self) -> ModeCaps { PskMod::new(self.v, self.center_hz).caps() }
    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        // Port of psk31.rs:192-243 verbatim, using self.v params. The differential
        // rule is unchanged (reversal=0, steady=1); only sps/baud are parametric.
        for &x in samples {
            self.sample_index += 1;
            let bb = self.nco.push(x);
            let f = Cplx::new(self.lpf_i.push(bb.re), self.lpf_q.push(bb.im));
            self.p_in += CARRIER_EMA * (f.norm_sqr() - self.p_in);
            self.p_tot += CARRIER_EMA * (bb.norm_sqr() - self.p_tot);
            let derot = self.costas.process(bb);
            self.acc_i += derot.re;
            if self.gardner.feed(derot.re).is_some() {
                let sym = self.acc_i;
                self.acc_i = 0.0;
                let open = self.p_tot > CARRIER_FLOOR && self.p_in > CARRIER_OPEN * self.p_tot;
                if !open {
                    self.pending.clear(); self.have_prev = false; self.prev_i = 0.0;
                    self.synced = false; self.zrun = 0; continue;
                }
                if self.have_prev {
                    let data_bit = u8::from(sym * self.prev_i >= 0.0);
                    if self.synced { self.pending.push(data_bit); }
                    else if data_bit == 0 { self.zrun += 1; if self.zrun >= 2 { self.synced = true; } }
                    else { self.zrun = 0; }
                }
                self.prev_i = sym; self.have_prev = true;
            }
        }
        self.drain_completed()
    }
    fn reset(&mut self) { *self = PskDemod::new(self.v, self.center_hz); }
}
```

Add `use crate::types::FrameMeta;`. Then reduce `crates/dsp/src/modes/psk31.rs` to a shim:

```rust
//! Back-compat shim: PSK31 is `PskVariant::Psk31`. New code uses `modes::psk`.
pub use crate::modes::psk::PSK_RATE as PSK31_RATE;
pub const PSK31_BAUD: f32 = 31.25;

use crate::modes::psk::{PskDemod, PskMod, PskVariant};

pub struct Psk31Mod(PskMod);
impl Psk31Mod { pub fn new(center_hz: f32) -> Self { Psk31Mod(PskMod::new(PskVariant::Psk31, center_hz)) } }
// delegate Modulator to self.0 …

pub struct Psk31Demod(PskDemod);
impl Psk31Demod { pub fn new(center_hz: f32) -> Self { Psk31Demod(PskDemod::new(PskVariant::Psk31, center_hz)) } }
// delegate Demodulator to self.0 …
```

- [ ] **Step 4: Run the tests, verify pass**

Run: `cargo test -p omnimodem-dsp --features testutil psk_bpsk_rate_grid_loopback && cargo test -p omnimodem-dsp --lib modes::psk31`
Expected: PASS (grid loopback + existing psk31 tests via the shim).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/src/modes/psk31.rs crates/dsp/tests/loopback.rs
git commit -m "feat(psk): parametric BPSK demodulator + psk31 back-compat shim (rate-grid loopback)"
```

### T6 — Register the BPSK family in the daemon

- [ ] **Step 1: Write the failing test** (`crates/omnimodem/src/mode/mod.rs`, in `#[cfg(test)] mod tests`)

```rust
#[test]
fn parse_resolves_psk_family() {
    assert_eq!(ModeConfig::parse("psk125"),
        Some(ModeConfig::Psk { submode: "psk125".into(), center_hz: 1500.0 }));
    assert_eq!(ModeConfig::parse("psk250:center=1000"),
        Some(ModeConfig::Psk { submode: "psk250".into(), center_hz: 1000.0 }));
    assert_eq!(ModeConfig::parse("psk31"),
        Some(ModeConfig::Psk { submode: "psk31".into(), center_hz: 1000.0 }));
    // round-trips through to_mode_string.
    let c = ModeConfig::Psk { submode: "psk500".into(), center_hz: 1500.0 };
    assert_eq!(ModeConfig::parse(&c.to_mode_string()), Some(c));
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test -p omnimodem parse_resolves_psk_family`
Expected: FAIL — `ModeConfig::Psk` does not exist.

- [ ] **Step 3: Implement the `Psk` variant + registry arms**

In `crates/omnimodem/src/mode/mod.rs`: add the variant, parse, label, and string arms. Keep the old `Psk31 { center_hz }` variant as a parse alias that maps to `Psk { submode: "psk31", .. }` OR replace it — replace it, updating the two existing `Psk31` references in `registry.rs` and the `mod.rs` tests.

```rust
// in enum ModeConfig (replaces `Psk31 { center_hz }`):
    Psk { submode: String, center_hz: f32 },
```
```rust
// in parse(): default center 1000 (psk31) / 1500 (higher rates). Use 1500 for
// every rate for consistency with fldigi's default audio center; psk31 keeps 1000
// so existing configs/tests are unchanged.
    _ if omnimodem_dsp::modes::psk::PskVariant::from_label(mode).is_some() => {
        let default_center = if mode == "psk31" { 1000.0 } else { 1500.0 };
        Some(ModeConfig::Psk { submode: mode.to_string(), center_hz: f("center", default_center) })
    }
```
```rust
// in to_mode_string():
    ModeConfig::Psk { submode, center_hz } => format!("{submode}:center={center_hz}"),
// in label(): return a &'static str via PskVariant::label():
    ModeConfig::Psk { submode, .. } =>
        omnimodem_dsp::modes::psk::PskVariant::from_label(submode).map(|v| v.label()).unwrap_or("psk"),
```

In `crates/omnimodem/src/mode/registry.rs`, replace the two `ModeConfig::Psk31` arms:

```rust
// demod_kind:
    ModeConfig::Psk { submode, center_hz } => {
        let v = PskVariant::from_label(submode).expect("validated by parse");
        DemodKind::Streaming(Box::new(PskDemod::new(v, *center_hz)))
    }
// build_modulator:
    ModeConfig::Psk { submode, center_hz } => {
        let v = PskVariant::from_label(submode).expect("validated by parse");
        Some(Box::new(PskMod::new(v, *center_hz)))
    }
```

Update the `use omnimodem_dsp::modes::psk31::{...}` import to `use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};`.

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test -p omnimodem mode::`
Expected: PASS (new parse test + all existing registry/mode tests).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/mode/mod.rs crates/omnimodem/src/mode/registry.rs
git commit -m "feat(daemon): register parametric PSK family (Psk ModeConfig variant)"
```

### T7 — Conformance gates (BPSK BER sweep + cross-decode gate)

- [ ] **Step 1: Write the failing test** (`crates/dsp/tests/ber.rs`)

```rust
#[test]
fn psk_bpsk_decode_rate_grid() {
    use omnimodem_dsp::modes::psk::{PskDemod, PskMod, PskVariant};
    let msg = "CQ DE K1ABC";
    // Faster rates tolerate more absolute noise (wider bandwidth => shorter
    // symbols but same Eb/N0 target); floors set just under observed.
    for (v, sigma, floor) in [
        (PskVariant::Psk63, 0.02f32, 0.9f32),
        (PskVariant::Psk125, 0.03, 0.9),
        (PskVariant::Psk250, 0.04, 0.85),
        (PskVariant::Psk500, 0.05, 0.85),
    ] {
        let rate = decode_rate(20, |seed| {
            let mut s = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
            let mut rng = Rng::new(700 + seed as u64);
            add_awgn(&mut s, sigma, &mut rng);
            has_text(&PskDemod::new(v, 1500.0).feed(&s), msg)
        });
        eprintln!("{:?} decode rate @ sigma={sigma}: {rate}", v);
        assert!(rate >= floor, "{:?} decode rate {rate} below floor {floor}", v);
    }
}
```

- [ ] **Step 2: Run it, verify it fails then passes after tuning**

Run: `cargo test -p omnimodem-dsp --features testutil psk_bpsk_decode_rate_grid`
Expected: initially may FAIL if a floor is set above observed; record the measured rate from the `eprintln!` and set each floor just under it (per the `ber.rs` convention). Re-run → PASS.

- [ ] **Step 3: Add the `#[ignore]` bidirectional cross-decode gate** (`crates/dsp/tests/kat.rs`)

```rust
#[test]
#[ignore = "requires fldigi (Phase-7 PSK interop gate)"]
fn psk_bpsk_cross_decode_doc() {
    // ours→ref:  write PskMod(Psk125,1500) audio to a .wav; fldigi PSK125 must
    //            decode "CQ DE K1ABC".
    // ref→ours:  fldigi transmits PSK125 → PskDemod recovers the text.
    // Rates 63/250/500/1000 identically. Vector: tests/vectors/psk_bpsk.json.
}
```

- [ ] **Step 4: Add a snapshot + verify** (`crates/dsp/tests/snapshots.rs`): snapshot `PskMod::new(PskVariant::Psk125, 1500.0)` output head to `psk125.snap`.

Run: `cargo test -p omnimodem-dsp --features testutil psk_bpsk_decode_rate_grid` and `cargo insta test` for the snapshot.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/tests/ber.rs crates/dsp/tests/kat.rs crates/dsp/tests/snapshots.rs crates/dsp/tests/vectors/psk125.snap
git commit -m "test(psk): BPSK rate-grid BER sweep + cross-decode gate + snapshot"
```

### T8 — Wire the BPSK family into the TUI

- [ ] **Step 1: Write the failing Go test** (`clients/omnimodem-tui/internal/app/modes_test.go` — extend or create)

```go
func TestPskFamilyModesPresent(t *testing.T) {
    for _, label := range []string{"psk63", "psk125", "psk250", "psk500", "psk1000"} {
        if modeByLabel(label) == nil {
            t.Fatalf("mode %q missing from operate list", label)
        }
        p := modeParamsFor(label, map[string]float64{"center": 1500})
        if p.GetPsk() == nil || p.GetPsk().CenterHz != 1500 {
            t.Fatalf("mode %q: psk params not wired", label)
        }
    }
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cd clients/omnimodem-tui && go test ./internal/app/ -run TestPskFamilyModesPresent`
Expected: FAIL — labels absent from `modes`; `GetPsk` undefined (proto not regenerated).

- [ ] **Step 3: Add proto `PskParams`, regenerate, add rows + arm**

In `proto/omnimodem.proto`, extend the oneof and add the message:

```proto
  oneof params {
    CwParams cw = 1;
    RttyParams rtty = 2;
    Psk31Params psk31 = 3;
    OliviaParams olivia = 4;
    Afsk1200Params afsk1200 = 5;
    PskParams psk = 6;
  }
```
```proto
message PskParams {
  string submode = 1;   // "psk63", "qpsk125", "psk250r", "psk63rc4", …
  float  center_hz = 2; // audio center frequency
}
```

Regenerate both sides: run `clients/omnimodem-tui/gen.sh` (Go) and rebuild the Rust prost bindings (the daemon's `build.rs` regenerates on `cargo build`). Then in `clients/omnimodem-tui/internal/app/modes.go`, add BPSK rows and a `psk` arm:

```go
// in the modes slice (after the psk31 row):
    {"psk63", "chat", 0, []modeParam{{"center", 1500}}},
    {"psk125", "chat", 0, []modeParam{{"center", 1500}}},
    {"psk250", "chat", 0, []modeParam{{"center", 1500}}},
    {"psk500", "chat", 0, []modeParam{{"center", 1500}}},
    {"psk1000", "chat", 0, []modeParam{{"center", 1500}}},
```
```go
// in modeParamsFor(), a single arm covering every psk* / qpsk* submode:
    case "psk63", "psk125", "psk250", "psk500", "psk1000",
        "psk63f", "psk125f", "psk250f", "psk500f",
        "qpsk31", "qpsk63", "qpsk125", "qpsk250", "qpsk500",
        "psk125r", "psk250r", "psk500r", "psk1000r",
        "psk63rc4", "psk63rc5", "psk63rc10", "psk63rc20", "psk63rc32",
        "psk125rc4", "psk125rc5", "psk125rc10", "psk125rc12", "psk125rc16",
        "psk250rc2", "psk250rc3", "psk250rc5", "psk250rc6", "psk250rc7",
        "psk500rc2", "psk500rc3", "psk500rc4":
        return &pb.ModeParams{Params: &pb.ModeParams_Psk{Psk: &pb.PskParams{
            Submode: label, CenterHz: float32(get("center", 1500)),
        }}}
```

The daemon's `ConfigureChannel` handler that maps `ModeParams` → mode string must gain a `PskParams` arm producing `"<submode>:center=<hz>"` — locate it (grep `Psk31Params` in `crates/omnimodem/src`) and add the parallel `PskParams` case.

- [ ] **Step 4: Run tests, verify pass**

Run: `cd clients/omnimodem-tui && go test ./...` and `cargo test -p omnimodem`
Expected: PASS (Go proto arm resolves; daemon param mapping test green).

- [ ] **Step 5: Commit**

```bash
git add proto/omnimodem.proto clients/omnimodem-tui/internal/app/modes.go clients/omnimodem-tui/internal/app/modes_test.go clients/omnimodem-tui/internal/pb crates/omnimodem/src
git commit -m "feat(tui): PSK BPSK-rate modes selectable + PskParams proto"
```

### T9 — Deferred to the phase PR (see the final PR task)

The per-phase PR is opened once (after the last family lands), covering Tasks 1–4. No separate PR per family — recorded so the T-numbering stays uniform.

---

# Task 2 — BPSK + FEC family (PSK63F/125F/250F/500F) — T1–T8

Adds a K=7 convolutional FEC layer (reusing `fec::conv`) over the BPSK payload, with interleave at ≥125 baud (none at 63F). Builds on the `PskVariant` table and the BPSK modulator/demodulator from Task 1.

**Files:**
- Modify: `crates/dsp/src/modes/psk.rs`, `crates/dsp/tests/{kat.rs,loopback.rs,ber.rs}`, `clients/omnimodem-tui/internal/app/modes.go`
- Create: (vectors) `crates/dsp/tests/vectors/psk_pskr.json` covers +F and robust; author its +F record here.

### T1 — Extend the golden vectors for +F

- [ ] **Step 1:** Extend `scratch/refvectors/psk_dump.cxx` to also dump, for `PSK125F`: the MFSK-Varicode-or-PSK31-Varicode payload bits fldigi actually emits (record which — this is the codec-choice decision point), the K=7 FEC codeword bits (before interleave), and the interleaved symbol stream (depth 40). For `PSK63F` dump the codeword with **no** interleave.
- [ ] **Step 2:** Author `crates/dsp/tests/vectors/psk_pskr.json` with `_meta` provenance + records `psk63f` and `psk125f` carrying `payload_bits`, `fec_codeword`, `interleaved` (empty for 63f). State in `_meta.notes` the observed varicode table.
- [ ] **Step 3: Commit**

```bash
git add scratch/refvectors/psk_dump.cxx crates/dsp/tests/vectors/psk_pskr.json
git commit -m "test(psk): fldigi +F/robust FEC golden vectors"
```

### T2 — Confirm/port the +F source codec

- [ ] **Step 1: Write the failing test** (`crates/dsp/tests/kat.rs`) asserting `encode_pskr_payload(PskVariant::Psk125F, "CQ")` equals the vector's `payload_bits`. If the vector shows MFSK Varicode, `encode_pskr_payload` transcribes that table from `fldigi/src/mfsk/mfskvaricode.cxx` (bit-exact, `// ref:` cited); if PSK31 Varicode, it reuses `PSK31`. The test pins the choice against the vector — no hand-waving.
- [ ] **Step 2: Run it** → FAIL (`encode_pskr_payload` missing).
- [ ] **Step 3: Implement `encode_pskr_payload`** in `psk.rs` per the vector-confirmed table. If MFSK Varicode is needed, add a `const fn build_mfsk_varicode() -> [&'static str; 256]` transcribing `mfskvaricode.cxx`'s table verbatim with the `// ref:` cite (transcribe the full 256-entry table — the reference lines to copy in full are `mfskvaricode.cxx:<start>-<end>`; the first few entries are the control codes then the same prefix-free structure as PSK31).
- [ ] **Step 4: Run it** → PASS (bit-exact vs fldigi).
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/tests/kat.rs
git commit -m "feat(psk): +F payload codec (fldigi KAT-pinned varicode choice)"
```

### T3 — Port the +F FEC + interleave (bit-exact)

- [ ] **Step 1: Write the failing test** (`crates/dsp/tests/kat.rs`)

```rust
#[test]
fn psk_pskr_fec_and_interleave_match_fldigi() {
    use omnimodem_dsp::modes::psk::{encode_pskr_tx_bits, PskVariant};
    // 63F: FEC only, no interleave. 125F: FEC then depth-40 interleave.
    let want63 = load_bits("psk_pskr.json", "psk63f.fec_codeword");
    assert_eq!(encode_pskr_tx_bits(PskVariant::Psk63F, "CQ"), want63,
        "PSK63F FEC codeword differs from fldigi");
    let want125 = load_bits("psk_pskr.json", "psk125f.interleaved");
    assert_eq!(encode_pskr_tx_bits(PskVariant::Psk125F, "CQ"), want125,
        "PSK125F interleaved bits differ from fldigi");
}
```

- [ ] **Step 2: Run it** → FAIL (`encode_pskr_tx_bits` missing).
- [ ] **Step 3: Implement `encode_pskr_tx_bits`** in `psk.rs`:

```rust
use crate::fec::interleave::{block_interleave, block_deinterleave};

/// The +F/robust TX bitstream: payload varicode → K=7 conv encode → (for
/// idepth>0) fldigi's 2×2×idepth square interleave. ref: psk.cxx:983-992
/// (encoder), 1031-1038 (interleaver). PSK63F skips interleave (psk.cxx:1234).
pub fn encode_pskr_tx_bits(v: PskVariant, text: &str) -> Vec<u8> {
    let pp = v.params();
    let code = v.conv_code().expect("robust/+F has a K=7 code");
    let payload = encode_pskr_payload(v, text);
    let coded = code.encode(&payload); // rate-1/2, zero-tail flush
    if pp.idepth == 0 {
        coded // PSK63F: no interleave
    } else {
        // fldigi interleave(isize=2, idepth): a square block interleaver of side
        // `idepth` over the code bits. Reuse block_interleave with rows=cols=idepth,
        // padding the final block with the fldigi flush bits. ref: psk.cxx:1031-1038.
        interleave_square(&coded, pp.idepth)
    }
}
```

Implement `interleave_square` matching fldigi's `interleave.cxx` fill/read order (confirm the exact order against the `psk125f.interleaved` vector — if `block_interleave`'s row-in/col-out does not match, transcribe fldigi's `interleave::symbols` index math verbatim with a `// ref: fldigi/src/psk/interleave.cxx:<lines>` cite; the vector is the arbiter).

- [ ] **Step 4: Run it** → PASS (both 63F and 125F bit-exact).
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/tests/kat.rs
git commit -m "feat(psk): +F K=7 FEC + square interleave (fldigi bit-exact KAT)"
```

### T4/T5 — +F modulator + demodulator

- [ ] **Step 1: Write the failing loopback test** (`crates/dsp/tests/loopback.rs`) over `[Psk63F, Psk125F, Psk250F, Psk500F]`: modulate `"CQ DE K1ABC"`, feed the demod, assert recovery (mirrors `psk_bpsk_rate_grid_loopback`).
- [ ] **Step 2: Run it** → FAIL (`PskMod`/`PskDemod` don't branch on FEC yet).
- [ ] **Step 3: Implement the +F branch.** In `PskMod::modulate`, when `pp.robust`: use `encode_pskr_tx_bits` → map each coded bit to a BPSK reversal phase (`1 ^ bit`) with the PSK-R preamble (`dcdbits/2` alternating `1/0` pairs + a `<NUL>`, `// ref: psk.cxx:2562-2577`) → `DiffPsk(bps=1)`. In `PskDemod::feed`, when `pp.robust`: collect de-rotated soft symbols → deinterleave (`block_deinterleave`, depth `idepth`) → LLR map (`+`/`-` scaled by amplitude, `// ref: psk.cxx:1496-1520 softbit`) → `code.viterbi_decode` → Varicode decode. Add a `PskrRx` sub-state struct owned by `PskDemod` (soft-symbol ring buffer + deinterleaver) so `feed` stays allocation-free per sample.
- [ ] **Step 4: Run it** → PASS (+F grid loopback).
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/tests/loopback.rs
git commit -m "feat(psk): +F modulator + soft-Viterbi demodulator (grid loopback)"
```

### T6 — Register +F (already covered by the parametric `Psk` arm)

- [ ] **Step 1:** Extend `parse_resolves_psk_family` in `mode/mod.rs` with `psk63f`/`psk125f` assertions. The parametric arm from Task 1 T6 already resolves them (no new registry code); this test confirms it.
- [ ] **Step 2: Run** `cargo test -p omnimodem parse_resolves_psk_family` → PASS.
- [ ] **Step 3: Commit**

```bash
git add crates/omnimodem/src/mode/mod.rs
git commit -m "test(daemon): confirm +F submodes resolve via the Psk arm"
```

### T7 — +F BER sweep + cross-decode gate

- [ ] **Step 1: Write** `psk_pskr_decode_rate_grid` in `ber.rs` over `[Psk63F, Psk125F, Psk250F]` — the FEC should let these decode at a **higher** sigma than plain BPSK at the same baud (assert the floor and note the FEC coding gain).
- [ ] **Step 2: Run**, record rates, set floors just under observed → PASS.
- [ ] **Step 3: Add** `#[ignore] fn psk_pskr_cross_decode_doc()` to `kat.rs` documenting fldigi PSK125F both-direction verification.
- [ ] **Step 4: Run** the BER test → PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/tests/ber.rs crates/dsp/tests/kat.rs
git commit -m "test(psk): +F BER sweep (FEC gain) + cross-decode gate"
```

### T8 — +F in the TUI

- [ ] **Step 1: Extend** `TestPskFamilyModesPresent` (or add `TestPskFecModesPresent`) for `psk63f/psk125f/psk250f/psk500f`.
- [ ] **Step 2: Run** → FAIL (rows absent).
- [ ] **Step 3: Add** the four `+F` rows to the `modes` slice in `modes.go` (`{"psk125f", "chat", 0, []modeParam{{"center", 1500}}}`, etc.). The `modeParamsFor` `psk` arm already lists them.
- [ ] **Step 4: Run** `go test ./...` → PASS.
- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/modes.go clients/omnimodem-tui/internal/app/modes_test.go
git commit -m "feat(tui): +F PSK modes selectable"
```

---

# Task 3 — QPSK family (QPSK31/63/125/250/500) — T1–T8

Differential QPSK + K=5 (POLY 0x17/0x19) convolutional FEC + soft Viterbi. The key porting facts are the dibit→phase-quadrant map and the `(4 - sym) & 3` TX reversal handling.

**Files:**
- Modify: `crates/dsp/src/modes/psk.rs`, `crates/dsp/tests/{kat.rs,loopback.rs,ber.rs}`, `clients/omnimodem-tui/internal/app/modes.go`
- Create: `crates/dsp/tests/vectors/psk_qpsk.json`

### T1 — Extract QPSK golden vectors

- [ ] **Step 1:** Extend `scratch/refvectors/psk_dump.cxx` to dump, for `QPSK63` + message `"CQ"`: the varicode payload bits, the K=5 FEC codeword (`encoder(5,0x17,0x19)`), and the differential QPSK symbol phase-quadrant index sequence (`0..3`) fed to `tx_symbol` (including the `(4-sym)&3` mapping, `// ref: psk.cxx:2266`).
- [ ] **Step 2:** Author `crates/dsp/tests/vectors/psk_qpsk.json` with `_meta` provenance + a `qpsk63` record (`payload_bits`, `fec_codeword`, `qpsk_symbol_quadrant`).
- [ ] **Step 3: Commit**

```bash
git add scratch/refvectors/psk_dump.cxx crates/dsp/tests/vectors/psk_qpsk.json
git commit -m "test(psk): fldigi QPSK golden vectors"
```

### T2 — QPSK source codec (reuse PSK31 Varicode)

- [ ] **Step 1: Write the failing test** in `kat.rs` asserting the QPSK payload bits (PSK31 Varicode; QPSK uses the same char codec as BPSK, `// ref: psk.cxx tx_char`) equal `qpsk63.payload_bits`.
- [ ] **Step 2: Run** → FAIL until the QPSK encode entry point exists.
- [ ] **Step 3: Implement** `encode_qpsk_payload(v, text) -> Vec<u8>` = `vari_encode(&PSK31, text)` with a `debug_assert!(v.params().qpsk)`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/tests/kat.rs
git commit -m "feat(psk): QPSK payload codec (fldigi KAT)"
```

### T3 — QPSK FEC + differential-quadrant map (bit-exact)

- [ ] **Step 1: Write the failing test** (`crates/dsp/tests/kat.rs`)

```rust
#[test]
fn psk_qpsk_fec_and_symbols_match_fldigi() {
    use omnimodem_dsp::modes::psk::{qpsk_fec_bits, qpsk_symbol_quadrants, PskVariant};
    let want_cw = load_bits("psk_qpsk.json", "qpsk63.fec_codeword");
    assert_eq!(qpsk_fec_bits(PskVariant::Qpsk63, "CQ"), want_cw,
        "QPSK63 K=5 FEC codeword differs from fldigi");
    let want_q = load_bits("psk_qpsk.json", "qpsk63.qpsk_symbol_quadrant");
    assert_eq!(qpsk_symbol_quadrants(PskVariant::Qpsk63, "CQ"), want_q,
        "QPSK63 differential quadrant indices differ from fldigi");
}
```

- [ ] **Step 2: Run** → FAIL (`qpsk_fec_bits`/`qpsk_symbol_quadrants` missing).
- [ ] **Step 3: Implement** in `psk.rs`:

```rust
/// QPSK TX FEC: K=5 (0x17,0x19) conv encode of the varicode payload. No
/// interleave (QPSK modes have idepth 0). ref: psk.cxx:66-68, 979-981.
pub fn qpsk_fec_bits(v: PskVariant, text: &str) -> Vec<u8> {
    let code = ConvCode { k: 5, polys: vec![0x17, 0x19] };
    code.encode(&encode_qpsk_payload(v, text))
}

/// Differential QPSK symbol quadrants (0..3) fed to the modulator. fldigi packs
/// the K=5 encoder's two output bits per step into a dibit `sym`, applies the
/// non-reversed mapping `sym = (4 - sym) & 3`, then differentially accumulates
/// the phase quadrant. ref: psk.cxx:1188-1214 (rx_qpsk mirror), 2258-2268 (tx).
pub fn qpsk_symbol_quadrants(v: PskVariant, text: &str) -> Vec<u8> {
    let coded = qpsk_fec_bits(v, text); // 2 bits per trellis step
    let mut acc = 0u8; // differential phase accumulator (mod 4)
    let mut out = Vec::with_capacity(coded.len() / 2);
    for pair in coded.chunks_exact(2) {
        // fldigi feeds bits MSB-first into the dibit. ref: psk.cxx:2258.
        let mut sym = (pair[0] << 1) | pair[1];
        sym = (4 - sym) & 3; // non-reversed QPSK (reverse=false). ref: psk.cxx:2266
        acc = (acc + sym) & 3; // differential accumulate
        out.push(acc);
    }
    out
}
```

Confirm the dibit bit-order (`pair[0]<<1 | pair[1]` vs the reverse) and the `(4-sym)&3` placement against `qpsk63.qpsk_symbol_quadrant` — the vector is the arbiter; flip the packing if it disagrees.

- [ ] **Step 4: Run** → PASS (FEC + quadrants bit-exact).
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/tests/kat.rs
git commit -m "feat(psk): QPSK K=5 FEC + differential quadrant map (fldigi bit-exact)"
```

### T4/T5 — QPSK modulator + demodulator (soft Viterbi)

- [ ] **Step 1: Write the failing loopback test** (`loopback.rs`) over `[Qpsk31, Qpsk63, Qpsk125, Qpsk250, Qpsk500]`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** `PskMod::modulate` QPSK branch: `qpsk_symbol_quadrants` → `DiffPsk::new(rate, center, sps, bps=2).modulate(quadrants)` — but note `DiffPsk::diff_encode` also accumulates; since `qpsk_symbol_quadrants` already returns *absolute* quadrants, feed a per-symbol delta OR add a `PskMod` path that renders absolute phasors directly (mirror `multicarrier::modulate_symbols` for N=1). Choose the direct-phasor path so the differential accumulation is done once (in `qpsk_symbol_quadrants`), matching fldigi. `PskDemod::feed` QPSK branch: per symbol `phase = arg(conj(prev)*sym); bits = ((phase/(π/2)+0.5) as i32) & 3` (`// ref: psk.cxx:1500-1503`), split to two soft LLRs (`sym[0]`/`sym[1]` per `rx_qpsk`, `// ref: psk.cxx:1195-1196`), buffer, `code.viterbi_decode`, Varicode decode. Add a `QpskRx` sub-state (prev phasor + LLR buffer) owned by `PskDemod`.
- [ ] **Step 4: Run** → PASS (QPSK grid loopback).
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/tests/loopback.rs
git commit -m "feat(psk): differential QPSK modulator + soft-Viterbi demod (grid loopback)"
```

### T6 — Register QPSK (parametric arm)

- [ ] **Step 1:** Extend `parse_resolves_psk_family` with `qpsk31`/`qpsk250` assertions (resolved by the Task-1 arm). **Step 2:** Run → PASS. **Step 3: Commit**

```bash
git add crates/omnimodem/src/mode/mod.rs
git commit -m "test(daemon): confirm QPSK submodes resolve via the Psk arm"
```

### T7 — QPSK BER + cross-decode gate

- [ ] **Step 1: Write** `psk_qpsk_decode_rate_grid` in `ber.rs` over `[Qpsk63, Qpsk125, Qpsk250]`. **Step 2:** Run, set floors under observed. **Step 3: Add** `#[ignore] fn psk_qpsk_cross_decode_doc()` in `kat.rs`. **Step 4:** Run → PASS. **Step 5: Commit**

```bash
git add crates/dsp/tests/ber.rs crates/dsp/tests/kat.rs
git commit -m "test(psk): QPSK BER sweep + cross-decode gate"
```

### T8 — QPSK in the TUI

- [ ] **Step 1:** Extend the Go test for `qpsk31/63/125/250/500`. **Step 2:** Run → FAIL. **Step 3: Add** the five QPSK rows to `modes.go` (`chat`, `center` param). **Step 4:** `go test ./...` → PASS. **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/modes.go clients/omnimodem-tui/internal/app/modes_test.go
git commit -m "feat(tui): QPSK modes selectable"
```

---

# Task 4 — PSK-R robust + multi-carrier grid (`nX_PSK*R`) — T1–T8

The robust modes are PSK-R (K=7 FEC + interleave) run over N parallel carriers via `frontend::multicarrier` (Task 0). Single-carrier PSK-R (Psk125R/250R/500R/1000R) is the N=1 case and is exercised alongside the multi-carrier grid.

**Files:**
- Modify: `crates/dsp/src/modes/psk.rs`, `crates/dsp/tests/{kat.rs,loopback.rs,ber.rs}`, `crates/dsp/tests/vectors/psk_pskr.json`, `clients/omnimodem-tui/internal/app/modes.go`

### T1 — Extend robust/multi-carrier golden vectors

- [ ] **Step 1:** Extend `scratch/refvectors/psk_dump.cxx` to dump, for `Psk250R` (N=1) and `Psk125Rc4` (N=4): the FEC codeword, the interleaved stream, and the **per-carrier** symbol distribution (which coded bit-pair goes to which carrier — `tx_symbol`/`tx_carriers` round-robin, `// ref: psk.cxx:2313-2318`), plus the carrier center frequencies (to cross-check `MultiCarrier`).
- [ ] **Step 2:** Add `psk250r` and `psk125rc4` records to `crates/dsp/tests/vectors/psk_pskr.json` (`fec_codeword`, `interleaved`, `carrier_symbols` as N sub-arrays, `carrier_hz`).
- [ ] **Step 3: Commit**

```bash
git add scratch/refvectors/psk_dump.cxx crates/dsp/tests/vectors/psk_pskr.json
git commit -m "test(psk): fldigi PSK-R + multi-carrier golden vectors"
```

### T2/T3 — Carrier distribution + interleave (bit-exact)

- [ ] **Step 1: Write the failing test** (`kat.rs`)

```rust
#[test]
fn psk_multicarrier_symbol_distribution_matches_fldigi() {
    use omnimodem_dsp::modes::psk::{pskr_carrier_symbols, PskVariant};
    // psk125rc4: FEC+interleave, then round-robin across 4 carriers.
    let got = pskr_carrier_symbols(PskVariant::Psk125Rc4, "CQ"); // Vec<Vec<u8>>, one per carrier
    let want = load_carrier_bits("psk_pskr.json", "psk125rc4.carrier_symbols");
    assert_eq!(got, want, "PSK125RC4 per-carrier symbol distribution differs from fldigi");
}
```

- [ ] **Step 2: Run** → FAIL (`pskr_carrier_symbols` missing).
- [ ] **Step 3: Implement** `pskr_carrier_symbols(v, text) -> Vec<Vec<u8>>`: `encode_pskr_tx_bits` (FEC + interleave, Task 2) → BPSK reversal symbols → round-robin deal across `carriers` (`sym[k]` → carrier `k % carriers`, `// ref: psk.cxx:2313-2318`). Confirm the deal order against `psk125rc4.carrier_symbols`.
- [ ] **Step 4: Run** → PASS. Also assert `MultiCarrier::carrier_hz` matches `psk125rc4.carrier_hz` (reuses the Task-0 block; ties the block's KAT to a real mode).
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/tests/kat.rs
git commit -m "feat(psk): multi-carrier symbol distribution (fldigi bit-exact KAT)"
```

### T4/T5 — Robust modulator + demodulator (over MultiCarrier)

- [ ] **Step 1: Write the failing loopback test** (`loopback.rs`) over `[Psk250R, Psk500R, Psk125Rc4, Psk250Rc2]` (N=1 and multi-carrier): modulate, feed the demod, assert recovery. Multi-carrier at high SNR; single-carrier robust at moderate SNR.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** `PskMod::modulate` robust branch: `pskr_carrier_symbols` → map each carrier's reversal stream to phasors → `MultiCarrier::modulate_symbols`. `PskDemod` robust branch: `MultiCarrier::demodulate` → per-carrier differential soft-demap → **re-interleave carriers back to the coded stream** (inverse of the round-robin deal) → deinterleave (depth `idepth`) → `viterbi_decode` → Varicode. Store a `MultiCarrier` + per-carrier `CostasLoop`/`GardnerTed` in a `RobustRx` sub-state owned by `PskDemod`. For N=1 this reduces to the +F path from Task 2 (assert both routes agree in a unit test).
- [ ] **Step 4: Run** → PASS (robust + multi-carrier grid loopback).
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/dsp/tests/loopback.rs
git commit -m "feat(psk): PSK-R + multi-carrier modulator/demodulator over MultiCarrier (loopback)"
```

### T6 — Register the robust/multi-carrier grid + parametric grid test

- [ ] **Step 1: Write the failing test** (`crates/dsp/src/modes/psk.rs` inline) — the **parametric grid table-test** the doctrine requires: enumerate every `PskVariant`, assert `params()` (symbollen/baud/qpsk/fec/idepth/carriers) matches an inline expected table transcribed from `psk.cxx`, and that `from_label(v.label()) == Some(v)` round-trips for all.

```rust
#[test]
fn psk_variant_grid_matches_fldigi_param_table() {
    use PskVariant::*;
    // (variant, symbollen, baud, qpsk, robust, idepth, carriers). ref: psk.cxx:381-901.
    let table = [
        (Psk63, 128, 62.5, false, false, 0, 1),
        (Psk125R, 64, 125.0, false, true, 40, 1),
        (Qpsk250, 32, 250.0, true, false, 0, 1),
        (Psk63Rc4, 128, 62.5, false, true, 80, 4),
        (Psk125Rc4, 64, 125.0, false, true, 40, 4),
        (Psk250Rc2, 32, 250.0, false, true, 80, 2),
        (Psk500Rc4, 16, 500.0, false, true, 160, 4),
        // … one row per variant; full grid.
    ];
    for (v, sl, baud, qpsk, robust, idepth, carriers) in table {
        let p = v.params();
        assert_eq!(p.symbollen, sl, "{v:?} symbollen");
        assert!((v.baud() - baud).abs() < 1e-3, "{v:?} baud");
        assert_eq!(p.qpsk, qpsk, "{v:?} qpsk");
        assert_eq!(p.robust, robust, "{v:?} robust");
        assert_eq!(p.idepth, idepth, "{v:?} idepth");
        assert_eq!(p.carriers, carriers, "{v:?} carriers");
        assert_eq!(PskVariant::from_label(v.label()), Some(v), "{v:?} label round-trip");
    }
}
```

- [ ] **Step 2: Run** → FAIL if any table row disagrees with `params()`; **reconcile against `psk.cxx` (the reference wins)** — fix `params()`, not the test, if the code was wrong.
- [ ] **Step 3:** Extend `parse_resolves_psk_family` (daemon) with `psk250r`/`psk125rc4` assertions (parametric arm resolves them).
- [ ] **Step 4: Run** `cargo test -p omnimodem-dsp --lib modes::psk` and `cargo test -p omnimodem parse_resolves_psk_family` → PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/psk.rs crates/omnimodem/src/mode/mod.rs
git commit -m "test(psk): full PskVariant grid param table-test vs fldigi"
```

### T7 — Robust BER (AWGN + Watterson) + cross-decode gate

- [ ] **Step 1: Write** `psk_robust_decode_rate` in `ber.rs` over `[Psk250R, Psk125Rc4]` on AWGN, and one Watterson-fading case (`WattersonChannel::ccir_good(8000.0)`) on `Psk250R` — the robust FEC + interleave is exactly what fading targets, so this makes the fading fixture a real gate (mirrors `afsk1200_decode_rate_watterson_fading`).
- [ ] **Step 2:** Run, record rates, set floors under observed → PASS.
- [ ] **Step 3: Add** `#[ignore] fn psk_robust_cross_decode_doc()` in `kat.rs` (fldigi PSK250R + PSK125RC4 both directions).
- [ ] **Step 4:** Run the BER tests → PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/dsp/tests/ber.rs crates/dsp/tests/kat.rs
git commit -m "test(psk): PSK-R AWGN + Watterson BER sweep + cross-decode gate"
```

### T8 — Robust/multi-carrier grid in the TUI

- [ ] **Step 1:** Extend the Go test for the robust + `nX` labels (`psk125r`, `psk250r`, `psk500r`, `psk1000r`, `psk63rc4`, `psk125rc4`, `psk250rc2`, …).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3: Add** the robust + multi-carrier rows to `modes.go`. To keep the operate list navigable, the `nX` grid is exposed but the row `label` uses the fldigi name; the `modeParamsFor` `psk` arm already lists every submode. (Optional: add a submode note; keep shape `chat`.)
- [ ] **Step 4:** `go test ./...` → PASS.
- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/modes.go clients/omnimodem-tui/internal/app/modes_test.go
git commit -m "feat(tui): PSK-R + multi-carrier modes selectable"
```

---

# Task 5 (T9) — Close Phase 7 with a PR

Per the master plan "Per-phase PR (delivery unit)": one branch `feature/omnimodem-phase7-psk-family`, all Task 0–4 commits on it, one PR.

**Files:** none (integration + PR only).

- [ ] **Step 1: Full workspace build + test**

Run:
```bash
cargo build --workspace
cargo test -p omnimodem-dsp --features testutil
cargo test -p omnimodem
cd clients/omnimodem-tui && go test ./... && cd -
```
Expected: all green. Every new mode loopbacks; every KAT bit-domain gate is bit-exact; BER floors met; TUI tests pass.

- [ ] **Step 2: Confirm the exit-criterion aggregate.** Add a `phase7_exit_criterion` aggregate in `kat.rs` (compile-checked manifest, mirroring `phase4_exit_criterion`) that calls each contract-critical PSK KAT (`psk_bpsk_varicode_matches_fldigi_vector`, `psk_bpsk_symbol_phase_matches_fldigi_and_audio_tracks`, `psk_pskr_fec_and_interleave_match_fldigi`, `psk_qpsk_fec_and_symbols_match_fldigi`, `psk_multicarrier_symbol_distribution_matches_fldigi`, `psk_variant_grid_matches_fldigi_param_table`). Run it → PASS. Commit.

```bash
git add crates/dsp/tests/kat.rs
git commit -m "test(psk): phase7_exit_criterion aggregate gate"
```

- [ ] **Step 3: Push the branch** (per master-plan push mechanics — the gitconfig rewrite requires the credentialed URL):

```bash
git push "https://x-access-token:$(gh auth token)@github.com/chrissnell/omnimodem.git" feature/omnimodem-phase7-psk-family
```

- [ ] **Step 4: Open the PR**

```bash
gh pr create --repo chrissnell/omnimodem \
  --title "Phase 7 — PSK family (BPSK rates, +F FEC, QPSK, PSK-R + multi-carrier)" \
  --body "Ports fldigi src/psk/ PSK family. Modes: PSK63/125/250/500/1000, PSK63F/125F/250F/500F, QPSK31/63/125/250/500, PSK-R (125R/250R/500R/1000R) + the nX multi-carrier grid. New block: frontend/multicarrier.rs. Reference commit + per-stage golden vectors in tests/vectors/psk_*.json. KAT (varicode/FEC/interleave/symbol-phase bit-exact; audio FP-tolerance), rate/carrier grid loopback, AWGN+Watterson BER, and TUI selection all green. Conformance evidence: phase7_exit_criterion + ber.rs sweep output."
```

Request review. Merge only after review; the next phase branches from the merged base.

- [ ] **Step 5:** After merge, pin `pr_url` on the tracking issue metadata if it clears the bar (a future run will look it up), and delete any stale phase-7 metadata key.

---

## Self-review

**1. Spec coverage** (Phase-7 scope from the roadmap):

| Scope item | Task |
|---|---|
| `multicarrier.rs` new block, KAT-first | Task 0 |
| BPSK rates PSK63/125/250/500/1000 (re-parametrize psk31) | Task 1 |
| `+F` FEC variants (conv layer over BPSK) | Task 2 |
| QPSK31–500 (diff QPSK + K=7… — corrected to **K=5** per `psk.cxx:66-68`; QPSK uses K=5, PSK-R uses K=7) | Task 3 |
| PSK-R robust + multi-carrier `nX_PSK*R` grid | Task 4 |
| 8PSK explicitly deferred | Out of scope (noted; Phase 16) |
| T1 golden-vector extraction | Task N.T1 (each family) |
| KAT parity (bit-exact symbol/bit; FP-tolerance audio) | T2–T4 KATs + `psk_*_audio` correlation |
| loopback | T5 grid loopbacks |
| BER sweep (AWGN + Watterson) | T7 each family; Watterson in Task 4 T7 |
| daemon registry wiring | T6 (Task 1 adds the `Psk` arm; 2–4 confirm) |
| TUI T8 (modes.go + modeParamsFor + proto + Go test) | T8 each family + Task 1 proto `PskParams` |
| T9 per-phase PR | Task 5 |

**Note on the roadmap's "QPSK … K=7":** the roadmap line said "differential QPSK + Viterbi"; the master plan template said "K=7 convolutional FEC" generically. `psk.cxx:66-68` shows QPSK uses **K=5 (0x17/0x19)** and PSK-R uses **K=7 (0x6d/0x4f)**. The reference wins (Doctrine §1) — the plan uses K=5 for QPSK, K=7 for PSK-R/+F, and calls this out so the executor does not blindly wire K=7 for QPSK.

**2. Placeholder scan:** No "TBD"/"handle edge cases"/"similar to Task N". Each code step shows real Rust with `// ref:` cites. Two deliberate vector-arbitrated decisions are flagged (not hand-waved): (a) the `+F`/robust varicode table — PSK31 vs MFSK Varicode — is *pinned against the extracted golden vector* in Task 2 T2, with the MFSK transcription path and its `// ref:` cite named if the vector demands it; (b) the interleaver fill/read order and QPSK dibit packing are confirmed against their vectors, with the fldigi `interleave.cxx`/`psk.cxx` transcription as the fallback. These are the correct way to port a table whose exact bit-order can only be confirmed against the reference — the vector is committed at T1, so the decision is executable, not deferred. The full Varicode table is already ported in `framing::varicode` (transcribed verbatim); this plan reuses it rather than re-transcribing.

**3. Type consistency:** `PskVariant`/`PskParams`/`PskMod`/`PskDemod`/`encode_bpsk_bits`/`encode_pskr_tx_bits`/`qpsk_fec_bits`/`qpsk_symbol_quadrants`/`pskr_carrier_symbols` are named once and reused consistently across tasks. `ConvCode { k, polys }` matches `fec/conv.rs`. `MultiCarrier::{new,carrier_hz,modulate_symbols,demodulate}` is defined in Task 0 and used in Task 4. `ModeConfig::Psk { submode, center_hz }` is defined in Task 1 T6 and referenced by registry arms + the `PskParams { submode, center_hz }` proto message + the TUI `modeParamsFor` `psk` arm — field names align (`submode`, `center_hz`). The `Frame`/`FramePayload::Text`/`FrameMeta`/`Sample`/`Cplx`/`ModeCaps`/`Modulator`/`Demodulator` types match `crates/dsp/src/{types.rs,mode.rs}`. The `psk31.rs` shim preserves `Psk31Mod::new(f32)`/`Psk31Demod::new(f32)` so existing `kat.rs`/`ber.rs`/`registry.rs` references compile unchanged.

## Execution handoff

Plan complete and saved to `docs/plans/2026-07-02-omnimodem-phase7-psk-family.md`. Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task (Task 0, then Tasks 1–4 in T1→T8 order — Task 1 must land first since 2–4 build on its `PskVariant` table and modulator/demodulator skeleton; Task 4 depends on Task 0's `MultiCarrier` and Task 2's interleave), two-stage review between tasks, then Task 5 opens the PR.
2. **Inline Execution** — execute tasks in this session via `executing-plans`, batch with checkpoints.

Sequencing constraint: **Task 0 → Task 1 → {Task 2, Task 3} → Task 4 → Task 5.** Tasks 2 and 3 are independent of each other (both depend only on Task 1). Each task closes only when its conformance gate is green (Doctrine §6 — no stubs).
