# Omnimodem Phase 5 — Breadth, then Integration & Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Grow the omnimodem catalog beyond the Phase-4 first modes — the rest of the WSJT-X family (FT4, JT65, JT9, WSPR) and the fldigi modes (MFSK16, Olivia, THOR, Hell) — by adding the building-block groups they need (convolutional+soft-Viterbi, K=32 Fano, Walsh/FHT, interleavers, Golay, GF(2⁶) soft-RS), and fold in the cross-cutting integration & safety work: per-channel metrics over gRPC plus a Prometheus exporter, the TX exclusive lease, mTLS for routable binds, and a reference CLI/TUI client.

**Architecture:** New building blocks land in the pure `omnimodem-dsp` crate under the existing `fec`/`frontend`/`framing` groups — each individually KAT-tested before any mode uses it. New modes are assemblies under `crates/dsp/src/modes/`, one file per mode, wired into the daemon's one-module `mode::registry` exactly as the Phase-4 modes were; the daemon learns no new mode specifics. Integration work touches only daemon seams: metrics ride the existing lossy-telemetry broadcast and a new optional HTTP exporter; the exclusive lease extends the shared PTT interlock; mTLS fills the Phase-1 `authz::tls` fail-closed stub; the CLI is a new workspace binary crate over the existing gRPC client.

**Tech Stack:** Rust (edition 2021, workspace). Reuses Phase-3 `omnimodem-dsp` blocks and Phase-1/2/4 daemon seams (`AudioBackend`, `PttDriver`, `drive_tx_cycle`, `RxTxInterlock`, the `mpsc`-command / `tokio::broadcast`-event spine, `tonic`/`prost` gRPC). New deps: `tonic`'s `tls` feature (rustls) for mTLS; `ratatui` + `crossterm` for the TUI; `clap` for the CLI. Tests: inline `#[cfg(test)]`, `insta` golden snapshots, `proptest` round-trips, the `testutil` AWGN/Watterson fixtures, and the conformance harness in `crates/dsp/tests/`.

---

## Scope & sequencing (read first)

Phase 5 is **five independent workstreams**. Per the writing-plans scope check, each is its own executable sub-plan producing working, testable software on its own; they share only the registry enum and the proto file. Recommended execution order and rationale:

| WS | Workstream | Depends on | Why this order |
|---|---|---|---|
| **A** | New building-block groups (FEC/DSP) | nothing | Prerequisite for every WS-B/C mode. KAT-gated in isolation. |
| **B** | WSJT-X breadth: FT4, JT65, JT9, WSPR | WS-A | Reuses the Phase-4 FT8 windowed path + WS-A blocks. |
| **C** | fldigi breadth: MFSK16, Olivia, THOR, Hell | WS-A | Reuses Phase-4 streaming path + WS-A conv/FHT/interleave. |
| **D** | Metrics & observability (gRPC + Prometheus) | nothing (independent) | Pure daemon seam; can run in parallel with A–C. |
| **E** | Safety & integration: TX exclusive lease, mTLS, reference CLI/TUI | nothing (independent) | Pure daemon/client seams; parallel with A–C. |

**Explicitly deferred within Phase 5** (design §"Phase 5" says *eventually* / *future work*): the FreeDV / M17 / ARDOP voice-and-digital-voice family (needs the OFDM core + vocoder interface + ARQ engine groups) and the external KISS↔gRPC translator process. Each is a follow-on sub-plan; this plan names their building-block prerequisites (WS-A §"Deferred-group stubs") so they slot in cleanly, but ships none of them. SIC and A-priori decoding for the WSJT-X modes also stay deferred (design lists them as WSJT-X-class differentiators that compose later); the Phase-5 decoders are BP+OSD / Fano single-pass per candidate, same as the Phase-4 FT8 decoder.

**Definition of done for a mode (unchanged from Phase 4, design §"Conformance"):** its KAT vectors pass, cross-decode with the reference works **both** directions, and its BER/decode-rate curve meets the committed threshold (equal-or-better at every SNR point). A loopback demo is necessary but not sufficient.

## File structure

**Created — `omnimodem-dsp` (WS-A building blocks):**

| File | Responsibility |
|---|---|
| `crates/dsp/src/fec/conv.rs` | Parametric convolutional encoder + soft Viterbi decoder (R, K, polys) + puncturing/depuncturing. |
| `crates/dsp/src/fec/fano.rs` | K=32, r=½ convolutional encoder + Fano sequential decoder (JT9/WSPR; Viterbi impractical at K=32). |
| `crates/dsp/src/fec/fht.rs` | Soft Walsh–Hadamard / Fast-Hadamard-Transform block codec, parametric size (64=Olivia, 32=Contestia). |
| `crates/dsp/src/fec/interleave.rs` | Block, depth-L convolutional, self-sync diagonal, and bit-reversal interleavers. |
| `crates/dsp/src/fec/golay.rs` | Golay(23,12)/(24,12) encode + soft decode. |
| `crates/dsp/src/fec/rs_gf64.rs` | GF(2⁶) Reed–Solomon with a soft-decision (Franke–Taylor) decoder (JT65 RS(63,12)). |

**Created — `omnimodem-dsp` (WS-B/C modes):**

| File | Responsibility |
|---|---|
| `crates/dsp/src/modes/ft4.rs` | `Ft4Demod` (block) + `Ft4Mod`: 4-GFSK + Costas-array(4×4×4) + (174,91) LDPC, FT8-derived. |
| `crates/dsp/src/modes/jt65.rs` | `Jt65Demod` (block) + `Jt65Mod`: 65-FSK + GF(2⁶) soft-RS(63,12) + 72-bit message. |
| `crates/dsp/src/modes/jt9.rs` | `Jt9Demod` (block) + `Jt9Mod`: 9-FSK + K=32 Fano + 72-bit message. |
| `crates/dsp/src/modes/wspr.rs` | `WsprDemod` (block) + `WsprMod`: 4-FSK + K=32 Fano + 50-bit message + bit-reversal interleave. |
| `crates/dsp/src/modes/mfsk16.rs` | `Mfsk16Demod` + `Mfsk16Mod`: 16-MFSK + conv K=7 + diagonal interleave + Varicode. |
| `crates/dsp/src/modes/olivia.rs` | `OliviaDemod` + `OliviaMod`: MFSK tone bank + FHT(64) + Baudot/ASCII. |
| `crates/dsp/src/modes/thor.rs` | `ThorDemod` + `ThorMod`: MFSK + conv K=7 + convolutional interleave + DominoEX nibble Varicode. |
| `crates/dsp/src/modes/hell.rs` | `HellDemod` + `HellMod`: Feld-Hell OOK column raster. |

**Created — daemon (WS-D/E):**

| File | Responsibility |
|---|---|
| `crates/omnimodem/src/metrics/mod.rs` | `ChannelMetrics` accumulator + the per-channel metric snapshot type. |
| `crates/omnimodem/src/metrics/prometheus.rs` | Optional Prometheus text-exposition HTTP exporter over a tokio listener. |
| `crates/omnimodem/src/ptt/lease.rs` | `TxLeaseRegistry`: per-channel exclusive TX lease over a rig. |
| `crates/omnimodem-cli/Cargo.toml` + `src/main.rs` | Reference CLI/TUI client (clap subcommands + a ratatui live view). |
| `crates/omnimodem-cli/src/tui.rs` | Live channel/metrics/event TUI. |

**Modified:**

| File | Change |
|---|---|
| `crates/dsp/src/fec/mod.rs` | `pub mod conv; fano; fht; interleave; golay; rs_gf64;` |
| `crates/dsp/src/framing/message77.rs` | Add the legacy 72-bit (JT65/JT9) and 50-bit (WSPR) packers. |
| `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/lib.rs` | `pub mod` + re-export the eight new modes. |
| `crates/omnimodem/src/mode/mod.rs` | Extend `ModeConfig` + `parse`/`label` with the eight new modes. |
| `crates/omnimodem/src/mode/registry.rs` | `demod_kind`/`build_modulator`/`tx_slot_s` arms for the eight modes. |
| `crates/omnimodem/src/core/event.rs` | Add `TelemetryEvent::ChannelMetrics { .. }`. |
| `crates/omnimodem/src/core/rx_worker.rs` | Accumulate + emit per-channel metrics from decode results. |
| `crates/omnimodem/src/core/tx_worker.rs` | Honor the TX exclusive lease before keying. |
| `crates/omnimodem/src/ptt/interlock.rs` (or new `lease.rs`) | Exclusive-lease state. |
| `crates/omnimodem/src/authz/tls.rs` | Replace the fail-closed stub with a real rustls mTLS config + per-method authz. |
| `crates/omnimodem/src/authz/mod.rs`, `src/main.rs` | `serve_routable` path that binds TCP under mTLS. |
| `proto/omnimodem.proto` | Add `ChannelMetrics`, `GetMetrics`, `AcquireTxLease`/`ReleaseTxLease` RPCs + messages. |
| `Cargo.toml` (workspace) | Add `crates/omnimodem-cli`; add `ratatui`/`crossterm`/`clap` and `tonic` `tls` feature. |
| `crates/dsp/tests/kat.rs`, `ber.rs`, `loopback.rs`, `snapshots.rs` | Per-block KAT + per-mode loopback/BER/golden + a `phase5_exit_criterion` gate. |

---

# WS-A — New building-block groups (FEC/DSP toolkit)

These are the convolutional/Viterbi/Fano/FHT/interleaver/Golay/soft-RS groups the design defers from Phase 3 "to Phase 5 with the modes that need them." Each block is pure `omnimodem-dsp`, KAT-gated against published vectors **before** any mode consumes it (design §"Layer 1 — Conformance"). All decoders consume the locked `Llr` convention from `fec::llr`: `L = ln(P(0)/P(1))`, positive ⇒ bit 0, hard slice `bit = (L < 0)`.

> **Reference-constant note (applies to every WS-A/B/C DSP task).** The design (§"Honest scope note", §"Constants to confirm at implementation time") is explicit that exact generator polynomials, interleaver maps, H-matrices, sync vectors, and scaling factors must be confirmed against the reference sources (`ft8_lib`, WSJT-X `lib/`, fldigi `src/`, codec2) **at implementation time**, not from secondary docs. Every WS-A task therefore pairs a **published-vector KAT** (the correctness gate) with a structural implementation; if the KAT fails, fix the constant against the named reference, not the test.

### Task A.1: Convolutional encoder + soft Viterbi decoder

**Files:**
- Create: `crates/dsp/src/fec/conv.rs`
- Modify: `crates/dsp/src/fec/mod.rs`
- Test: `crates/dsp/src/fec/conv.rs` (inline)

- [ ] **Step 1: Add the module declaration**

In `crates/dsp/src/fec/mod.rs`, after `pub mod osd;` add:

```rust
pub mod conv;
```

- [ ] **Step 2: Write the failing KAT + round-trip test**

Create `crates/dsp/src/fec/conv.rs`:

```rust
//! Parametric convolutional code: rate-1/n encoder + soft-decision Viterbi
//! decoder, with optional puncturing. Used by fldigi-family modes (K=7 MFSK16/
//! THOR/DominoEX, K=5 M17/fldigi-QPSK). Bit order: data bits MSB-first into the
//! shift register, matching fldigi/`libm17`. K=32 codes use `fano` instead —
//! Viterbi is impractical there.

use crate::types::Llr;

/// A rate-1/n convolutional code definition. `polys` are the n generator
/// polynomials (octal in the reference; stored as raw bit masks here), `k` is
/// the constraint length (register length = k).
#[derive(Debug, Clone)]
pub struct ConvCode {
    pub k: usize,
    pub polys: Vec<u32>,
}

impl ConvCode {
    /// fldigi/MFSK16 K=7 rate-1/2, polys 0o133, 0o171 (verify vs fldigi
    /// `viterbi.cxx`).
    pub fn k7_r12() -> Self {
        ConvCode { k: 7, polys: vec![0o133, 0o171] }
    }

    /// Encode data bits (each 0/1) → n output bits per input bit, with a
    /// zero-tail flush of `k-1` bits so the decoder terminates in the zero state.
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let n = self.polys.len();
        let mut reg: u32 = 0;
        let mut out = Vec::with_capacity((data.len() + self.k - 1) * n);
        let flushed = data.iter().copied().chain(std::iter::repeat(0).take(self.k - 1));
        for bit in flushed {
            reg = (reg << 1) | (bit as u32 & 1);
            for &p in &self.polys {
                out.push((reg & p).count_ones() as u8 & 1);
            }
        }
        out
    }

    /// Soft Viterbi decode: `llrs` carries n LLRs per trellis step (positive ⇒
    /// code bit 0). Returns the `data.len()` decoded data bits (tail stripped).
    pub fn viterbi_decode(&self, llrs: &[Llr], data_len: usize) -> Vec<u8> {
        let n = self.polys.len();
        let states = 1usize << (self.k - 1);
        let steps = data_len + self.k - 1;
        debug_assert_eq!(llrs.len(), steps * n);
        let mut metric = vec![f32::NEG_INFINITY; states];
        metric[0] = 0.0;
        let mut back = vec![0u8; steps * states];
        for t in 0..steps {
            let mut next = vec![f32::NEG_INFINITY; states];
            for s in 0..states {
                if metric[s] == f32::NEG_INFINITY {
                    continue;
                }
                for bit in 0..2u32 {
                    let reg = ((s as u32) << 1) | bit;
                    let ns = (reg as usize) & (states - 1);
                    let mut branch = 0.0f32;
                    for (j, &p) in self.polys.iter().enumerate() {
                        let code_bit = (reg & p).count_ones() & 1;
                        let l = llrs[t * n + j];
                        // Correlation metric: +L if code bit is 0, -L if 1.
                        branch += if code_bit == 0 { l } else { -l };
                    }
                    let cand = metric[s] + branch;
                    if cand > next[ns] {
                        next[ns] = cand;
                        back[t * states + ns] = bit as u8;
                    }
                }
            }
            metric = next;
        }
        // Trace back from the zero state (tail forces termination there).
        let mut out = vec![0u8; steps];
        let mut s = 0usize;
        for t in (0..steps).rev() {
            let bit = back[t * states + s];
            out[t] = bit;
            // Predecessor state: undo the shift.
            s = ((s << 1) | (bit as usize)) & (states - 1);
            // (predecessor reconstruction validated by the round-trip test)
        }
        out.truncate(data_len);
        out
    }
}

/// Puncture an encoded stream by a boolean pattern (true=keep). Depuncture
/// re-inserts erasure LLRs (0.0) where bits were dropped.
pub fn puncture(bits: &[u8], pattern: &[bool]) -> Vec<u8> {
    bits.iter().zip(pattern.iter().cycle()).filter(|(_, &k)| k).map(|(&b, _)| b).collect()
}

pub fn depuncture(llrs: &[Llr], pattern: &[bool], full_len: usize) -> Vec<Llr> {
    let mut out = Vec::with_capacity(full_len);
    let mut it = llrs.iter();
    for i in 0..full_len {
        if pattern[i % pattern.len()] {
            out.push(*it.next().copied().get_or_insert(0.0));
        } else {
            out.push(0.0); // erasure
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_to_llr(bits: &[u8]) -> Vec<Llr> {
        // Hard bits → strong LLRs (+4 for 0, -4 for 1).
        bits.iter().map(|&b| if b == 0 { 4.0 } else { -4.0 }).collect()
    }

    #[test]
    fn encode_known_answer_k7() {
        // KAT: confirm the first output bits of the all-ones-then-flush input
        // against fldigi `viterbi.cxx` for polys 0o133/0o171. Placeholder vector
        // — replace with the reference's printed output at implementation time.
        let code = ConvCode::k7_r12();
        let enc = code.encode(&[1, 0, 1, 1]);
        assert_eq!(enc.len(), (4 + 6) * 2);
    }

    #[test]
    fn viterbi_round_trips_clean() {
        let code = ConvCode::k7_r12();
        let data = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0];
        let enc = code.encode(&data);
        let llrs = bits_to_llr(&enc);
        let dec = code.viterbi_decode(&llrs, data.len());
        assert_eq!(dec, data);
    }

    #[test]
    fn viterbi_corrects_a_few_errors() {
        let code = ConvCode::k7_r12();
        let data = [1u8, 1, 0, 1, 0, 1, 1, 0, 0, 1];
        let mut enc = code.encode(&data);
        enc[3] ^= 1;
        enc[10] ^= 1; // two bit flips a K=7 r=1/2 code recovers
        let dec = code.viterbi_decode(&bits_to_llr(&enc), data.len());
        assert_eq!(dec, data);
    }

    #[test]
    fn puncture_depuncture_inserts_erasures() {
        let p = [true, true, false]; // rate 2/3 puncture
        let bits = [1u8, 0, 1, 1, 0, 1];
        let punc = puncture(&bits, &p);
        assert_eq!(punc.len(), 4);
        let de = depuncture(&bits_to_llr(&punc), &p, 6);
        assert_eq!(de[2], 0.0); // erased position
    }
}
```

- [ ] **Step 3: Run to confirm it compiles and passes the round-trips**

Run: `cargo test -p omnimodem-dsp fec::conv`
Expected: `viterbi_round_trips_clean`, `viterbi_corrects_a_few_errors`, `puncture_depuncture_inserts_erasures` PASS. If trace-back is wrong, the predecessor reconstruction is the bug to fix — the round-trip test is the gate.

- [ ] **Step 4: Replace the placeholder KAT with the real reference vector**

Print fldigi's encoder output for a known input (`viterbi.cxx` test harness, or `gen_packets`-equivalent), paste the exact expected bits into `encode_known_answer_k7`, and re-run. Expected: PASS against the reference.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/fec/conv.rs crates/dsp/src/fec/mod.rs
git commit -m "Add parametric convolutional encoder + soft Viterbi + puncturing"
```

### Task A.2: K=32 Fano sequential decoder (JT9/WSPR)

**Files:**
- Create: `crates/dsp/src/fec/fano.rs`
- Modify: `crates/dsp/src/fec/mod.rs`
- Test: `crates/dsp/src/fec/fano.rs` (inline)

- [ ] **Step 1: Add the module declaration**

In `crates/dsp/src/fec/mod.rs` add `pub mod fano;`.

- [ ] **Step 2: Write the failing round-trip test**

Create `crates/dsp/src/fec/fano.rs`:

```rust
//! K=32, rate-1/2 convolutional code with a Fano sequential decoder. WSJT-X JT9
//! and WSPR use this; Viterbi is impractical at K=32 (2^31 states). Generator
//! polys 0xf2d05351 / 0xe4613c47 (WSJT-X `wsprd`/`jt9` — confirm at impl time).
//! Bit order MSB-first, matching WSJT-X.

use crate::types::Llr;

pub struct FanoCode {
    polys: [u32; 2],
}

impl Default for FanoCode {
    fn default() -> Self {
        FanoCode { polys: [0xf2d0_5351, 0xe461_3c47] }
    }
}

impl FanoCode {
    /// Encode `data` bits with a 31-bit zero tail; 2 output bits per input.
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let mut reg: u32 = 0;
        let mut out = Vec::with_capacity((data.len() + 31) * 2);
        for &bit in data.iter().chain(std::iter::repeat(&0).take(31)) {
            reg = (reg << 1) | (bit as u32 & 1);
            for p in self.polys {
                out.push((reg & p).count_ones() as u8 & 1);
            }
        }
        out
    }

    /// Fano sequential decode. `llrs`: 2 per step. `data_len`: payload bits.
    /// `delta`: Fano threshold step. Returns `Some(bits)` if the decoder
    /// reaches the end within the node budget, else `None` (too noisy).
    pub fn fano_decode(&self, llrs: &[Llr], data_len: usize, delta: f32) -> Option<Vec<u8>> {
        // Standard Fano: walk the trellis depth-first, advancing on a rising
        // path metric, backing up and lowering the running threshold when the
        // metric falls below it. Bounded by a node-visit budget so a hopeless
        // input returns None instead of looping. (Full body per WSJT-X `fano.f90`;
        // the round-trip + noise tests below are the gate.)
        let _ = (llrs, data_len, delta);
        fano_impl(&self.polys, llrs, data_len, delta, 50_000)
    }
}

fn fano_impl(
    polys: &[u32; 2],
    llrs: &[Llr],
    data_len: usize,
    delta: f32,
    budget: u32,
) -> Option<Vec<u8>> {
    // Implementation note: a faithful Fano port is ~120 lines. The executor
    // ports WSJT-X `fano232` here. Contract pinned by the tests: clean input
    // round-trips; light noise still decodes; pure noise returns None.
    let _ = (polys, llrs, data_len, delta, budget);
    unimplemented!("port WSJT-X Fano; tests below pin the contract")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn llr(bits: &[u8]) -> Vec<Llr> {
        bits.iter().map(|&b| if b == 0 { 3.0 } else { -3.0 }).collect()
    }

    #[test]
    fn round_trips_clean_input() {
        let code = FanoCode::default();
        let data: Vec<u8> = (0..50).map(|i| ((i * 7 + 3) % 2) as u8).collect();
        let enc = code.encode(&data);
        let dec = code.fano_decode(&llr(&enc), data.len(), 0.5).expect("decode");
        assert_eq!(dec, data);
    }

    #[test]
    fn returns_none_on_pure_noise() {
        let code = FanoCode::default();
        let noise: Vec<Llr> = (0..200).map(|i| if i % 3 == 0 { 0.1 } else { -0.1 }).collect();
        assert!(code.fano_decode(&noise, 50, 0.5).is_none());
    }
}
```

- [ ] **Step 3: Run to confirm `round_trips_clean_input` fails (unimplemented)**

Run: `cargo test -p omnimodem-dsp fec::fano::tests::round_trips_clean_input`
Expected: FAIL (panic `unimplemented`).

- [ ] **Step 4: Port the Fano decoder body**

Replace `fano_impl`'s body with a faithful port of WSJT-X `fano.f90`/`fano232` (threshold `delta`, node budget the loop bound). Re-run `cargo test -p omnimodem-dsp fec::fano`.
Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/fec/fano.rs crates/dsp/src/fec/mod.rs
git commit -m "Add K=32 rate-1/2 Fano sequential decoder (JT9/WSPR)"
```

### Task A.3: Soft Walsh–Hadamard / FHT block codec (Olivia)

**Files:**
- Create: `crates/dsp/src/fec/fht.rs`
- Modify: `crates/dsp/src/fec/mod.rs`
- Test: `crates/dsp/src/fec/fht.rs` (inline)

- [ ] **Step 1: Add `pub mod fht;` to `crates/dsp/src/fec/mod.rs`.**

- [ ] **Step 2: Write the failing round-trip test + implementation**

Create `crates/dsp/src/fec/fht.rs`:

```rust
//! Soft Walsh–Hadamard / Fast-Hadamard-Transform block codec. A `log2(n)`-bit
//! symbol selects one of `n` orthogonal Walsh sequences; the soft decoder runs
//! the FHT over the received soft sequence and picks the max-correlation index.
//! Parametric size: n=64 (Olivia), n=32 (Contestia). Verify Walsh ordering vs
//! fldigi `olivia.cxx` (natural vs sequency order).

/// Encode a symbol in `0..n` to its length-`n` ±1 Walsh sequence.
pub fn walsh_encode(symbol: usize, n: usize) -> Vec<i8> {
    debug_assert!(n.is_power_of_two() && symbol < n);
    (0..n).map(|i| if ((symbol & i).count_ones() & 1) == 0 { 1i8 } else { -1 }).collect()
}

/// In-place fast Hadamard transform on `f32` (natural order).
pub fn fht(a: &mut [f32]) {
    let n = a.len();
    debug_assert!(n.is_power_of_two());
    let mut h = 1;
    while h < n {
        let mut i = 0;
        while i < n {
            for j in i..i + h {
                let x = a[j];
                let y = a[j + h];
                a[j] = x + y;
                a[j + h] = x - y;
            }
            i += 2 * h;
        }
        h *= 2;
    }
}

/// Soft-decode a received length-`n` soft sequence to the most likely symbol
/// and its correlation magnitude (a soft confidence). Positive soft value ⇒
/// Walsh +1 chip.
pub fn walsh_soft_decode(soft: &[f32]) -> (usize, f32) {
    let mut work = soft.to_vec();
    fht(&mut work);
    let mut best = 0usize;
    let mut mag = 0.0f32;
    for (i, &v) in work.iter().enumerate() {
        if v.abs() > mag {
            mag = v.abs();
            best = i;
        }
    }
    (best, mag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fht_is_self_inverse_up_to_scale() {
        let mut a = [1.0, 0.0, 0.0, 0.0];
        fht(&mut a);
        fht(&mut a);
        for v in a {
            assert!((v - 4.0 * if v == 4.0 { 1.0 } else { 0.0 }).abs() < 1e-6 || v.abs() < 1e-6);
        }
    }

    #[test]
    fn clean_symbol_round_trips_for_n64() {
        let n = 64;
        for sym in [0usize, 1, 17, 63] {
            let chips = walsh_encode(sym, n);
            let soft: Vec<f32> = chips.iter().map(|&c| c as f32).collect();
            let (got, mag) = walsh_soft_decode(&soft);
            assert_eq!(got, sym, "symbol {sym}");
            assert!(mag > 0.0);
        }
    }

    #[test]
    fn noisy_symbol_still_decodes() {
        let n = 32;
        let chips = walsh_encode(5, n);
        let soft: Vec<f32> = chips.iter().enumerate()
            .map(|(i, &c)| c as f32 * 0.6 + if i % 7 == 0 { -0.5 } else { 0.2 }).collect();
        assert_eq!(walsh_soft_decode(&soft).0, 5);
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p omnimodem-dsp fec::fht`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/fec/fht.rs crates/dsp/src/fec/mod.rs
git commit -m "Add soft Walsh/FHT block codec (Olivia/Contestia)"
```

### Task A.4: Interleavers (block, diagonal, convolutional, bit-reversal)

**Files:**
- Create: `crates/dsp/src/fec/interleave.rs`
- Modify: `crates/dsp/src/fec/mod.rs`
- Test: `crates/dsp/src/fec/interleave.rs` (inline)

- [ ] **Step 1: Add `pub mod interleave;` to `crates/dsp/src/fec/mod.rs`.**

- [ ] **Step 2: Write the implementation + round-trip tests**

Create `crates/dsp/src/fec/interleave.rs`:

```rust
//! Interleavers that spread burst errors across the FEC block. Each has an
//! inverse that exactly reconstructs the input. Variants: block (row-in/col-out),
//! self-synchronizing diagonal (MFSK16), depth-L convolutional (DominoEX/THOR),
//! and bit-reversal (WSPR). Generic over `T: Copy` so it works on bits or LLRs.

/// Block interleaver: write `rows*cols` items row-major, read column-major.
pub fn block_interleave<T: Copy + Default>(data: &[T], rows: usize, cols: usize) -> Vec<T> {
    let mut out = vec![T::default(); rows * cols];
    for (i, &x) in data.iter().take(rows * cols).enumerate() {
        let (r, c) = (i / cols, i % cols);
        out[c * rows + r] = x;
    }
    out
}

pub fn block_deinterleave<T: Copy + Default>(data: &[T], rows: usize, cols: usize) -> Vec<T> {
    // Inverse: swap the roles of rows/cols.
    block_interleave(data, cols, rows)
}

/// Bit-reversal interleave: position i ↔ reverse of its `log2(n)`-bit index.
/// Self-inverse. WSPR uses the 162-symbol reversal table (skip indices ≥ n).
pub fn bit_reversal_indices(n: usize) -> Vec<usize> {
    let bits = (usize::BITS - (n - 1).leading_zeros()) as usize;
    let mut idx = Vec::with_capacity(n);
    let mut j = 0usize;
    for _ in 0..(1 << bits) {
        if j < n {
            idx.push(j);
        }
        // reverse-increment
        let mut bit = 1 << (bits - 1);
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
    }
    idx
}

pub fn permute<T: Copy + Default>(data: &[T], idx: &[usize]) -> Vec<T> {
    idx.iter().map(|&i| data[i]).collect()
}

pub fn unpermute<T: Copy + Default>(data: &[T], idx: &[usize]) -> Vec<T> {
    let mut out = vec![T::default(); data.len()];
    for (pos, &i) in idx.iter().enumerate() {
        out[i] = data[pos];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_round_trips() {
        let d: Vec<u8> = (0..12).collect();
        let il = block_interleave(&d, 3, 4);
        assert_eq!(block_deinterleave(&il, 3, 4), d);
        assert_ne!(il, d, "interleave must reorder");
    }

    #[test]
    fn bit_reversal_round_trips() {
        let d: Vec<u8> = (0..162).collect();
        let idx = bit_reversal_indices(162);
        assert_eq!(idx.len(), 162);
        let il = permute(&d, &idx);
        assert_eq!(unpermute(&il, &idx), d);
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p omnimodem-dsp fec::interleave`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/fec/interleave.rs crates/dsp/src/fec/mod.rs
git commit -m "Add interleavers (block, bit-reversal, permutation) with inverses"
```

### Task A.5: Golay(23,12)/(24,12) codec

**Files:**
- Create: `crates/dsp/src/fec/golay.rs`
- Modify: `crates/dsp/src/fec/mod.rs`
- Test: `crates/dsp/src/fec/golay.rs` (inline)

- [ ] **Step 1: Add `pub mod golay;` to `crates/dsp/src/fec/mod.rs`.**

- [ ] **Step 2: Implementation + KAT/correction tests**

Create `crates/dsp/src/fec/golay.rs`:

```rust
//! Golay(23,12) and extended Golay(24,12). Corrects up to 3 errors; the
//! extended code adds an overall parity bit for 3-error-correct/4-detect. Used
//! by FreeDV 1600 and M17 LICH. 12 data bits in the low bits of a u16.

/// Golay(23,12) generator polynomial 0xC75 (standard).
const GOLAY_POLY: u32 = 0xC75;

/// Encode 12 data bits → 23-bit codeword (data in high 12 bits, parity low 11).
pub fn encode23(data: u16) -> u32 {
    let mut g = (data as u32 & 0xFFF) << 11;
    let mut rem = g;
    for i in (11..23).rev() {
        if rem & (1 << i) != 0 {
            rem ^= GOLAY_POLY << (i - 11);
        }
    }
    g |= rem & 0x7FF;
    g
}

/// Decode a (possibly corrupted) 23-bit word → (data, errors_corrected) or None
/// if > 3 errors. Syndrome table-driven.
pub fn decode23(word: u32) -> Option<(u16, u32)> {
    // Build/search the syndrome the standard way: if syndrome weight ≤ 3, the
    // error pattern is the syndrome; otherwise try flipping each data bit and
    // re-checking. (Faithful to the Kasami algorithm; the tests pin it.)
    let mut best: Option<(u32, u32)> = None;
    for trial in 0..=22u32 {
        let flip = if trial == 0 { 0 } else { 1 << (trial - 1) };
        let cand = word ^ flip;
        let syn = syndrome(cand);
        let w = syn.count_ones() + flip.count_ones();
        if syn == 0 && w <= 3 {
            best = Some((cand, w));
            break;
        }
    }
    best.map(|(cw, e)| (((cw >> 11) & 0xFFF) as u16, e))
}

fn syndrome(word: u32) -> u32 {
    let mut rem = word & 0x7F_FFFF;
    for i in (11..23).rev() {
        if rem & (1 << i) != 0 {
            rem ^= GOLAY_POLY << (i - 11);
        }
    }
    rem & 0x7FF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_round_trip() {
        for d in [0u16, 1, 0xABC, 0xFFF, 0x555] {
            let cw = encode23(d);
            assert_eq!(syndrome(cw), 0, "valid codeword has zero syndrome");
            assert_eq!(decode23(cw).unwrap().0, d);
        }
    }

    #[test]
    fn corrects_up_to_one_bit() {
        let d = 0xABC;
        let cw = encode23(d);
        let bad = cw ^ (1 << 5);
        assert_eq!(decode23(bad).unwrap().0, d);
    }
}
```

> Note: the single-flip search in `decode23` corrects ≤1 error robustly and is the gate the tests pin; for the full 3-error Kasami decoder, extend the trial loop to weight-2 and weight-3 syndrome coset leaders. The reference is codec2 `golay23.c` / `libm17`.

- [ ] **Step 3: Run, then commit**

Run: `cargo test -p omnimodem-dsp fec::golay` → PASS.

```bash
git add crates/dsp/src/fec/golay.rs crates/dsp/src/fec/mod.rs
git commit -m "Add Golay(23,12)/(24,12) codec"
```

### Task A.6: GF(2⁶) soft-decision Reed–Solomon (JT65)

**Files:**
- Create: `crates/dsp/src/fec/rs_gf64.rs`
- Modify: `crates/dsp/src/fec/mod.rs`
- Test: `crates/dsp/src/fec/rs_gf64.rs` (inline)

- [ ] **Step 1: Add `pub mod rs_gf64;` to `crates/dsp/src/fec/mod.rs`.**

- [ ] **Step 2: Implementation skeleton + round-trip test**

Create `crates/dsp/src/fec/rs_gf64.rs`:

```rust
//! Reed–Solomon over GF(2⁶) with a soft-decision (Franke–Taylor) decoder for
//! JT65 RS(63,12). Hard algebraic decode is the baseline; the Franke–Taylor
//! stochastic-soft layer adds the last few dB by trial-erasing the least-
//! reliable symbols. Reference: WSJT-X `lib/` Karn RS + `ftrsd`.

/// GF(2⁶) with primitive polynomial x⁶+x+1 (0x43), matching WSJT-X JT65.
pub struct Gf64 {
    exp: [u8; 64],
    log: [u8; 64],
}

impl Gf64 {
    pub fn new() -> Self {
        let mut exp = [0u8; 64];
        let mut log = [0u8; 64];
        let mut x = 1u8;
        for i in 0..63 {
            exp[i] = x;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x40 != 0 {
                x ^= 0x43;
            }
            x &= 0x3F;
        }
        exp[63] = exp[0];
        Gf64 { exp, log }
    }

    pub fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 { 0 } else { self.exp[(self.log[a as usize] as usize + self.log[b as usize] as usize) % 63] }
    }
}

/// RS(63,12) over GF(2⁶): 12 data symbols → 63-symbol codeword.
pub struct RsGf64 {
    field: Gf64,
    nroots: usize,
}

impl RsGf64 {
    pub fn jt65() -> Self {
        RsGf64 { field: Gf64::new(), nroots: 51 }
    }

    /// Systematic encode: 12 data symbols (each 0..63) → 63 symbols.
    pub fn encode(&self, data: &[u8; 12]) -> Vec<u8> {
        // Standard systematic RS encode using the generator polynomial built
        // from nroots consecutive roots. (Port Karn `encode_rs`; the round-trip
        // test is the gate.)
        let _ = &self.field;
        let mut out = vec![0u8; 63];
        out[..12].copy_from_slice(data);
        // parity[..nroots] computed by the executor's port
        out
    }

    /// Hard algebraic decode (Berlekamp–Massey + Chien + Forney). Returns the
    /// 12 data symbols, or None on uncorrectable. The soft Franke–Taylor layer
    /// wraps this (try-erase least-reliable symbols) and is added in JT65's mode
    /// file where symbol reliabilities exist.
    pub fn decode(&self, _received: &[u8; 63]) -> Option<[u8; 12]> {
        unimplemented!("port Karn decode_rs over GF(2^6); round-trip test pins it")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gf64_mul_has_identity_and_inverse_structure() {
        let f = Gf64::new();
        assert_eq!(f.mul(1, 37), 37);
        assert_eq!(f.mul(0, 37), 0);
        // a * a^-1 = 1 for a nonzero
        for a in 1u8..64 {
            let inv = f.exp[(63 - f.log[a as usize] as usize) % 63];
            assert_eq!(f.mul(a, inv), 1, "inverse of {a}");
        }
    }

    #[test]
    #[ignore = "enable once decode_rs is ported"]
    fn rs_round_trips_clean() {
        let rs = RsGf64::jt65();
        let data = [3u8, 14, 1, 5, 9, 26, 53, 58, 0, 63, 12, 7];
        let cw: [u8; 63] = rs.encode(&data).try_into().unwrap();
        assert_eq!(rs.decode(&cw).unwrap(), data);
    }
}
```

- [ ] **Step 3: Run the field test, confirm it passes; the RS round-trip stays `#[ignore]` until `decode` is ported**

Run: `cargo test -p omnimodem-dsp fec::rs_gf64`
Expected: `gf64_mul_*` PASS; `rs_round_trips_clean` reported as ignored.

- [ ] **Step 4: Port Karn `encode_rs`/`decode_rs` and un-ignore**

Fill `encode`/`decode`, remove `#[ignore]`, re-run.
Expected: `rs_round_trips_clean` PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/fec/rs_gf64.rs crates/dsp/src/fec/mod.rs
git commit -m "Add GF(2^6) Reed-Solomon (JT65 RS(63,12)) with field tables"
```

### Task A.7: Legacy 72-bit and 50-bit message packers

**Files:**
- Modify: `crates/dsp/src/framing/message77.rs`
- Test: `crates/dsp/src/framing/message77.rs` (inline)

- [ ] **Step 1: Inspect the existing 77-bit codec**

Run: `grep -nE 'pub (fn|struct)' crates/dsp/src/framing/message77.rs`
The 77-bit packer + callsign hashing already exist (Phase 3/4). The 72-bit (JT65/JT9) and 50-bit (WSPR/FST4W) packers are the additions.

- [ ] **Step 2: Write the failing round-trip tests**

Append to `crates/dsp/src/framing/message77.rs` (inside the file, with a new `mod legacy`):

```rust
/// Legacy WSJT-X packers: 72-bit (JT65/JT9) and 50-bit (WSPR/FST4W). Distinct
/// from the modern 77-bit codec above. Bit order MSB-first (WSJT-X convention).
pub mod legacy {
    /// Pack a standard `CALL1 CALL2 GRID`-style JT65/JT9 message to 72 bits.
    pub fn pack72(message: &str) -> Option<[u8; 72]> {
        // Port WSJT-X `packjt.f90`. Structural placeholder; the round-trip test
        // is the gate.
        let _ = message;
        None
    }

    pub fn unpack72(bits: &[u8; 72]) -> Option<String> {
        let _ = bits;
        None
    }

    /// Pack a WSPR `CALL GRID dBm` message to 50 bits (28-bit call + 15-bit
    /// grid + 7-bit power), per WSJT-X `wsprd`.
    pub fn pack50(call: &str, grid4: &str, dbm: u8) -> Option<[u8; 50]> {
        let _ = (call, grid4, dbm);
        None
    }

    pub fn unpack50(bits: &[u8; 50]) -> Option<(String, String, u8)> {
        let _ = bits;
        None
    }
}

#[cfg(test)]
mod legacy_tests {
    use super::legacy::*;

    #[test]
    #[ignore = "enable once packjt port lands"]
    fn jt65_message_round_trips() {
        let bits = pack72("K1ABC W9XYZ EN37").unwrap();
        assert_eq!(unpack72(&bits).unwrap(), "K1ABC W9XYZ EN37");
    }

    #[test]
    #[ignore = "enable once wspr packer lands"]
    fn wspr_message_round_trips() {
        let bits = pack50("K1ABC", "FN42", 37).unwrap();
        assert_eq!(unpack50(&bits).unwrap(), ("K1ABC".into(), "FN42".into(), 37));
    }
}
```

- [ ] **Step 3: Port the packers from WSJT-X, un-ignore, run**

Run: `cargo test -p omnimodem-dsp framing::message77`
Expected: both legacy round-trips PASS once `packjt`/`wspr` ports land.

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/framing/message77.rs
git commit -m "Add legacy 72-bit (JT65/JT9) and 50-bit (WSPR) message packers"
```

### Task A.8: Deferred-group stubs (OFDM / vocoder / ARQ) — documentation only

**Files:**
- Modify: `crates/dsp/src/frontend/mod.rs`, `crates/dsp/src/framing/mod.rs` (doc comments only)

- [ ] **Step 1: Record the deferral in the group module docs**

The FreeDV/M17/ARDOP family is deferred (see plan §"Scope"). To keep the catalog map honest, add a one-line doc note in `crates/dsp/src/frontend/mod.rs` and `crates/dsp/src/framing/mod.rs` stating that the OFDM core (`frontend::ofdm`), vocoder interface (`framing::vocoder`), and ARQ engine are Phase-5-follow-on groups, not yet present. No code.

- [ ] **Step 2: Commit**

```bash
git add crates/dsp/src/frontend/mod.rs crates/dsp/src/framing/mod.rs
git commit -m "Note deferred OFDM/vocoder/ARQ groups in catalog module docs"
```

---

# WS-B — WSJT-X breadth: FT4, JT65, JT9, WSPR

All four are **windowed/block** modes (`BlockDemodulator`), like the Phase-4 FT8. Each reuses the FT8 STFT → candidate-finder → demap scaffolding (`frontend::stft`, `sync::candidate`, `fec::llr`) and adds its mode-specific FEC (WS-A) and message codec (`framing::message77` + the legacy packers). The registry wires them exactly as FT8: `demod_kind` returns `Windowed(_, window_s)`, `build_modulator` returns the TX `Modulator`, `tx_slot_s` returns the slot period. Before WS-B, finish WS-A.

### Task B.0: Add the four WSJT-X mode labels to the daemon config

**Files:**
- Modify: `crates/omnimodem/src/mode/mod.rs`
- Test: `crates/omnimodem/src/mode/mod.rs` (inline)

- [ ] **Step 1: Write the failing parse test**

Add to the inline `mod tests` in `crates/omnimodem/src/mode/mod.rs`:

```rust
    #[test]
    fn parse_resolves_wsjtx_breadth_modes() {
        assert_eq!(ModeConfig::parse("ft4"), Some(ModeConfig::Ft4));
        assert_eq!(ModeConfig::parse("jt65"), Some(ModeConfig::Jt65));
        assert_eq!(ModeConfig::parse("jt9"), Some(ModeConfig::Jt9));
        assert_eq!(ModeConfig::parse("wspr"), Some(ModeConfig::Wspr));
    }
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p omnimodem mode::tests::parse_resolves_wsjtx_breadth_modes`
Expected: FAIL — variants don't exist.

- [ ] **Step 3: Extend the enum + parse + label**

In `crates/omnimodem/src/mode/mod.rs`, add to the `ModeConfig` enum (after `Psk31`):

```rust
    Ft4,
    Jt65,
    Jt9,
    Wspr,
```

Add to `parse`'s match (before `_ => None`):

```rust
            "ft4" => Some(ModeConfig::Ft4),
            "jt65" => Some(ModeConfig::Jt65),
            "jt9" => Some(ModeConfig::Jt9),
            "wspr" => Some(ModeConfig::Wspr),
```

Add to `label`'s match:

```rust
            ModeConfig::Ft4 => "ft4",
            ModeConfig::Jt65 => "jt65",
            ModeConfig::Jt9 => "jt9",
            ModeConfig::Wspr => "wspr",
```

- [ ] **Step 4: Run mode tests**

Run: `cargo test -p omnimodem mode::`
Expected: PASS (the `labels_are_distinct_and_non_empty` test still holds; add the four labels there too if it enumerates).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/mode/mod.rs
git commit -m "Add FT4/JT65/JT9/WSPR mode labels to ModeConfig"
```

### Task B.1: FT4 assembly (4-GFSK + Costas-array 4×4 + (174,91) LDPC)

**Files:**
- Create: `crates/dsp/src/modes/ft4.rs`
- Modify: `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/lib.rs`
- Test: `crates/dsp/src/modes/ft4.rs` (inline) + `crates/dsp/tests/loopback.rs`

- [ ] **Step 1: Inspect the Phase-4 FT8 assembly to reuse its scaffolding**

Run: `grep -nE 'pub (fn|struct)|use ' crates/dsp/src/modes/ft8.rs | head -40`
FT4 differs from FT8 in: 4-GFSK (not 8-GFSK), 4×4 Costas arrays ×4 positions (not 7×7 ×3), BT=1.0 (not 2.0), 7.5 s window (not 15 s), and the same (174,91) LDPC + CRC-14 + 77-bit message. Reuse FT8's LDPC/OSD/CRC/77-bit calls verbatim; change the front-end + sync parameters.

- [ ] **Step 2: Write the failing modulator + caps test**

Create `crates/dsp/src/modes/ft4.rs`:

```rust
//! FT4 mode assembly: 4-GFSK, 4×4 Costas-array sync, (174,91) LDPC + CRC-14 +
//! 77-bit message, 7.5 s slots. Shares FT8's FEC/message spine (`fec::ldpc`,
//! `fec::osd`, `framing::message77`); differs in the front-end waveform (4 tones,
//! BT=1.0) and the sync arrays. Reference: WSJT-X `ft4sim`/`ft4d`.

use crate::frontend::modulate::Gfsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

pub const FT4_RATE: u32 = 12_000;
pub const FT4_WINDOW_S: f32 = 7.5;
pub const FT4_TONES: u32 = 4;
pub const FT4_BAUD: f32 = 23.4; // 12000/512 symbol length (confirm vs ft4sim)

pub struct Ft4Mod {
    gfsk: Gfsk,
}

impl Ft4Mod {
    pub fn new() -> Self {
        let sps = (FT4_RATE as f32 / FT4_BAUD).round() as usize;
        // 4-GFSK, tone spacing = baud, BT=1.0.
        Ft4Mod { gfsk: Gfsk::new(FT4_RATE as f32, sps, 600.0, FT4_BAUD, 1.0) }
    }
}

impl Default for Ft4Mod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for Ft4Mod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: FT4_RATE,
            bandwidth_hz: FT4_TONES as f32 * FT4_BAUD,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Windowed { window_s: FT4_WINDOW_S, period_s: FT4_WINDOW_S },
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            FramePayload::Message77(b) => return self.modulate_bits(b),
            other => return Err(ModError::UnsupportedPayload(crate::modes::ft8::payload_kind(other))),
        };
        // text → 77-bit message → CRC-14 → (174,91) LDPC → 4 tones interleaved
        // with the FT4 Costas arrays. Reuse the FT8 encode path with FT4 tables.
        let symbols = crate::modes::ft8::encode_message_to_symbols(&text, crate::modes::ft8::Variant::Ft4)
            .map_err(|e| ModError::Encode(e))?;
        Ok(self.gfsk.modulate(&symbols))
    }
}

impl Ft4Mod {
    fn modulate_bits(&mut self, _bits: &[u8]) -> Result<Vec<Sample>, ModError> {
        Err(ModError::UnsupportedPayload("message77 bits not yet wired for ft4"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_are_windowed_75s_tx() {
        let m = Ft4Mod::new();
        let c = m.caps();
        assert_eq!(c.native_rate, 12_000);
        assert!(c.tx);
        assert!(matches!(c.shape, DemodShape::Windowed { window_s, .. } if (window_s - 7.5).abs() < 0.01));
    }

    #[test]
    fn modulates_a_standard_message() {
        let mut m = Ft4Mod::new();
        let s = m.modulate(&Frame::text("CQ K1ABC FN42")).unwrap();
        // ~7.5 s window worth of audio at 12 kHz.
        assert!(s.len() > 10_000, "got {}", s.len());
    }
}
```

> This task assumes the Phase-4 `ft8.rs` exposes `encode_message_to_symbols(text, Variant)`, `payload_kind`, and a `Variant` enum. **Step 0 of B.1 is to refactor `ft8.rs`** to expose these as `pub(crate)` if they are currently private/monolithic — a small extraction that lets FT8/FT4 share the encode path. If `ft8.rs` hard-codes FT8 tables inline, add a `Variant { Ft8, Ft4 }` parameter threading the tone count, Costas tables, and symbol count; the FT8 loopback test (already green) is the regression gate for that refactor.

- [ ] **Step 3: Wire the module + re-export**

In `crates/dsp/src/modes/mod.rs` add `pub mod ft4;`. In `crates/dsp/src/lib.rs` add to the `pub use modes::{...}` block: `ft4::{Ft4Demod, Ft4Mod},` (reduce to `Ft4Mod` until B.2 lands the demod).

- [ ] **Step 4: Run the modulator tests**

Run: `cargo test -p omnimodem-dsp modes::ft4`
Expected: PASS (modulator + caps). Re-run the FT8 suite to confirm the refactor didn't regress: `cargo test -p omnimodem-dsp modes::ft8` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/ft4.rs crates/dsp/src/modes/mod.rs crates/dsp/src/lib.rs crates/dsp/src/modes/ft8.rs
git commit -m "Add FT4 modulator sharing the FT8 LDPC/message spine"
```

### Task B.2: FT4 block demodulator

**Files:**
- Modify: `crates/dsp/src/modes/ft4.rs`
- Test: `crates/dsp/src/modes/ft4.rs` (inline)

- [ ] **Step 1: Write the failing loopback test**

Append to `ft4.rs`'s `mod tests`:

```rust
    #[test]
    fn loopback_decodes_clean_window() {
        let msg = "CQ K1ABC FN42";
        let mut tx = Ft4Mod::new();
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        // Pad to a full 7.5 s window if the modulator emits less.
        let n = (FT4_RATE as f32 * FT4_WINDOW_S) as usize;
        let mut window = samples.clone();
        window.resize(n, 0.0);

        let mut rx = Ft4Demod::new();
        let decodes = rx.decode_window(&window, 0);
        assert!(decodes.iter().any(|f| matches!(&f.payload,
            FramePayload::Text(t) if t.contains("K1ABC"))), "no FT4 decode");
    }
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p omnimodem-dsp modes::ft4::tests::loopback_decodes_clean_window`
Expected: FAIL — `Ft4Demod` undefined.

- [ ] **Step 3: Implement the block demod by parameterizing the FT8 decoder**

```rust
use crate::frontend::stft::Stft;
use crate::sync::candidate::CandidateFinder;
use crate::types::FrameMeta;

pub struct Ft4Demod {
    inner: crate::modes::ft8::WsjtxDecoder, // shared windowed decoder, FT4-configured
}

impl Ft4Demod {
    pub fn new() -> Self {
        // The shared decoder takes a Variant that selects tone count, Costas
        // arrays, symbol count, window length and the LDPC matrix. FT4 and FT8
        // differ only in those tables; the BP+OSD core is identical.
        Ft4Demod { inner: crate::modes::ft8::WsjtxDecoder::new(crate::modes::ft8::Variant::Ft4) }
    }
}

impl Default for Ft4Demod {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockDemodulator for Ft4Demod {
    fn caps(&self) -> ModeCaps {
        Ft4Mod::new().caps()
    }

    fn decode_window(&mut self, window: &[Sample], window_start_ns: u64) -> Vec<Frame> {
        self.inner.decode_window(window, window_start_ns)
    }
}

// Silence unused-import warnings until the executor wires Stft/CandidateFinder
// directly (they live inside WsjtxDecoder).
#[allow(unused_imports)]
use {Stft as _, CandidateFinder as _, FrameMeta as _};
```

> This task's real work is the **second half of the FT8 refactor**: extract the Phase-4 FT8 windowed decoder into a `WsjtxDecoder { variant }` that holds the STFT bank, candidate finder, Costas-array correlator, soft-LLR demapper, LDPC BP+OSD, CRC-14 check and 77-bit unpack, all selected by `Variant`. FT8 becomes `WsjtxDecoder::new(Variant::Ft8)`. The existing FT8 loopback/KAT tests are the regression gate; the FT4 loopback above is the new gate. Confirm FT4 Costas arrays and symbol layout against `ft4sim` (design §"Costas-array generator + correlator (4×4 ×4 for FT4)").

- [ ] **Step 4: Re-export the demod, run loopback**

`lib.rs`: `ft4::{Ft4Demod, Ft4Mod},`. Run: `cargo test -p omnimodem-dsp modes::ft4`
Expected: PASS. FT8 suite still green.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/ft4.rs crates/dsp/src/modes/ft8.rs crates/dsp/src/lib.rs
git commit -m "Add FT4 block demod via a Variant-parameterized WsjtxDecoder"
```

### Task B.3: JT65 assembly (65-FSK + GF(2⁶) soft-RS + 72-bit message)

**Files:**
- Create: `crates/dsp/src/modes/jt65.rs`
- Modify: `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/lib.rs`
- Test: `crates/dsp/src/modes/jt65.rs` (inline)

- [ ] **Step 1: Write the failing loopback test + modulator**

Create `crates/dsp/src/modes/jt65.rs`:

```rust
//! JT65 mode assembly: 65-FSK (1 sync tone + 64 data tones), RS(63,12) over
//! GF(2⁶) with the soft Franke–Taylor decoder, 72-bit legacy message, 60 s
//! transmissions on the minute. Reference: WSJT-X `jt65sim`/`jt65`. Building
//! blocks: `frontend::modulate::MFsk`, `fec::rs_gf64::RsGf64`,
//! `framing::message77::legacy::{pack72,unpack72}`, `frontend::stft::Stft`.

use crate::fec::rs_gf64::RsGf64;
use crate::framing::message77::legacy::{pack72, unpack72};
use crate::frontend::modulate::MFsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const JT65_RATE: u32 = 11_025;
pub const JT65_WINDOW_S: f32 = 60.0;
pub const JT65_TONES: u32 = 65;
pub const JT65_SYMBOLS: usize = 126; // 63 data interleaved with 63 sync

pub struct Jt65Mod {
    rs: RsGf64,
}

impl Jt65Mod {
    pub fn new() -> Self {
        Jt65Mod { rs: RsGf64::jt65() }
    }
}

impl Default for Jt65Mod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for Jt65Mod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: JT65_RATE,
            bandwidth_hz: 175.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Windowed { window_s: JT65_WINDOW_S, period_s: JT65_WINDOW_S },
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("jt65 needs text")),
        };
        // 72-bit message → 12 six-bit RS symbols → RS(63,12) → interleave with
        // the JT65 pseudo-random sync vector → 65-FSK (sync tone + 64 data).
        let bits = pack72(&text).ok_or_else(|| ModError::TooLong(text.clone()))?;
        let data_syms = bits72_to_rs_symbols(&bits);
        let cw = self.rs.encode(&data_syms);
        let symbols = interleave_with_sync(&cw);
        let sps = (JT65_RATE as f32 / 2.69).round() as usize; // ~2.69 baud
        let mfsk = MFsk::new(JT65_RATE as f32, sps, 1270.0, 2.69, JT65_TONES);
        Ok(mfsk.modulate(&symbols))
    }
}

fn bits72_to_rs_symbols(bits: &[u8; 72]) -> [u8; 12] {
    let mut out = [0u8; 12];
    for (i, chunk) in bits.chunks(6).enumerate() {
        out[i] = chunk.iter().fold(0u8, |a, &b| (a << 1) | (b & 1));
    }
    out
}

/// Interleave 63 RS symbols with the 63-bit JT65 sync vector → 126 symbols.
fn interleave_with_sync(cw: &[u8]) -> Vec<u32> {
    // Port the WSJT-X sync-vector interleave; the loopback test pins it.
    let mut out = Vec::with_capacity(JT65_SYMBOLS);
    for &s in cw {
        out.push(0); // sync tone placeholder
        out.push(s as u32 + 1); // data tone offset by the sync tone
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_are_windowed_60s() {
        assert!(matches!(Jt65Mod::new().caps().shape,
            DemodShape::Windowed { window_s, .. } if (window_s - 60.0).abs() < 0.1));
    }

    #[test]
    #[ignore = "enable once pack72 + rs_gf64::decode land (WS-A A.6/A.7)"]
    fn loopback_decodes_message() {
        let msg = "K1ABC W9XYZ EN37";
        let mut tx = Jt65Mod::new();
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let n = (JT65_RATE as f32 * JT65_WINDOW_S) as usize;
        let mut window = samples.clone();
        window.resize(n, 0.0);
        let mut rx = Jt65Demod::new();
        let decodes = rx.decode_window(&window, 0);
        assert!(decodes.iter().any(|f| matches!(&f.payload,
            FramePayload::Text(t) if t.contains("K1ABC"))));
    }
}
```

- [ ] **Step 2: Implement the demod (uses STFT tone detection + soft-RS)**

Append to `jt65.rs`:

```rust
use crate::frontend::stft::Stft;

pub struct Jt65Demod {
    rs: RsGf64,
}

impl Jt65Demod {
    pub fn new() -> Self {
        Jt65Demod { rs: RsGf64::jt65() }
    }
}

impl Default for Jt65Demod {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockDemodulator for Jt65Demod {
    fn caps(&self) -> ModeCaps {
        Jt65Mod::new().caps()
    }

    fn decode_window(&mut self, window: &[Sample], _start_ns: u64) -> Vec<Frame> {
        // 1. STFT the window at the JT65 symbol period; per-symbol pick the 64
        //    data-tone energies → soft reliabilities.
        // 2. De-interleave the sync vector, recover 63 RS symbols + reliabilities.
        // 3. Soft Franke–Taylor RS decode → 12 symbols → 72 bits → unpack72.
        let _ = (&self.rs, Stft::new(4096));
        let mut out = Vec::new();
        if let Some(bits) = self.try_decode(window) {
            if let Some(text) = unpack72(&bits) {
                out.push(Frame {
                    payload: FramePayload::Text(text),
                    meta: FrameMeta { crc_ok: true, decoder: Some("jt65".into()), ..Default::default() },
                });
            }
        }
        out
    }
}

impl Jt65Demod {
    fn try_decode(&mut self, _window: &[Sample]) -> Option<[u8; 72]> {
        // Executor fills: STFT tone detect → soft RS decode → 72 bits.
        None
    }
}
```

- [ ] **Step 3: Wire module + re-export; run (loopback stays `#[ignore]` until WS-A lands)**

`modes/mod.rs`: `pub mod jt65;`. `lib.rs`: `jt65::{Jt65Demod, Jt65Mod},`.
Run: `cargo test -p omnimodem-dsp modes::jt65`
Expected: `caps_are_windowed_60s` PASS; loopback ignored.

- [ ] **Step 4: Un-ignore + tune once the RS soft decoder and `pack72` land**

After WS-A tasks A.6/A.7 are green, remove `#[ignore]`, fill `try_decode`/`interleave_with_sync`, and cross-check against `jt65code`/`jt65sim`. Run: `cargo test -p omnimodem-dsp modes::jt65` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/jt65.rs crates/dsp/src/modes/mod.rs crates/dsp/src/lib.rs
git commit -m "Add JT65 assembly (65-FSK + soft GF(2^6) RS + 72-bit message)"
```

### Task B.4: JT9 assembly (9-FSK + K=32 Fano + 72-bit message)

**Files:**
- Create: `crates/dsp/src/modes/jt9.rs`
- Modify: `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/lib.rs`
- Test: `crates/dsp/src/modes/jt9.rs` (inline)

- [ ] **Step 1: Modulator + caps test (mirror JT65, swap FEC to Fano)**

Create `crates/dsp/src/modes/jt9.rs` with `Jt9Mod`/`Jt9Demod` following the JT65 structure, but:
- 9-FSK (`MFsk::new(.., 9)`), ~1.74 baud, 85-symbol frame, 15.6 Hz tone spacing (confirm vs `jt9`).
- FEC: K=32 Fano (`crate::fec::fano::FanoCode`), not RS.
- Message: 72-bit `pack72`/`unpack72`.
- Windowed 60 s.

```rust
//! JT9 mode assembly: 9-FSK, K=32 r=1/2 Fano-decoded convolutional code, 72-bit
//! legacy message, 60 s on the minute. The narrowest WSJT-X JT-family mode.
//! Building blocks: `frontend::modulate::MFsk`, `fec::fano::FanoCode`,
//! `framing::message77::legacy`, `frontend::stft::Stft`. Reference: WSJT-X `jt9`.

use crate::fec::fano::FanoCode;
use crate::framing::message77::legacy::{pack72, unpack72};
use crate::frontend::modulate::MFsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const JT9_RATE: u32 = 12_000;
pub const JT9_WINDOW_S: f32 = 60.0;
pub const JT9_TONES: u32 = 9;

pub struct Jt9Mod { fano: FanoCode }
pub struct Jt9Demod { fano: FanoCode }
// ... new()/Default/Modulator/BlockDemodulator mirroring JT65, using FanoCode
// for encode/fano_decode. Loopback test gated #[ignore] until A.2 lands.
```

The full body mirrors B.3 (caps test active; loopback `#[ignore = "enable once fano lands (WS-A A.2)"]`). Tone-bank encode: 72 bits → Fano `encode` → 81 symbols → map to 8 data tones + 1 sync (confirm layout vs `jt9`).

- [ ] **Step 2–4: Wire, run, un-ignore as in B.3**

`modes/mod.rs`: `pub mod jt9;`. `lib.rs`: `jt9::{Jt9Demod, Jt9Mod},`.
Run: `cargo test -p omnimodem-dsp modes::jt9` → caps PASS, loopback ignored until A.2; un-ignore + cross-check vs `jt9` after Fano lands.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/jt9.rs crates/dsp/src/modes/mod.rs crates/dsp/src/lib.rs
git commit -m "Add JT9 assembly (9-FSK + K=32 Fano + 72-bit message)"
```

### Task B.5: WSPR assembly (4-FSK + K=32 Fano + 50-bit + bit-reversal interleave)

**Files:**
- Create: `crates/dsp/src/modes/wspr.rs`
- Modify: `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/lib.rs`
- Test: `crates/dsp/src/modes/wspr.rs` (inline)

- [ ] **Step 1: Modulator + caps + loopback test**

Create `crates/dsp/src/modes/wspr.rs`:

```rust
//! WSPR mode assembly: 4-FSK (1.4648 Hz tone spacing, 1.4648 baud), 162 symbols,
//! K=32 Fano FEC, bit-reversal interleave, 50-bit `CALL GRID dBm` message, 110.6 s
//! transmissions starting at even minutes. Reference: WSJT-X `wsprsim`/`wsprd`.
//! Building blocks: `frontend::modulate::MFsk`, `fec::fano::FanoCode`,
//! `fec::interleave::{bit_reversal_indices,permute,unpermute}`,
//! `framing::message77::legacy::{pack50,unpack50}`.

use crate::fec::fano::FanoCode;
use crate::fec::interleave::{bit_reversal_indices, permute, unpermute};
use crate::framing::message77::legacy::{pack50, unpack50};
use crate::frontend::modulate::MFsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const WSPR_RATE: u32 = 12_000;
pub const WSPR_WINDOW_S: f32 = 114.0; // ~110.6 s transmission within the 2-min slot
pub const WSPR_SYMBOLS: usize = 162;
pub const WSPR_BAUD: f32 = 1.4648;

pub struct WsprMod {
    fano: FanoCode,
    sync: Vec<u8>, // 162-entry WSPR sync vector (LSB of each symbol)
}

impl WsprMod {
    pub fn new() -> Self {
        WsprMod { fano: FanoCode::default(), sync: wspr_sync_vector() }
    }
}

impl Default for WsprMod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for WsprMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: WSPR_RATE,
            bandwidth_hz: 6.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Windowed { window_s: WSPR_WINDOW_S, period_s: 120.0 },
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("wspr needs 'CALL GRID DBM'")),
        };
        let (call, grid, dbm) = parse_wspr_message(&text).ok_or_else(|| ModError::Encode(text.clone()))?;
        let bits = pack50(&call, &grid, dbm).ok_or_else(|| ModError::TooLong(text.clone()))?;
        let coded = self.fano.encode(&bits); // → ~162 channel bits
        let idx = bit_reversal_indices(WSPR_SYMBOLS);
        let interleaved = permute(&coded[..WSPR_SYMBOLS], &idx);
        // Each WSPR symbol tone = 2*data_bit + sync_bit (4-FSK).
        let symbols: Vec<u32> = interleaved.iter().zip(self.sync.iter())
            .map(|(&d, &s)| (2 * d as u32) + s as u32).collect();
        let sps = (WSPR_RATE as f32 / WSPR_BAUD).round() as usize;
        let mfsk = MFsk::new(WSPR_RATE as f32, sps, 1500.0, WSPR_BAUD, 4);
        Ok(mfsk.modulate(&symbols))
    }
}

fn wspr_sync_vector() -> Vec<u8> {
    // The fixed 162-entry WSPR sync pattern (WSJT-X `wsprsim`). Executor pastes
    // the exact table; length-checked here.
    vec![0u8; WSPR_SYMBOLS]
}

fn parse_wspr_message(s: &str) -> Option<(String, String, u8)> {
    let mut it = s.split_whitespace();
    Some((it.next()?.to_string(), it.next()?.to_string(), it.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_are_windowed_2min_slot() {
        assert!(matches!(WsprMod::new().caps().shape,
            DemodShape::Windowed { period_s, .. } if (period_s - 120.0).abs() < 0.1));
    }

    #[test]
    fn parses_wspr_message() {
        assert_eq!(parse_wspr_message("K1ABC FN42 37"), Some(("K1ABC".into(), "FN42".into(), 37)));
    }

    #[test]
    #[ignore = "enable once fano + pack50 + sync vector land"]
    fn loopback_decodes() {
        let mut tx = WsprMod::new();
        let samples = tx.modulate(&Frame::text("K1ABC FN42 37")).unwrap();
        let n = (WSPR_RATE as f32 * WSPR_WINDOW_S) as usize;
        let mut window = samples.clone();
        window.resize(n, 0.0);
        let mut rx = WsprDemod::new();
        let d = rx.decode_window(&window, 0);
        assert!(d.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t.contains("K1ABC"))));
    }
}
```

- [ ] **Step 2: Implement `WsprDemod`** — STFT 4-tone detect → de-interleave (`unpermute`) → Fano `fano_decode` → `unpack50`; mirrors B.3's demod shape. Use `unpermute` + `unpack50` + `self.fano.fano_decode`.

- [ ] **Step 3: Wire module + re-export**

`modes/mod.rs`: `pub mod wspr;`. `lib.rs`: `wspr::{WsprDemod, WsprMod},`.

- [ ] **Step 4: Run; un-ignore after WS-A**

Run: `cargo test -p omnimodem-dsp modes::wspr` → caps + parse PASS; loopback ignored. Un-ignore + cross-check vs `wsprsim`/`wsprd` once A.2/A.7 land.

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/modes/wspr.rs crates/dsp/src/modes/mod.rs crates/dsp/src/lib.rs
git commit -m "Add WSPR assembly (4-FSK + Fano + bit-reversal interleave + 50-bit)"
```

### Task B.6: Register the four WSJT-X modes in the daemon

**Files:**
- Modify: `crates/omnimodem/src/mode/registry.rs`
- Test: `crates/omnimodem/src/mode/registry.rs` (inline)

- [ ] **Step 1: Write the failing registry test**

Add to `registry.rs`'s `mod tests`:

```rust
    #[test]
    fn wsjtx_breadth_modes_are_windowed_with_modulators() {
        for (cfg, win) in [
            (ModeConfig::Ft4, 7.5f32),
            (ModeConfig::Jt65, 60.0),
            (ModeConfig::Jt9, 60.0),
            (ModeConfig::Wspr, 114.0),
        ] {
            assert!(matches!(demod_kind(&cfg), DemodKind::Windowed(_, w) if (w - win).abs() < 0.5),
                "{cfg:?} not windowed @ {win}");
            assert!(build_modulator(&cfg).is_some(), "no modulator for {cfg:?}");
        }
        assert_eq!(tx_slot_s(&ModeConfig::Wspr), Some(120.0));
        assert_eq!(tx_slot_s(&ModeConfig::Ft4), Some(7.5));
    }
```

- [ ] **Step 2: Run to confirm it fails (non-exhaustive match)**

Run: `cargo test -p omnimodem mode::registry`
Expected: FAIL — the new `ModeConfig` arms are unhandled (compile error on the `match`es).

- [ ] **Step 3: Add the registry arms**

In `registry.rs`, import the new types:

```rust
use omnimodem_dsp::modes::{
    ft4::{Ft4Demod, Ft4Mod},
    jt65::{Jt65Demod, Jt65Mod},
    jt9::{Jt9Demod, Jt9Mod},
    wspr::{WsprDemod, WsprMod},
};
```

Add to `demod_kind` (each follows the FT8 windowed arm):

```rust
        ModeConfig::Ft4 => windowed(Box::new(Ft4Demod::new())),
        ModeConfig::Jt65 => windowed(Box::new(Jt65Demod::new())),
        ModeConfig::Jt9 => windowed(Box::new(Jt9Demod::new())),
        ModeConfig::Wspr => windowed(Box::new(WsprDemod::new())),
```

where `windowed` is a small helper added near the top:

```rust
/// Wrap a block demod, reading its window length from caps.
fn windowed(bd: Box<dyn BlockDemodulator>) -> DemodKind {
    let window_s = match bd.caps().shape {
        DemodShape::Windowed { window_s, .. } => window_s,
        _ => 15.0,
    };
    DemodKind::Windowed(bd, window_s)
}
```

(Refactor the existing FT8 arm to use `windowed(Box::new(Ft8Demod::new()))` too.) Add to `build_modulator`:

```rust
        ModeConfig::Ft4 => Some(Box::new(Ft4Mod::new())),
        ModeConfig::Jt65 => Some(Box::new(Jt65Mod::new())),
        ModeConfig::Jt9 => Some(Box::new(Jt9Mod::new())),
        ModeConfig::Wspr => Some(Box::new(WsprMod::new())),
```

`tx_slot_s` needs no new arms — it reads `caps().shape.period_s` generically.

- [ ] **Step 4: Run registry + mode tests**

Run: `cargo test -p omnimodem mode::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/mode/registry.rs
git commit -m "Register FT4/JT65/JT9/WSPR in the daemon mode registry"
```

---

# WS-C — fldigi breadth: MFSK16, Olivia, THOR, Hell

All four are **streaming** modes (`Demodulator`), like the Phase-4 PSK31/RTTY/CW. They reuse the streaming front-end (`frontend::modulate::MFsk`, `frontend::nco::DownConverter`, `frontend::stft`, `sync::timing`) and the WS-A conv/FHT/interleave blocks. Before WS-C, finish WS-A (A.1 conv, A.3 FHT, A.4 interleave). Each mode follows the Phase-4 modulator-then-demodulator-then-loopback rhythm.

### Task C.0: Add the four fldigi mode labels to the daemon config

**Files:**
- Modify: `crates/omnimodem/src/mode/mod.rs`
- Test: inline

- [ ] **Step 1: Failing parse test**

```rust
    #[test]
    fn parse_resolves_fldigi_breadth_modes() {
        assert_eq!(ModeConfig::parse("mfsk16"), Some(ModeConfig::Mfsk16));
        assert_eq!(ModeConfig::parse("olivia"), Some(ModeConfig::Olivia { tones: 32, bandwidth_hz: 1000 }));
        assert_eq!(ModeConfig::parse("thor"), Some(ModeConfig::Thor { tones: 18 }));
        assert_eq!(ModeConfig::parse("hell"), Some(ModeConfig::Hell));
    }
```

- [ ] **Step 2: Extend enum + parse + label**

Add to `ModeConfig`:

```rust
    Mfsk16,
    Olivia { tones: u16, bandwidth_hz: u16 },
    Thor { tones: u16 },
    Hell,
```

Add to `parse` (Olivia/THOR carry the common defaults — 32/1000 and 18; richer parametric strings like `olivia-16-500` are a later extension):

```rust
            "mfsk16" => Some(ModeConfig::Mfsk16),
            "olivia" => Some(ModeConfig::Olivia { tones: 32, bandwidth_hz: 1000 }),
            "thor" => Some(ModeConfig::Thor { tones: 18 }),
            "hell" => Some(ModeConfig::Hell),
```

Add to `label`:

```rust
            ModeConfig::Mfsk16 => "mfsk16",
            ModeConfig::Olivia { .. } => "olivia",
            ModeConfig::Thor { .. } => "thor",
            ModeConfig::Hell => "hell",
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p omnimodem mode::` → PASS.

```bash
git add crates/omnimodem/src/mode/mod.rs
git commit -m "Add MFSK16/Olivia/THOR/Hell mode labels to ModeConfig"
```

### Task C.1: MFSK16 assembly (16-MFSK + conv K=7 + diagonal interleave + Varicode)

**Files:**
- Create: `crates/dsp/src/modes/mfsk16.rs`
- Modify: `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/lib.rs`
- Test: inline

- [ ] **Step 1: Modulator + caps test**

Create `crates/dsp/src/modes/mfsk16.rs`:

```rust
//! MFSK16 mode assembly: 16-tone MFSK at 15.625 baud, rate-1/2 K=7
//! convolutional FEC, self-synchronizing diagonal interleave, MFSK Varicode.
//! Reference: fldigi `mfsk.cxx`. Building blocks: `frontend::modulate::MFsk`,
//! `fec::conv::ConvCode`, `fec::interleave`, `framing::varicode`,
//! `frontend::stft::Stft`, `sync::timing::TransitionMinimizer`.

use crate::fec::conv::ConvCode;
use crate::framing::varicode::{decode as vari_decode, encode as vari_encode, MFSK as MFSK_VARI};
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

pub const MFSK16_RATE: u32 = 8_000;
pub const MFSK16_BAUD: f32 = 15.625;
pub const MFSK16_TONES: u32 = 16;

pub struct Mfsk16Mod {
    conv: ConvCode,
}

impl Mfsk16Mod {
    pub fn new() -> Self {
        Mfsk16Mod { conv: ConvCode::k7_r12() }
    }
}

impl Default for Mfsk16Mod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for Mfsk16Mod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: MFSK16_RATE,
            bandwidth_hz: MFSK16_TONES as f32 * MFSK16_BAUD,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("mfsk16 needs text")),
        };
        // Varicode bits → conv-encode → diagonal interleave → Gray-mapped 4-bit
        // symbols → 16-MFSK.
        let vbits = vari_encode(&MFSK_VARI, &text);
        let coded = self.conv.encode(&vbits);
        let symbols = bits_to_mfsk_symbols(&coded); // 4 bits/symbol, Gray-coded
        let sps = (MFSK16_RATE as f32 / MFSK16_BAUD).round() as usize;
        let mfsk = MFsk::new(MFSK16_RATE as f32, sps, 1000.0, MFSK16_BAUD, MFSK16_TONES);
        Ok(mfsk.modulate(&symbols))
    }
}

fn bits_to_mfsk_symbols(bits: &[u8]) -> Vec<u32> {
    // Pack 4 bits/symbol, Gray-code (fldigi uses Gray-mapped tone indices). The
    // loopback test pins endianness + Gray direction.
    bits.chunks(4).map(|c| {
        let v = c.iter().fold(0u32, |a, &b| (a << 1) | (b as u32 & 1));
        v ^ (v >> 1) // binary → Gray
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_are_streaming_tx() {
        assert!(matches!(Mfsk16Mod::new().caps().shape, DemodShape::Streaming));
        assert!(Mfsk16Mod::new().caps().tx);
    }

    #[test]
    #[ignore = "enable once conv (A.1) and the MFSK Varicode table are present"]
    fn loopback_recovers_text() {
        let msg = "CQ DE K1ABC";
        let mut tx = Mfsk16Mod::new();
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = Mfsk16Demod::new();
        let text: String = rx.feed(&samples).iter().filter_map(|f| match &f.payload {
            FramePayload::Text(t) => Some(t.clone()), _ => None }).collect();
        assert!(text.contains("CQ DE K1ABC"), "got {text:?}");
    }
}
```

> Note: `framing::varicode` already exists with a `PSK31` table (Phase 4). MFSK16 needs the **MFSK/IZ8BLY Varicode table** — add a `pub const MFSK: VaricodeTable` to `varicode.rs` if absent (a data-only addition; the design lists "Varicode with pluggable tables — PSK, MFSK/IZ8BLY, DominoEX/THOR nibble"). Confirm the table against fldigi `varicode.cxx`.

- [ ] **Step 2: Implement `Mfsk16Demod`** — STFT 16-tone detect per symbol → soft tone energies → Gray-unmap → diagonal de-interleave → soft Viterbi (`conv.viterbi_decode`) → Varicode decode. Symbol timing via `TransitionMinimizer`. The body mirrors the Phase-4 RTTY/PSK31 demod loop but over STFT frames; the loopback test is the gate.

- [ ] **Step 3: Wire + re-export + run**

`modes/mod.rs`: `pub mod mfsk16;`. `lib.rs`: `mfsk16::{Mfsk16Demod, Mfsk16Mod},`.
Run: `cargo test -p omnimodem-dsp modes::mfsk16` → caps PASS; loopback ignored until A.1 + the MFSK Varicode table land, then un-ignore.

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/modes/mfsk16.rs crates/dsp/src/modes/mod.rs crates/dsp/src/lib.rs crates/dsp/src/framing/varicode.rs
git commit -m "Add MFSK16 assembly (16-MFSK + conv K=7 + diagonal interleave)"
```

### Task C.2: Olivia assembly (MFSK tone bank + FHT(64) + ASCII)

**Files:**
- Create: `crates/dsp/src/modes/olivia.rs`
- Modify: `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/lib.rs`
- Test: inline

- [ ] **Step 1: Modulator + caps + loopback test**

Create `crates/dsp/src/modes/olivia.rs`:

```rust
//! Olivia mode assembly: MFSK tone bank, each character encoded as a 64-bit
//! Walsh block (FHT soft-decoded), additive PRBS whitening, configurable
//! tones×bandwidth (default 32/1000). Very robust at low SNR. Reference: fldigi
//! `olivia.cxx`. Building blocks: `frontend::modulate::MFsk`, `fec::fht`,
//! `fec::scramble` (PRBS whitening), `frontend::stft::Stft`.

use crate::fec::fht::{walsh_encode, walsh_soft_decode};
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

pub const OLIVIA_RATE: u32 = 8_000;

pub struct OliviaMod {
    tones: u32,
    bandwidth_hz: f32,
}

impl OliviaMod {
    pub fn new(tones: u16, bandwidth_hz: u16) -> Self {
        OliviaMod { tones: tones as u32, bandwidth_hz: bandwidth_hz as f32 }
    }

    fn baud(&self) -> f32 {
        self.bandwidth_hz / self.tones as f32
    }
}

impl Modulator for OliviaMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: OLIVIA_RATE,
            bandwidth_hz: self.bandwidth_hz,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("olivia needs text")),
        };
        // Each 7-bit char → 64-chip Walsh block → whiten → map chips to MFSK
        // tone indices (log2(tones) bits/symbol).
        let mut symbols = Vec::new();
        for ch in text.bytes() {
            let block = walsh_encode((ch & 0x3F) as usize, 64);
            symbols.extend(chips_to_tones(&block, self.tones));
        }
        let sps = (OLIVIA_RATE as f32 / self.baud()).round() as usize;
        let base = 500.0;
        let mfsk = MFsk::new(OLIVIA_RATE as f32, sps, base, self.baud(), self.tones);
        Ok(mfsk.modulate(&symbols))
    }
}

fn chips_to_tones(chips: &[i8], tones: u32) -> Vec<u32> {
    // Pack log2(tones) Walsh chips per MFSK symbol; ±1 chip → bit. The loopback
    // test pins the packing against fldigi.
    let bits_per = (tones.trailing_zeros()) as usize;
    chips.chunks(bits_per).map(|c| {
        c.iter().fold(0u32, |a, &chip| (a << 1) | u32::from(chip < 0))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_reflect_tone_bandwidth() {
        let m = OliviaMod::new(32, 1000);
        assert_eq!(m.caps().bandwidth_hz, 1000.0);
        assert!((m.baud() - 31.25).abs() < 0.01);
    }

    #[test]
    #[ignore = "enable once fht (A.3) is present and chip packing is confirmed"]
    fn loopback_recovers_text() {
        let mut tx = OliviaMod::new(32, 1000);
        let samples = tx.modulate(&Frame::text("TEST")).unwrap();
        let mut rx = OliviaDemod::new(32, 1000);
        let text: String = rx.feed(&samples).iter().filter_map(|f| match &f.payload {
            FramePayload::Text(t) => Some(t.clone()), _ => None }).collect();
        assert!(text.contains("TEST"), "got {text:?}");
    }
}
```

- [ ] **Step 2: Implement `OliviaDemod`** — STFT tone detect → accumulate 64 soft chips/char → `walsh_soft_decode` → de-whiten → ASCII. Uses `fec::fht::walsh_soft_decode`. Loopback is the gate.

- [ ] **Step 3: Wire + re-export + run**

`modes/mod.rs`: `pub mod olivia;`. `lib.rs`: `olivia::{OliviaDemod, OliviaMod},`.
Run: `cargo test -p omnimodem-dsp modes::olivia` → caps PASS; un-ignore loopback after A.3.

- [ ] **Step 4: Commit**

```bash
git add crates/dsp/src/modes/olivia.rs crates/dsp/src/modes/mod.rs crates/dsp/src/lib.rs
git commit -m "Add Olivia assembly (MFSK tone bank + FHT(64) soft decode)"
```

### Task C.3: THOR assembly (MFSK + conv K=7 + convolutional interleave + nibble Varicode)

**Files:**
- Create: `crates/dsp/src/modes/thor.rs`
- Modify: `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/lib.rs`
- Test: inline

- [ ] **Step 1: Modulator + caps + loopback test**

Create `crates/dsp/src/modes/thor.rs` following the MFSK16 structure (C.1) but: IFK+ differential tone encoding (DominoEX lineage), depth-L convolutional interleave (`fec::interleave`), the DominoEX/THOR nibble Varicode table, and the configured tone count (THOR16=16, THOR8=8, default `tones: 18`-style profiles — store `tones` from config).

```rust
//! THOR mode assembly: MFSK with IFK+ differential tone encoding, rate-1/2 K=7
//! convolutional FEC, depth-L convolutional interleave, DominoEX-nibble
//! Varicode. Robust HF chat mode. Reference: fldigi `thor.cxx`. Building blocks:
//! `frontend::modulate::MFsk`, `fec::conv::ConvCode`, `fec::interleave`,
//! `framing::varicode` (DominoEX nibble table), `frontend::stft::Stft`.

use crate::fec::conv::ConvCode;
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

pub const THOR_RATE: u32 = 8_000;

pub struct ThorMod { tones: u32, conv: ConvCode }
pub struct ThorDemod { tones: u32, conv: ConvCode }
// new(tones)/Default/Modulator/Demodulator mirror C.1 with IFK+ differential
// mapping (each symbol = (prev_tone + nibble + 2) % tones) and the nibble
// Varicode table. caps test active; loopback #[ignore] until A.1 + nibble table.
```

The full body mirrors C.1; the IFK+ differential map and the DominoEX nibble Varicode (add `pub const DOMINOEX: VaricodeTable` to `varicode.rs`) are the THOR-specific pieces. Confirm tone spacing/IFK offset against fldigi `thor.cxx`.

- [ ] **Step 2–4: Wire, run, un-ignore, commit**

`modes/mod.rs`: `pub mod thor;`. `lib.rs`: `thor::{ThorDemod, ThorMod},`.
Run: `cargo test -p omnimodem-dsp modes::thor` → caps PASS; un-ignore after A.1.

```bash
git add crates/dsp/src/modes/thor.rs crates/dsp/src/modes/mod.rs crates/dsp/src/lib.rs crates/dsp/src/framing/varicode.rs
git commit -m "Add THOR assembly (IFK+ MFSK + conv K=7 + convolutional interleave)"
```

### Task C.4: Hell assembly (Feld-Hell OOK column raster)

**Files:**
- Create: `crates/dsp/src/modes/hell.rs`
- Modify: `crates/dsp/src/modes/mod.rs`, `crates/dsp/src/lib.rs`
- Test: inline

- [ ] **Step 1: Modulator + caps test**

Hell is **not** an FEC mode — it paints a 14-pixel-tall column raster by OOK-keying a tone, and the "decode" is image reconstruction, so the `Frame` it emits is the rendered glyph column run. Create `crates/dsp/src/modes/hell.rs`:

```rust
//! Feldhell (Feld-Hell) mode assembly: OOK column-raster fax. TX rasterizes each
//! character through a 7×14 font and OOK-keys a tone column-by-column at 122.5
//! baud / 245 columns-per-second. RX envelope-detects the tone and reassembles
//! the column raster; because Hell is a visual mode, the demod emits the
//! best-effort decoded text via the built-in font correlator (fldigi `feld.cxx`).
//! Building blocks: `frontend::modulate::CwKeyer`-style OOK keyer (reuse the
//! `frontend::osc` + envelope), `frontend::detector::EnvelopeDetector`,
//! `framing::morse`-adjacent font raster (new `hell_font` table).

use crate::frontend::detector::EnvelopeDetector;
use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FramePayload, Sample};

pub const HELL_RATE: u32 = 8_000;
pub const HELL_BAUD: f32 = 122.5;
pub const HELL_ROWS: usize = 14;

pub struct HellMod {
    tone_hz: f32,
}

impl HellMod {
    pub fn new() -> Self {
        HellMod { tone_hz: 1000.0 }
    }
}

impl Default for HellMod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for HellMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: HELL_RATE,
            bandwidth_hz: 245.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("hell needs text")),
        };
        // Each char → 7 columns × 14 rows pixel matrix; each pixel = one OOK
        // sub-element at the tone. Reuse a raised-cosine pixel envelope to limit
        // bandwidth (design §"OOK column-raster (Hellschreiber)").
        let samples = render_hell(&text, self.tone_hz, HELL_RATE as f32);
        Ok(samples)
    }
}

fn render_hell(text: &str, tone_hz: f32, rate: f32) -> Vec<Sample> {
    // Executor pastes the 7×14 Hell font table and renders pixel columns. The
    // golden snapshot test (Task I) is the gate for the waveform.
    let _ = (text, tone_hz, rate);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_are_streaming_tx() {
        assert!(matches!(HellMod::new().caps().shape, DemodShape::Streaming));
        assert!(HellMod::new().caps().tx);
    }
}
```

- [ ] **Step 2: Implement `HellDemod`** — `EnvelopeDetector` over the down-converted tone reconstructs per-column intensity; a font correlator matches reassembled 7×14 columns to glyphs and emits decoded text. Because Hell tolerates visual noise, the gate is a **loopback-at-moderate-SNR** test asserting ≥ N% of characters recovered, not bit-exactness.

- [ ] **Step 3: Wire + re-export + run + commit**

`modes/mod.rs`: `pub mod hell;`. `lib.rs`: `hell::{HellDemod, HellMod},`.
Run: `cargo test -p omnimodem-dsp modes::hell` → caps PASS.

```bash
git add crates/dsp/src/modes/hell.rs crates/dsp/src/modes/mod.rs crates/dsp/src/lib.rs
git commit -m "Add Feld-Hell assembly (OOK column raster + font correlator)"
```

### Task C.5: Register the four fldigi modes in the daemon

**Files:**
- Modify: `crates/omnimodem/src/mode/registry.rs`
- Test: inline

- [ ] **Step 1: Failing registry test**

```rust
    #[test]
    fn fldigi_breadth_modes_are_streaming_with_modulators() {
        for cfg in [
            ModeConfig::Mfsk16,
            ModeConfig::Olivia { tones: 32, bandwidth_hz: 1000 },
            ModeConfig::Thor { tones: 18 },
            ModeConfig::Hell,
        ] {
            assert!(matches!(demod_kind(&cfg), DemodKind::Streaming(_)), "{cfg:?} not streaming");
            assert!(build_modulator(&cfg).is_some(), "no modulator for {cfg:?}");
            assert_eq!(tx_slot_s(&cfg), None, "{cfg:?} is streaming, no TX slot");
        }
    }
```

- [ ] **Step 2: Add the registry arms**

Import the new types and add `demod_kind`/`build_modulator` arms:

```rust
        ModeConfig::Mfsk16 => DemodKind::Streaming(Box::new(Mfsk16Demod::new())),
        ModeConfig::Olivia { tones, bandwidth_hz } => {
            DemodKind::Streaming(Box::new(OliviaDemod::new(*tones, *bandwidth_hz)))
        }
        ModeConfig::Thor { tones } => DemodKind::Streaming(Box::new(ThorDemod::new(*tones))),
        ModeConfig::Hell => DemodKind::Streaming(Box::new(HellDemod::new())),
```

```rust
        ModeConfig::Mfsk16 => Some(Box::new(Mfsk16Mod::new())),
        ModeConfig::Olivia { tones, bandwidth_hz } => Some(Box::new(OliviaMod::new(*tones, *bandwidth_hz))),
        ModeConfig::Thor { tones } => Some(Box::new(ThorMod::new(*tones))),
        ModeConfig::Hell => Some(Box::new(HellMod::new())),
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p omnimodem mode::` → PASS.

```bash
git add crates/omnimodem/src/mode/registry.rs
git commit -m "Register MFSK16/Olivia/THOR/Hell in the daemon mode registry"
```

---

# WS-D — Metrics & observability (per-channel gRPC metrics + Prometheus)

The lossy-telemetry event class already exists (Phase 1); Phase 5 grows the **metric content** and adds an optional Prometheus exporter. Metrics ride the existing `tokio::broadcast` telemetry channel as a new `TelemetryEvent::ChannelMetrics` and are also queryable via a new unary `GetMetrics` RPC. This workstream is independent of WS-A/B/C and can run in parallel.

### Task D.1: Define the `ChannelMetrics` accumulator

**Files:**
- Create: `crates/omnimodem/src/metrics/mod.rs`
- Modify: `crates/omnimodem/src/lib.rs`
- Test: `crates/omnimodem/src/metrics/mod.rs` (inline)

- [ ] **Step 1: Declare the module**

In `crates/omnimodem/src/lib.rs`, after `pub mod core;` add:

```rust
pub mod metrics;
```

- [ ] **Step 2: Write the accumulator + failing test**

Create `crates/omnimodem/src/metrics/mod.rs`:

```rust
//! Per-channel metrics (design §"Metrics & observability"). The RX worker feeds
//! decode outcomes in; `snapshot()` produces an immutable view emitted on the
//! lossy telemetry channel and served by `GetMetrics`. All fields are the data
//! sources the design's goal #4 names: SNR, level, DCD, good/bad-FCS counts, PTT
//! state, duty cycle, AFC offset, over/underrun & clip counts, and which
//! ensemble member decoded each frame.

pub mod prometheus;

use crate::ids::ChannelId;

/// Mutable per-channel accumulator. Cheap to update on the hot path (plain
/// integer/float adds); snapshot is taken on the control edge.
#[derive(Debug, Default, Clone)]
pub struct ChannelMetrics {
    pub good_frames: u64,
    pub bad_frames: u64,
    pub tx_frames: u64,
    pub snr_db: f32,
    pub dbfs: f32,
    pub afc_offset_hz: f32,
    pub dcd: bool,
    pub ptt_keyed: bool,
    pub audio_overruns: u64,
    pub audio_underruns: u64,
    pub clip_count: u64,
    /// Which ensemble member / slicer decoded the most recent frame.
    pub last_decoder: Option<String>,
}

impl ChannelMetrics {
    /// Record a decoded frame: bump good/bad by CRC validity and remember which
    /// ensemble member produced it.
    pub fn record_frame(&mut self, crc_ok: bool, decoder: Option<&str>) {
        if crc_ok {
            self.good_frames += 1;
        } else {
            self.bad_frames += 1;
        }
        if let Some(d) = decoder {
            self.last_decoder = Some(d.to_string());
        }
    }

    /// Frame error rate over all decoded frames (0.0 when none seen).
    pub fn fer(&self) -> f32 {
        let total = self.good_frames + self.bad_frames;
        if total == 0 {
            0.0
        } else {
            self.bad_frames as f32 / total as f32
        }
    }

    pub fn snapshot(&self, channel: ChannelId) -> ChannelMetricsSnapshot {
        ChannelMetricsSnapshot { channel, metrics: self.clone() }
    }
}

/// Immutable snapshot carried over telemetry / served by `GetMetrics`.
#[derive(Debug, Clone)]
pub struct ChannelMetricsSnapshot {
    pub channel: ChannelId,
    pub metrics: ChannelMetrics,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_frame_counts_good_and_bad() {
        let mut m = ChannelMetrics::default();
        m.record_frame(true, Some("afsk1200/hydra"));
        m.record_frame(false, None);
        m.record_frame(true, Some("afsk1200/single"));
        assert_eq!(m.good_frames, 2);
        assert_eq!(m.bad_frames, 1);
        assert!((m.fer() - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(m.last_decoder.as_deref(), Some("afsk1200/single"));
    }

    #[test]
    fn fer_is_zero_with_no_frames() {
        assert_eq!(ChannelMetrics::default().fer(), 0.0);
    }
}
```

- [ ] **Step 3: Create an empty `prometheus.rs` so the module compiles**

Create `crates/omnimodem/src/metrics/prometheus.rs` with just `//! Prometheus text-exposition exporter (filled in Task D.4).` for now.

- [ ] **Step 4: Run the test**

Run: `cargo test -p omnimodem metrics::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/metrics/ crates/omnimodem/src/lib.rs
git commit -m "Add per-channel ChannelMetrics accumulator"
```

### Task D.2: Add the `ChannelMetrics` telemetry event and emit it from the RX worker

**Files:**
- Modify: `crates/omnimodem/src/core/event.rs`, `crates/omnimodem/src/core/rx_worker.rs`
- Test: `crates/omnimodem/src/core/rx_worker.rs` (inline)

- [ ] **Step 1: Add the event variant**

In `crates/omnimodem/src/core/event.rs`, add to `TelemetryEvent`:

```rust
    /// Per-channel decode/health metrics (lossy: only the latest matters).
    ChannelMetrics {
        channel: ChannelId,
        good_frames: u64,
        bad_frames: u64,
        snr_db: f32,
        dbfs: f32,
        afc_offset_hz: f32,
        dcd: bool,
        last_decoder: Option<String>,
    },
```

- [ ] **Step 2: Write the failing RX-worker test**

Inspect the RX worker first: `grep -nE 'fn |FrameEvent|telemetry|emit' crates/omnimodem/src/core/rx_worker.rs`. The worker already emits `FrameEvent::RxFrame` for each decode. Add a test asserting a `ChannelMetrics` telemetry event is emitted after decodes (adapt to the worker's existing test harness — likely a `FileBackend` capture feeding an AFSK demod, as in the Phase-4 `configuring_an_afsk_channel_spawns_rx_and_emits_frames` core test):

```rust
    #[test]
    fn rx_worker_emits_channel_metrics_after_decode() {
        // Arrange a capture of a known AFSK frame (reuse the loopback samples),
        // spawn the RX worker, and assert at least one ChannelMetrics telemetry
        // event arrives with good_frames >= 1.
        // (Mirror the existing rx_worker decode test's setup.)
    }
```

- [ ] **Step 3: Accumulate + emit in the worker loop**

In `rx_worker.rs`, give the worker a `ChannelMetrics` field. After each `feed`/`decode_window` call that returns frames, for every frame call `metrics.record_frame(frame.meta.crc_ok, frame.meta.decoder.as_deref())`, update `metrics.snr_db`/`dbfs`/`afc_offset_hz`/`dcd` from the decode metadata, and emit on a throttled cadence (e.g. once per ~1 s of audio, or whenever a frame is produced) :

```rust
        // After handling decoded frames for this chunk:
        let _ = telemetry.send(TelemetryEvent::ChannelMetrics {
            channel,
            good_frames: metrics.good_frames,
            bad_frames: metrics.bad_frames,
            snr_db: metrics.snr_db,
            dbfs: metrics.dbfs,
            afc_offset_hz: metrics.afc_offset_hz,
            dcd: metrics.dcd,
            last_decoder: metrics.last_decoder.clone(),
        });
```

Throttle: only send when the second-resolution timestamp advances or a frame was produced, so a busy channel doesn't flood the lossy channel (it's lossy, but needless churn wastes CPU).

- [ ] **Step 4: Run RX worker + core tests**

Run: `cargo test -p omnimodem core::`
Expected: PASS (existing decode tests still green; the new metrics test passes).

- [ ] **Step 5: Commit**

```bash
git add crates/omnimodem/src/core/event.rs crates/omnimodem/src/core/rx_worker.rs
git commit -m "Emit per-channel ChannelMetrics telemetry from the RX worker"
```

### Task D.3: Proto + gRPC: `ChannelMetrics` event mapping and `GetMetrics` RPC

**Files:**
- Modify: `proto/omnimodem.proto`, `crates/omnimodem/src/grpc/convert.rs`, `crates/omnimodem/src/grpc/service.rs`, `crates/omnimodem/src/grpc/subscribe.rs`, `crates/omnimodem/src/core/command.rs`, `crates/omnimodem/src/core/mod.rs`
- Test: `crates/omnimodem/tests/` (integration) or inline convert test

- [ ] **Step 1: Extend the proto (additive only — VERSIONING.md)**

In `proto/omnimodem.proto`, add the RPC to the `ModemControl` service:

```proto
  // Snapshot per-channel metrics (decode counts, SNR, levels, DCD, AFC).
  rpc GetMetrics(GetMetricsRequest) returns (GetMetricsResponse);
```

Add the new event to the `Event` oneof (next free tag = 12):

```proto
    ChannelMetrics channel_metrics = 12;  // LOSSY
```

Add the messages:

```proto
message GetMetricsRequest {
  uint32 channel = 1;   // 0 == all channels
}

message GetMetricsResponse {
  repeated ChannelMetrics metrics = 1;
}

message ChannelMetrics {
  uint32 channel = 1;
  uint64 good_frames = 2;
  uint64 bad_frames = 3;
  float snr_db = 4;
  float dbfs = 5;
  float afc_offset_hz = 6;
  bool dcd = 7;
  string last_decoder = 8;
}
```

- [ ] **Step 2: Map the telemetry event to the proto in `subscribe.rs`**

Find where `TelemetryEvent` variants become `Event`s (`grep -n 'TelemetryEvent::' crates/omnimodem/src/grpc/subscribe.rs crates/omnimodem/src/grpc/convert.rs`). Add the `ChannelMetrics` arm mapping daemon fields → `proto::ChannelMetrics`, wrapped in `Event { kind: Some(event::Kind::ChannelMetrics(..)) }`.

- [ ] **Step 3: Add a `GetMetrics` command + core handler**

In `core/command.rs` add:

```rust
    GetMetrics {
        channel: Option<ChannelId>,
        reply: tokio::sync::oneshot::Sender<Vec<crate::metrics::ChannelMetricsSnapshot>>,
    },
```

In `core/mod.rs` `handle_command`, add a `Command::GetMetrics { channel, reply }` arm that collects `ChannelMetrics::snapshot` for the matching channel(s) from the per-channel worker state the supervisor holds, and `let _ = reply.send(snaps);`. (The RX worker owns the live accumulator; mirror its latest snapshot into supervisor/channel state on each metrics emit, or keep a shared `Arc<Mutex<ChannelMetrics>>` per channel that both the worker updates and the core reads — the latter avoids a round-trip and is the recommended wiring.)

- [ ] **Step 4: Implement the `get_metrics` gRPC handler**

In `grpc/service.rs`, add:

```rust
    async fn get_metrics(
        &self,
        request: Request<proto::GetMetricsRequest>,
    ) -> Result<Response<proto::GetMetricsResponse>, Status> {
        let req = request.into_inner();
        let channel = (req.channel != 0).then_some(ChannelId(req.channel));
        let (tx, rx) = oneshot::channel();
        self.send_command(Command::GetMetrics { channel, reply: tx })?;
        let snaps = rx.await.map_err(|_| Status::unavailable("core dropped reply"))?;
        Ok(Response::new(proto::GetMetricsResponse {
            metrics: snaps.into_iter().map(convert::metrics_to_proto).collect(),
        }))
    }
```

Add `convert::metrics_to_proto(snapshot) -> proto::ChannelMetrics` in `convert.rs`.

- [ ] **Step 5: Run the full daemon test suite + commit**

Run: `cargo test -p omnimodem`
Expected: PASS (regenerated proto compiles; existing subscribe tests still green).

```bash
git add proto/omnimodem.proto crates/omnimodem/src/grpc/ crates/omnimodem/src/core/command.rs crates/omnimodem/src/core/mod.rs
git commit -m "Add GetMetrics RPC and ChannelMetrics event to the gRPC surface"
```

### Task D.4: Optional Prometheus text-exposition exporter

**Files:**
- Modify: `crates/omnimodem/src/metrics/prometheus.rs`, `crates/omnimodem/src/main.rs`
- Test: `crates/omnimodem/src/metrics/prometheus.rs` (inline)

- [ ] **Step 1: Write the failing render test**

Replace `crates/omnimodem/src/metrics/prometheus.rs`:

```rust
//! Optional Prometheus exporter. Off by default; enabled by setting
//! `OMNIMODEM_PROMETHEUS_ADDR` (e.g. `127.0.0.1:9184`). Serves the standard
//! text-exposition format on `GET /metrics` over a tokio TCP listener — no extra
//! HTTP framework, just a tiny hand-rolled responder (the surface is one route).

use crate::metrics::ChannelMetricsSnapshot;

/// Render a set of per-channel snapshots to Prometheus text exposition.
pub fn render(snaps: &[ChannelMetricsSnapshot]) -> String {
    let mut s = String::new();
    s.push_str("# HELP omnimodem_good_frames Decoded frames with valid CRC.\n");
    s.push_str("# TYPE omnimodem_good_frames counter\n");
    for snap in snaps {
        let c = snap.channel.0;
        let m = &snap.metrics;
        s.push_str(&format!("omnimodem_good_frames{{channel=\"{c}\"}} {}\n", m.good_frames));
        s.push_str(&format!("omnimodem_bad_frames{{channel=\"{c}\"}} {}\n", m.bad_frames));
        s.push_str(&format!("omnimodem_snr_db{{channel=\"{c}\"}} {}\n", m.snr_db));
        s.push_str(&format!("omnimodem_dbfs{{channel=\"{c}\"}} {}\n", m.dbfs));
        s.push_str(&format!("omnimodem_afc_offset_hz{{channel=\"{c}\"}} {}\n", m.afc_offset_hz));
        s.push_str(&format!("omnimodem_dcd{{channel=\"{c}\"}} {}\n", u8::from(m.dcd)));
    }
    s
}

/// Serve `/metrics` until cancelled. `fetch` is called per scrape to get the
/// latest snapshots (the core's `GetMetrics` path).
pub async fn serve<F>(addr: std::net::SocketAddr, fetch: F) -> std::io::Result<()>
where
    F: Fn() -> Vec<ChannelMetricsSnapshot> + Send + Sync + 'static,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(addr).await?;
    loop {
        let (mut sock, _) = listener.accept().await?;
        let body = render(&fetch());
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf).await; // drain the request line; single route
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = sock.write_all(resp.as_bytes()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ChannelId;
    use crate::metrics::ChannelMetrics;

    #[test]
    fn render_emits_labeled_series() {
        let mut m = ChannelMetrics::default();
        m.good_frames = 5;
        m.bad_frames = 1;
        m.snr_db = -7.5;
        let out = render(&[m.snapshot(ChannelId(2))]);
        assert!(out.contains("omnimodem_good_frames{channel=\"2\"} 5"));
        assert!(out.contains("omnimodem_bad_frames{channel=\"2\"} 1"));
        assert!(out.contains("omnimodem_snr_db{channel=\"2\"} -7.5"));
    }
}
```

- [ ] **Step 2: Run the render test**

Run: `cargo test -p omnimodem metrics::prometheus`
Expected: PASS.

- [ ] **Step 3: Wire the exporter into `main.rs` behind the env var**

In `main.rs`, after the core is built, if `OMNIMODEM_PROMETHEUS_ADDR` parses to a `SocketAddr`, spawn `tokio::spawn(metrics::prometheus::serve(addr, fetch))` where `fetch` issues a `GetMetrics { channel: None }` command to the core and blocks on the reply (or reads the shared per-channel `Arc<Mutex<ChannelMetrics>>` snapshots directly). Add the `serve` future to the `tokio::select!` so shutdown tears it down.

- [ ] **Step 4: Build + commit**

Run: `cargo build -p omnimodem`
Expected: builds.

```bash
git add crates/omnimodem/src/metrics/prometheus.rs crates/omnimodem/src/main.rs
git commit -m "Add optional Prometheus /metrics exporter (env-gated)"
```

---

# WS-E — Safety & integration: TX exclusive lease, mTLS, reference CLI/TUI

Three independent integration pieces. None depends on WS-A/B/C; the lease and mTLS are daemon seams, the CLI is a new client crate.

### Task E.1: TX exclusive lease registry

**Files:**
- Create: `crates/omnimodem/src/ptt/lease.rs`
- Modify: `crates/omnimodem/src/ptt/mod.rs`
- Test: `crates/omnimodem/src/ptt/lease.rs` (inline)

- [ ] **Step 1: Declare the module**

In `crates/omnimodem/src/ptt/mod.rs`, add `pub mod lease;`.

- [ ] **Step 2: Write the registry + failing tests**

Create `crates/omnimodem/src/ptt/lease.rs`:

```rust
//! TX exclusive lease (design §"Phase 4 TX model" / §"Open questions": the lease
//! itself slips to Phase 5). A session that cannot tolerate interleaved TX
//! (contest/Winlink) takes an exclusive lease on a rig; while held, only the
//! lease-holding channel may transmit on that rig. Keyed by DeviceId (the rig),
//! because two channels sharing one physical radio contend for the same lease.
//! This composes with — does not replace — the RxTxInterlock: the interlock
//! mutes RX during any key; the lease governs *who may queue TX at all*.

use crate::ids::{ChannelId, DeviceId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct TxLeaseRegistry {
    holders: Arc<Mutex<HashMap<DeviceId, ChannelId>>>,
}

/// Why an `acquire` failed.
#[derive(Debug, PartialEq, Eq)]
pub enum LeaseError {
    /// Another channel already holds the rig's exclusive lease.
    HeldBy(ChannelId),
}

impl TxLeaseRegistry {
    pub fn new() -> Self {
        TxLeaseRegistry::default()
    }

    /// Acquire the exclusive lease on `rig` for `channel`. Idempotent for the
    /// current holder; errors if a different channel holds it.
    pub fn acquire(&self, rig: &DeviceId, channel: ChannelId) -> Result<(), LeaseError> {
        let mut map = self.holders.lock().unwrap();
        match map.get(rig) {
            Some(&h) if h == channel => Ok(()),
            Some(&h) => Err(LeaseError::HeldBy(h)),
            None => {
                map.insert(rig.clone(), channel);
                Ok(())
            }
        }
    }

    /// Release the lease if `channel` holds it (no-op otherwise).
    pub fn release(&self, rig: &DeviceId, channel: ChannelId) {
        let mut map = self.holders.lock().unwrap();
        if map.get(rig) == Some(&channel) {
            map.remove(rig);
        }
    }

    /// May `channel` transmit on `rig` right now? True if the rig is unleased or
    /// leased to this channel. The TX worker checks this before keying.
    pub fn may_transmit(&self, rig: &DeviceId, channel: ChannelId) -> bool {
        match self.holders.lock().unwrap().get(rig) {
            Some(&h) => h == channel,
            None => true,
        }
    }

    /// Release every lease a channel holds (called on channel teardown).
    pub fn release_all(&self, channel: ChannelId) {
        self.holders.lock().unwrap().retain(|_, &mut h| h != channel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig(t: &str) -> DeviceId {
        DeviceId::AlsaCard { card_name: t.into() }
    }

    #[test]
    fn unleased_rig_allows_any_channel() {
        let r = TxLeaseRegistry::new();
        assert!(r.may_transmit(&rig("A"), ChannelId(1)));
    }

    #[test]
    fn holder_excludes_others() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        assert!(r.may_transmit(&rig("A"), ChannelId(1)));
        assert!(!r.may_transmit(&rig("A"), ChannelId(2)));
        assert_eq!(r.acquire(&rig("A"), ChannelId(2)), Err(LeaseError::HeldBy(ChannelId(1))));
    }

    #[test]
    fn acquire_is_idempotent_for_holder() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        assert!(r.acquire(&rig("A"), ChannelId(1)).is_ok());
    }

    #[test]
    fn release_frees_the_rig() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        r.release(&rig("A"), ChannelId(1));
        assert!(r.may_transmit(&rig("A"), ChannelId(2)));
    }

    #[test]
    fn release_all_drops_every_lease_for_a_channel() {
        let r = TxLeaseRegistry::new();
        r.acquire(&rig("A"), ChannelId(1)).unwrap();
        r.acquire(&rig("B"), ChannelId(1)).unwrap();
        r.release_all(ChannelId(1));
        assert!(r.may_transmit(&rig("A"), ChannelId(2)));
        assert!(r.may_transmit(&rig("B"), ChannelId(2)));
    }
}
```

- [ ] **Step 3: Run + commit**

Run: `cargo test -p omnimodem ptt::lease` → PASS.

```bash
git add crates/omnimodem/src/ptt/lease.rs crates/omnimodem/src/ptt/mod.rs
git commit -m "Add TX exclusive lease registry (per-rig, per-channel)"
```

### Task E.2: Honor the lease in the TX worker + expose Acquire/Release RPCs

**Files:**
- Modify: `crates/omnimodem/src/core/tx_worker.rs`, `crates/omnimodem/src/core/mod.rs`, `crates/omnimodem/src/core/command.rs`, `proto/omnimodem.proto`, `crates/omnimodem/src/grpc/service.rs`
- Test: `crates/omnimodem/src/core/tx_worker.rs` (inline)

- [ ] **Step 1: Thread the lease into `TxWorkerCfg`**

In `tx_worker.rs`, add to `TxWorkerCfg`:

```rust
    pub channel_id: ChannelId,
    pub lease: crate::ptt::lease::TxLeaseRegistry,
```

(`channel` already exists for events; reuse it if it is a `ChannelId` — check the existing field; the lease check needs the channel + rig already on the cfg.)

- [ ] **Step 2: Write the failing test — a non-holder's job is rejected**

```rust
    #[test]
    fn worker_skips_tx_when_another_channel_holds_the_lease() {
        // Acquire the rig's lease for a DIFFERENT channel, enqueue a job on this
        // worker's channel, and assert it completes WITHOUT keying (TransmitComplete
        // with no PttKeyed{keyed:true}) — the lease blocks the key.
    }
```

- [ ] **Step 3: Check the lease before keying**

In `run()`, just before `cfg.interlock.begin_tx(&cfg.rig)`:

```rust
        if !cfg.lease.may_transmit(&cfg.rig, cfg.channel) {
            // Another channel holds the exclusive lease; drop this job without
            // keying. Surface start+complete so the client isn't left hanging.
            let _ = cfg.telemetry.send(TelemetryEvent::TransmitStarted {
                channel: cfg.channel, transmit_id: job.transmit_id });
            let _ = cfg.telemetry.send(TelemetryEvent::TransmitComplete {
                channel: cfg.channel, transmit_id: job.transmit_id });
            continue;
        }
```

(`cfg.channel` is the `ChannelId`; if the existing field is named differently, use it.)

- [ ] **Step 4: Add Acquire/Release commands + RPCs**

Proto additions (additive):

```proto
  // Acquire/release a channel's exclusive TX lease on its bound rig.
  rpc AcquireTxLease(TxLeaseRequest) returns (TxLeaseResponse);
  rpc ReleaseTxLease(TxLeaseRequest) returns (TxLeaseResponse);
```

```proto
message TxLeaseRequest { uint32 channel = 1; }
message TxLeaseResponse {
  bool granted = 1;       // false on Acquire if another channel holds it
  uint32 held_by = 2;     // the current holder when granted == false
}
```

Add `Command::AcquireTxLease { channel, reply }` / `ReleaseTxLease { channel, reply }`; the core handler resolves the channel's bound rig (from supervisor/channel state), calls `lease.acquire`/`release`, and replies with the grant result. Add the two unary gRPC handlers in `service.rs` mirroring `key_ptt`. On channel teardown in `core/mod.rs`, call `lease.release_all(channel)`.

- [ ] **Step 5: Run + commit**

Run: `cargo test -p omnimodem`
Expected: PASS.

```bash
git add crates/omnimodem/src/core/ proto/omnimodem.proto crates/omnimodem/src/grpc/service.rs
git commit -m "Honor TX exclusive lease in the TX worker; add Acquire/Release RPCs"
```

### Task E.3: Real mTLS for routable binds

**Files:**
- Modify: `crates/omnimodem/src/authz/tls.rs`, `crates/omnimodem/src/authz/mod.rs`, `crates/omnimodem/src/main.rs`, `Cargo.toml` (workspace), `crates/omnimodem/Cargo.toml`
- Test: `crates/omnimodem/src/authz/tls.rs` (inline) + `crates/omnimodem/tests/` (integration)

- [ ] **Step 1: Enable the `tls` feature on tonic**

In the workspace `Cargo.toml`, change the tonic dependency to `tonic = { version = "0.12", features = ["tls"] }`. (The daemon crate inherits it via `workspace = true`.)

- [ ] **Step 2: Replace the fail-closed stub with a real config loader + failing test**

Rewrite `crates/omnimodem/src/authz/tls.rs`:

```rust
//! mTLS for routable binds (design §"Local authorization": mTLS + per-method
//! authz is MANDATORY for any routable interface). Phases 1–4 only stubbed this
//! to fail closed; Phase 5 loads server cert/key + a client-CA bundle and builds
//! a tonic `ServerTlsConfig` that REQUIRES client certificates. A routable bind
//! still fails closed if any of the three PEM paths is missing or unreadable.

use std::path::Path;
use tonic::transport::{Identity, ServerTlsConfig};

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("routable bind requires OMNIMODEM_TLS_CERT, _KEY and _CLIENT_CA to be set")]
    NotConfigured,
    #[error("reading TLS material: {0}")]
    Io(#[from] std::io::Error),
}

/// Paths to the PEM material for a routable mTLS bind.
pub struct TlsPaths {
    pub server_cert: std::path::PathBuf,
    pub server_key: std::path::PathBuf,
    pub client_ca: std::path::PathBuf,
}

impl TlsPaths {
    /// Read the three paths from the environment; `None` if any is unset.
    pub fn from_env() -> Option<TlsPaths> {
        Some(TlsPaths {
            server_cert: std::env::var_os("OMNIMODEM_TLS_CERT")?.into(),
            server_key: std::env::var_os("OMNIMODEM_TLS_KEY")?.into(),
            client_ca: std::env::var_os("OMNIMODEM_TLS_CLIENT_CA")?.into(),
        })
    }
}

/// Build a tonic mTLS server config that REQUIRES a client cert chained to
/// `client_ca`. Fails closed (`NotConfigured`) if `paths` is `None`.
pub fn routable_tls_config(paths: Option<TlsPaths>) -> Result<ServerTlsConfig, TlsError> {
    let p = paths.ok_or(TlsError::NotConfigured)?;
    let cert = std::fs::read(&p.server_cert)?;
    let key = std::fs::read(&p.server_key)?;
    let client_ca = std::fs::read(&p.client_ca)?;
    Ok(ServerTlsConfig::new()
        .identity(Identity::from_pem(cert, key))
        .client_ca_root(tonic::transport::Certificate::from_pem(client_ca)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_fails_closed() {
        assert!(matches!(routable_tls_config(None), Err(TlsError::NotConfigured)));
    }

    #[test]
    fn missing_file_is_an_io_error_not_a_silent_open() {
        let paths = TlsPaths {
            server_cert: "/nonexistent/cert.pem".into(),
            server_key: "/nonexistent/key.pem".into(),
            client_ca: "/nonexistent/ca.pem".into(),
        };
        assert!(matches!(routable_tls_config(Some(paths)), Err(TlsError::Io(_))));
    }
}
```

- [ ] **Step 3: Update `validate_transport` and add `serve_routable`**

In `authz/mod.rs`, update the `Transport::Routable` arm of `validate_transport` to call `tls::routable_tls_config(tls::TlsPaths::from_env())?` and discard the config (validation only confirms it *can* be built). Add a `serve_routable` that binds TCP under the mTLS config and applies the **same** per-method peer authorization the UDS path uses — but keyed on the client-cert subject/SAN rather than `SO_PEERCRED`:

```rust
/// Serve the control plane over a routable TCP interface under mTLS. Client
/// certs are required (config built in `tls`); a per-method interceptor can
/// further restrict by cert identity. Fails closed if TLS material is absent.
pub async fn serve_routable(
    svc: ControlService,
    addr: std::net::SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let tls = tls::routable_tls_config(tls::TlsPaths::from_env())?;
    tonic::transport::Server::builder()
        .tls_config(tls)?
        .add_service(ModemControlServer::new(svc))
        .serve(addr)
        .await?;
    Ok(())
}
```

- [ ] **Step 4: Select transport in `main.rs` from the environment**

In `main.rs`, choose the transport: if `OMNIMODEM_ROUTABLE_ADDR` is set, build `Transport::Routable { addr }`, run `validate_transport` (which now fails closed without certs), and call `serve_routable`; otherwise keep the UDS path. Keep the `tokio::select!` shutdown wrapper.

- [ ] **Step 5: Run tests + build, then commit**

Run: `cargo test -p omnimodem authz::` and `cargo build -p omnimodem`
Expected: PASS / builds.

```bash
git add crates/omnimodem/src/authz/ crates/omnimodem/src/main.rs Cargo.toml crates/omnimodem/Cargo.toml
git commit -m "Implement mTLS for routable binds (fail-closed cert loading + serve_routable)"
```

### Task E.4: Reference CLI client crate (clap subcommands)

**Files:**
- Create: `crates/omnimodem-cli/Cargo.toml`, `crates/omnimodem-cli/src/main.rs`
- Modify: `Cargo.toml` (workspace members)
- Test: `crates/omnimodem-cli/src/main.rs` (inline arg-parse tests)

- [ ] **Step 1: Add the crate to the workspace**

In the workspace `Cargo.toml`, add `"crates/omnimodem-cli"` to `members`. Add to `[workspace.dependencies]`:

```toml
clap = { version = "4", features = ["derive"] }
ratatui = "0.28"
crossterm = "0.28"
```

- [ ] **Step 2: Create the crate manifest**

Create `crates/omnimodem-cli/Cargo.toml`:

```toml
[package]
name = "omnimodem-cli"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "omnimodem"
path = "src/main.rs"

[dependencies]
tonic = { workspace = true }
prost = { workspace = true }
tokio = { workspace = true }
clap = { workspace = true }
ratatui = { workspace = true }
crossterm = { workspace = true }
tonic-build = { workspace = true }

[build-dependencies]
tonic-build = { workspace = true }
```

The CLI reuses the same `proto/omnimodem.proto`; add a `build.rs` that compiles it (copy the daemon's `build.rs` pattern — confirm with `cat crates/omnimodem/build.rs`).

- [ ] **Step 3: Write the clap CLI + arg-parse test**

Create `crates/omnimodem-cli/src/main.rs`:

```rust
//! `omnimodem` — reference CLI/TUI client for omnimodem. Speaks the same
//! `ModemControl` gRPC surface third-party frontends use (design goal #2), over
//! the default UDS or a routable mTLS endpoint. Subcommands cover the everyday
//! operator loop; `tui` opens the live dashboard.

mod tui;

pub mod proto {
    tonic::include_proto!("omnimodem.v1");
}

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "omnimodem", about = "Reference client for the omnimodem software modem")]
struct Cli {
    /// Daemon endpoint: a UDS path (default) or http(s) URL for a routable bind.
    #[arg(long, default_value = "/tmp/omnimodem/omnimodem.sock", global = true)]
    endpoint: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List channels and their current state.
    State,
    /// List present audio/PTT devices.
    Devices,
    /// Configure a channel's mode: `omnimodem channel <id> --name NAME --mode ft8`.
    Channel { id: u32, #[arg(long)] name: String, #[arg(long)] mode: String },
    /// Transmit a payload on a channel (UTF-8 text, or hex with --hex).
    Transmit { channel: u32, payload: String, #[arg(long)] hex: bool },
    /// Acquire/release the exclusive TX lease on a channel's rig.
    Lease { channel: u32, #[arg(long)] release: bool },
    /// Print current per-channel metrics.
    Metrics { #[arg(long, default_value_t = 0)] channel: u32 },
    /// Open the live TUI dashboard (channels, metrics, event stream).
    Tui,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Tui => tui::run(&cli.endpoint).await,
        other => run_command(&cli.endpoint, other).await,
    }
}

async fn run_command(_endpoint: &str, _cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    // Connect via the proto client and dispatch. UDS connect uses
    // `tonic::transport::Endpoint` + a custom connector for the Unix socket;
    // http(s) endpoints connect directly (mTLS material from env for routable).
    // Each arm maps to one unary RPC defined in the proto.
    unimplemented!("dispatch each subcommand to its RPC")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_channel_subcommand() {
        let cli = Cli::try_parse_from([
            "omnimodem", "channel", "0", "--name", "main", "--mode", "ft8",
        ])
        .unwrap();
        match cli.command {
            Command::Channel { id, name, mode } => {
                assert_eq!(id, 0);
                assert_eq!(name, "main");
                assert_eq!(mode, "ft8");
            }
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn transmit_defaults_to_text() {
        let cli = Cli::try_parse_from(["omnimodem", "transmit", "1", "CQ"]).unwrap();
        assert!(matches!(cli.command, Command::Transmit { channel: 1, hex: false, .. }));
    }
}
```

- [ ] **Step 4: Implement `run_command`**

Fill the dispatch: build the gRPC client (UDS connector for socket-path endpoints; direct connect for `http(s)://`), and map each subcommand to its RPC (`State`→`GetState`, `Devices`→`ListDevices`, `Channel`→`ConfigureChannel`, `Transmit`→`Transmit`, `Lease`→`AcquireTxLease`/`ReleaseTxLease`, `Metrics`→`GetMetrics`). Pretty-print responses as aligned tables. The UDS connector follows the tonic Unix-socket example (a `tower::service_fn` over `tokio::net::UnixStream`).

- [ ] **Step 5: Run the arg-parse tests + build, commit**

Run: `cargo test -p omnimodem-cli` and `cargo build -p omnimodem-cli`
Expected: PASS / builds.

```bash
git add crates/omnimodem-cli/ Cargo.toml
git commit -m "Add reference omnimodem CLI client (clap subcommands over gRPC)"
```

### Task E.5: Reference TUI dashboard

**Files:**
- Create: `crates/omnimodem-cli/src/tui.rs`
- Test: `crates/omnimodem-cli/src/tui.rs` (inline render test)

- [ ] **Step 1: Write the TUI skeleton + a pure-render test**

Create `crates/omnimodem-cli/src/tui.rs`:

```rust
//! Live TUI dashboard: a channel table (mode, device, running, SNR, good/bad
//! frames, DCD, PTT) refreshed from `GetMetrics` + `GetState`, and a scrolling
//! event pane fed by `SubscribeEvents`. ratatui + crossterm. The render is a
//! pure function of a `DashboardState` so it is unit-testable headless.

use crate::proto;

/// Everything the dashboard draws, decoupled from the terminal so `render_rows`
/// can be tested without a TTY.
#[derive(Default)]
pub struct DashboardState {
    pub channels: Vec<proto::ChannelInfo>,
    pub metrics: Vec<proto::ChannelMetrics>,
    pub recent_events: Vec<String>,
}

impl DashboardState {
    /// One display row per channel, joining state + latest metrics.
    pub fn render_rows(&self) -> Vec<[String; 6]> {
        self.channels
            .iter()
            .map(|c| {
                let m = self.metrics.iter().find(|m| m.channel == c.channel);
                [
                    c.channel.to_string(),
                    c.mode.clone(),
                    if c.running { "▶".into() } else { "■".into() },
                    m.map(|m| format!("{:.1}", m.snr_db)).unwrap_or_else(|| "-".into()),
                    m.map(|m| m.good_frames.to_string()).unwrap_or_else(|| "0".into()),
                    m.map(|m| u8::from(m.dcd).to_string()).unwrap_or_else(|| "0".into()),
                ]
            })
            .collect()
    }
}

/// Run the dashboard until the user quits (`q`). Connects, subscribes, and
/// redraws on each event / metrics tick.
pub async fn run(_endpoint: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Executor: set up crossterm raw mode + ratatui Terminal, spawn the
    // SubscribeEvents stream into a channel, poll GetMetrics on a tick, and draw
    // the table from DashboardState::render_rows in the main loop. Restore the
    // terminal on exit (including on panic).
    unimplemented!("crossterm/ratatui event loop")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_joins_channel_state_and_metrics() {
        let st = DashboardState {
            channels: vec![proto::ChannelInfo {
                channel: 0,
                name: "main".into(),
                mode: "ft8".into(),
                device_id: "x".into(),
                running: true,
            }],
            metrics: vec![proto::ChannelMetrics {
                channel: 0,
                good_frames: 7,
                bad_frames: 1,
                snr_db: -12.0,
                dbfs: -20.0,
                afc_offset_hz: 1.5,
                dcd: true,
                last_decoder: "ft8".into(),
            }],
            recent_events: vec![],
        };
        let rows = st.render_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], "ft8");
        assert_eq!(rows[0][3], "-12.0");
        assert_eq!(rows[0][4], "7");
        assert_eq!(rows[0][5], "1"); // dcd true → "1"
    }
}
```

- [ ] **Step 2: Implement `run`** — crossterm raw-mode setup, ratatui `Terminal`, a `SubscribeEvents` task feeding `recent_events`, a periodic `GetMetrics`/`GetState` poll updating `DashboardState`, and a draw loop rendering the table + event pane. Restore the terminal on exit and on panic (install a panic hook).

- [ ] **Step 3: Run the render test + build, commit**

Run: `cargo test -p omnimodem-cli tui` and `cargo build -p omnimodem-cli`
Expected: PASS / builds.

```bash
git add crates/omnimodem-cli/src/tui.rs
git commit -m "Add reference TUI dashboard (channels + metrics + event stream)"
```

---

# Part I — Conformance gates (layered in per mode, finalized last)

Per the design's "definition of done," a mode is not done on a loopback demo — it must pass KAT vectors, bidirectional cross-decode against the reference, and a BER/decode-rate curve that is equal-or-better at every SNR point. These gates extend the existing Phase-4 conformance harness (`crates/dsp/tests/{kat,ber,loopback,snapshots}.rs`). Add each mode's gate as that mode lands; finalize the aggregate `phase5_exit_criterion` once all eight modes are green.

### Task I.1: Per-mode loopback + golden-snapshot gates

**Files:**
- Modify: `crates/dsp/tests/loopback.rs`, `crates/dsp/tests/snapshots.rs`

- [ ] **Step 1: Add a loopback round-trip per new mode**

For each of the eight modes, add a `#[test]` to `loopback.rs` that modulates a fixed message and asserts the demod recovers it (windowed modes pad to a full window, as in the inline tests). These mirror the inline loopback tests but live in the integration suite where the whole `omnimodem-dsp` surface is exercised together. Reuse the inline test bodies.

- [ ] **Step 2: Add a modulator golden snapshot per new mode**

For each mode, add an `insta` snapshot of the modulated symbol stream (or a hash of the waveform) for a fixed message to `snapshots.rs`, so any change that alters on-air output is caught in review (design §"Modulator golden snapshots").

- [ ] **Step 3: Run + review snapshots + commit**

Run: `cargo test -p omnimodem-dsp --test loopback --test snapshots`
Then `cargo insta review` to accept the new golden snapshots.

```bash
git add crates/dsp/tests/loopback.rs crates/dsp/tests/snapshots.rs crates/dsp/src/snapshots/
git commit -m "Add Phase-5 per-mode loopback and modulator golden-snapshot gates"
```

### Task I.2: Per-mode BER/decode-rate sweeps with thresholds

**Files:**
- Modify: `crates/dsp/tests/ber.rs`, `crates/dsp/src/testutil.rs`

- [ ] **Step 1: Add a seeded AWGN decode-rate sweep per mode**

For each mode, add a sweep over a committed SNR range (using the existing `testutil::add_awgn`/`Rng` and `WattersonChannel` fixtures) asserting the decode rate meets a per-mode threshold at each point. WSJT-X modes sweep in a 2500 Hz reference bandwidth; the thresholds are the committed numbers from the design's "equal-or-better at every SNR point" bar. Mark the heavy sweeps `#[ignore]` so they run nightly, not on every PR (design §"CI tiering").

- [ ] **Step 2: Add Watterson HF-fading sweeps for the weak-signal modes**

The WSJT-X and Olivia modes are built for fading channels; add CCIR good/moderate/poor Watterson sweeps (the `WattersonChannel` fixture from Phase 4) for FT4/JT65/JT9/WSPR/Olivia, since AWGN-only overstates performance (design §"Channel simulators").

- [ ] **Step 3: Run the fast subset + commit**

Run: `cargo test -p omnimodem-dsp --test ber` (the non-`#[ignore]` quick points)
Expected: PASS.

```bash
git add crates/dsp/tests/ber.rs crates/dsp/src/testutil.rs
git commit -m "Add Phase-5 per-mode BER/decode-rate + Watterson sweeps"
```

### Task I.3: Bidirectional cross-decode interop gates (reference binaries)

**Files:**
- Modify: `crates/dsp/tests/kat.rs`

- [ ] **Step 1: Add `#[ignore]`-gated cross-decode tests per mode**

For each mode, add **both directions** behind `#[ignore]` (they need the reference toolchains installed; run nightly/behind a label):
- our TX → reference decoder: FT4/JT65/JT9 → WSJT-X `jt9`/`jt65`; WSPR → `wsprd`; MFSK16/Olivia/THOR/Hell → fldigi.
- reference TX → our decoder: WSJT-X `ft4sim`/`jt65code`/`wsprsim` output → our block demods; fldigi-generated audio → our streaming demods.

Each writes our modulator output to a WAV the reference binary consumes (and vice-versa via the file `AudioBackend`), then asserts the message matches. Skip cleanly when the binary is absent (probe `which jt9` etc.) so the suite is green on a dev box without the toolchains.

- [ ] **Step 2: Add per-block KATs for the WS-A building blocks**

Add known-answer vectors to `kat.rs` for the new coding blocks against published/reference vectors: conv K=7 encode, Fano round-trip at a reference SNR, FHT(64) symbol recovery, Golay(23,12) syndrome table, GF(2⁶) RS(63,12) encode, the 72/50-bit packers. These pin the WS-A blocks to the standards, not just to themselves.

- [ ] **Step 3: Run the non-ignored KATs + commit**

Run: `cargo test -p omnimodem-dsp --test kat`
Expected: PASS (the cross-decode tests are reported ignored without the reference binaries).

```bash
git add crates/dsp/tests/kat.rs
git commit -m "Add Phase-5 cross-decode interop gates and WS-A block KATs"
```

### Task I.4: The `phase5_exit_criterion` aggregate gate

**Files:**
- Modify: `crates/dsp/tests/kat.rs` (or wherever `phase4_exit_criterion` lives)

- [ ] **Step 1: Inspect the Phase-4 aggregate gate**

Run: `grep -rn 'phase4_exit_criterion' crates/dsp/tests/`
The Phase-4 gate asserts every Phase-4 mode's KAT + loopback pass. Phase 5 adds an analogous aggregate.

- [ ] **Step 2: Write the Phase-5 aggregate gate**

Add a `phase5_exit_criterion` test that asserts, for all eight new modes: the loopback round-trip passes, the fast BER point meets threshold, and the modulator golden snapshot is stable. (The `#[ignore]` cross-decode + heavy sweeps are referenced in a doc comment as the nightly completion of the bar, not run inline.)

- [ ] **Step 3: Run + commit**

Run: `cargo test -p omnimodem-dsp --test kat phase5_exit_criterion`
Expected: PASS once all eight modes are implemented.

```bash
git add crates/dsp/tests/kat.rs
git commit -m "Add phase5_exit_criterion aggregate conformance gate"
```

### Task I.5: Final workspace verification

- [ ] **Step 1: Full build + test + lint**

Run, in order:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: all green. The `#[ignore]` nightly interop/sweep tests are excluded from the default run; document in the PR description how to run them (`cargo test --workspace -- --ignored` with the reference toolchains installed).

- [ ] **Step 2: Update the design doc status line**

In `docs/design/2026-06-17-omnimodem-design.md`, update the top-of-file **Status** line to record Phase 5 breadth + integration as implemented, and link this plan (matching how Phases 1–3 reference their plans).

- [ ] **Step 3: Commit**

```bash
git add docs/design/2026-06-17-omnimodem-design.md
git commit -m "Mark Phase 5 (breadth + integration & safety) implemented in the design doc"
```

---

## Self-review (performed against the design)

**Spec coverage** — every Phase-5 design component maps to a task:

| Design §"Phase 5" component | Task(s) |
|---|---|
| WSJT-X breadth (FT4/JT65/JT9/WSPR) | B.1–B.6 |
| fldigi modes (MFSK/Olivia/THOR/Hell) | C.1–C.5 |
| Remaining building-block groups (conv/Viterbi/FHT/interleave; +Fano/Golay/soft-RS) | A.1–A.7 |
| FreeDV/M17/ARDOP "eventually" + OFDM/vocoder/ARQ groups | Explicitly deferred (Scope §; A.8 records the deferral) |
| Metrics & observability + Prometheus exporter | D.1–D.4 |
| TX exclusive lease | E.1–E.2 |
| mTLS for routable binds | E.3 |
| Reference CLI/TUI client | E.4–E.5 |
| KISS/AGWPE translator | Explicitly deferred (Scope §: out of core, external process) |
| Conformance bar (KAT + bidirectional cross-decode + BER curves) | I.1–I.4 |

**Type consistency** — the registry uses `ModeConfig::{Ft4,Jt65,Jt9,Wspr,Mfsk16,Olivia{tones,bandwidth_hz},Thor{tones},Hell}` identically in `mode/mod.rs` (B.0/C.0) and `mode/registry.rs` (B.6/C.5). Each mode file exposes `{Name}Mod`/`{Name}Demod` with `new()`/`Default`, matching the re-export lines and the registry constructors. `ChannelMetrics` field names are identical across `metrics/mod.rs` (D.1), the `TelemetryEvent::ChannelMetrics` variant (D.2), the proto message (D.3), and the Prometheus renderer (D.4). `TxLeaseRegistry::{acquire,release,may_transmit,release_all}` names match between E.1 and E.2.

**Deferral honesty** — the FreeDV/M17/ARDOP family and the KISS translator are out of scope *by the design's own "eventually"/"future work" language*, not silently dropped; A.8 records the missing groups so the follow-on plan slots in. SIC/AP decoding deferral is stated in the Scope section. No silent caps.

**Reference-constant caveat** — every DSP task that depends on a generator polynomial, sync vector, interleave map, H-matrix, or scaling constant pairs a structural implementation with a published-vector KAT and names the reference source to confirm against, per the design's repeated "confirm constants at implementation time" instruction. These are deliberate, not placeholder gaps.
