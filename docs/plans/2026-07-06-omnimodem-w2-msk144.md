# Phase W2 — MSK144 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. Steps use checkbox (`- [ ]`) tracking. Built from the cited reference files per the Porting Doctrine in `2026-07-02-omnimodem-full-mode-parity-implementation.md`.

**Goal:** Port WSJT-X **MSK144** (VHF meteor scatter) into omnimodem, bit-exact-compatible on the air. MSK144 is a **streaming, ping-buffered short-burst** mode (not the 15 s windowed grid): a 72 ms, 864-sample offset-MSK frame is transmitted repeatedly for the whole period, and the decoder pulls a single frame out of a short meteor ping. Ships as one PR.

**Architecture:** A new FEC block `fec/ldpc_msk144.rs` transcribes the LDPC**(128,90)** code + **CRC-13** verbatim from the reference and reuses the existing BP decoder (`Ldpc::from_systematic_sparse`). A new front end `frontend/msk.rs` holds the reusable offset-MSK/OQPSK primitives (half-sine pulse, continuous-phase MSK modulator, FFT analytic-signal bandpass, frequency shifter). The mode assembly `modes/msk144.rs` builds the `s8 + 48 + s8 + 80` = 144-tone frame + differential-MSK tone map, a streaming `Demodulator` (analytic signal over overlapping 7168-sample blocks → per-window frequency/time sync → reference matched filter + BP decode), and a `Modulator`. One arm each in `mode/{mod,registry}.rs` and the TUI `modes.go`.

**Reference:** WSJTX/wsjtx @ `ccdfaf3c1c109010d15399674ce278167cfde848`:
- `genmsk_128_90.f90` — TX: pack77 → `encode_128_90` → 144-bit channel vector → differential-MSK tone map.
- `encode_128_90.f90` + `ldpc_128_90_generator.f90` — CRC-13 append + systematic LDPC(128,90) encode.
- `ldpc_128_90_reordered_parity.f90` — sparse `Nm`/`nrw` Tanner graph for BP decode.
- `crc13.cpp` — boost `augmented_crc<13, 0x15D7>`.
- `msk144decodeframe.f90` — RX matched filter + sync-word discriminator + `bpdecode128_90` + CRC check + unpack.
- `msk144sync.f90` / `msk144_freq_search.f90` — coherent frequency/time sync via cyclic cross-correlation against the 42-sample sync template.
- `analytic.f90` / `tweak1.f90` — analytic-signal bandpass + frequency shift.
- `msk144sim.f90` — the continuous-phase 2-FSK audio synthesis (tone 0/1 → `freq ∓ baud/4`, 6 samples/half-symbol).
- `decode_msk144.f90` — the 7168-sample / 3584-step streaming block loop.

## Reference facts (extracted — do not re-derive)
- **Frame:** 144 half-symbols = `s8(8) + codeword(1:48) + s8(8) + codeword(49:128)`, `s8 = {0,1,1,1,0,0,1,0}`. NSPM = 864 samples (72 ms at 12 kHz), 6 samples/half-symbol, 2000 baud.
- **Message protection:** 77-bit pack77 → **CRC-13** (poly `0x15D7`, boost augmented, computed over the 77 bits zero-padded to 96) → 90 info bits → **LDPC(128,90)** (38 parity checks). Codeword = `[msg(90) | parity(38)]`.
- **Generator:** `g(38)`, 23 hex chars/row, MSB-first, last nibble contributes 2 bits (90 cols).
- **Tone map:** `bitseq` (±1) → differential offset-MSK per `genmsk_128_90.f90:110-118`, then polarity flip.
- **Waveform:** continuous-phase FSK, tone 0 → `freq − 500`, tone 1 → `freq + 500` (baud/4 = 500 Hz), phase carried across half-symbols.
- **RX:** analytic bandpass 1500 ± 900/1100 Hz → per-window freq search (`tweak1`) + cyclic cross-correlation of both sync words → carrier-phase derotate → half-sine matched filter → 144 soft symbols → sync-error discriminator (`nbadsync ≤ 4`) → normalise → `llr = −2·softbit/σ²` (σ=0.60; sign flip for omnimodem's positive⇒bit0 convention) → BP decode → CRC-13 check → message-type filter → unpack77.

## Tasks (T1–T9)
- [x] **T0/T1 — Golden vectors.** `scratch/refvectors/msk144/build_msk144.sh` links the unmodified `genmsk_128_90`/`encode_128_90` + boost `crc13`, injecting a controlled 77-bit message via a `packjt77` stub, dumping CRC-13 / 128-bit codeword / 144 tones. Committed to `tests/vectors/msk144_reference.json` (2 patterns) with provenance.
- [x] **T2/T3 — CRC-13 + LDPC(128,90) (bit-exact).** `fec/ldpc_msk144.rs`: `get_crc13`, `encode_128_90`, `encode_msk144`, `msk144_code()`. KAT vs the golden codeword + CRC-13 (byte-for-byte), plus `G·Hᵀ=0` self-consistency. Lib unit tests (CI runs without `testutil`).
- [x] **T4 — Modulator (tones bit-exact, audio FP).** `frontend/msk.rs::cpfsk_modulate` + `modes/msk144.rs::tones_from_codeword`. Tone map asserted bit-exact vs both golden vectors; audio gated by loopback.
- [x] **T5 — Streaming demodulator.** `frontend/msk.rs::Analytic`/`freq_shift_into` + `modes/msk144.rs` (`sync_search`, `decode_frame`, `Msk144Demod`). TX→RX loopback + AWGN decode-rate ≥ 0.9.
- [x] **T6 — Daemon registry.** `ModeConfig::Msk144 { freq_hz }`, parse/round-trip, streaming demod + modulator arms. Registry unit test.
- [x] **T7 — Conformance gates.** Bit-exact KAT + AWGN loopback in `kat.rs`; `#[ignore]` bidirectional cross-decode gate documented.
- [x] **T8 — TUI.** `msk144` streaming meteor-scatter sequencer row + Go test.
- [x] **T9 — PR.** Whole workspace + TUI green; single Phase-W2 PR.

## Notes / deviations
- The RX drives the **single-frame** sync path (per-window cyclic cross-correlation) rather than the full `msk144spd` squared-spectrum ping locator + multi-frame coherent averaging; the latter are decode-quality refinements (like FT8's SIC) and are deferred. This is sufficient for the loopback + AWGN gates on a continuous transmission.
- The reference `bpdecode128_90` is a tanh-domain BP decoder; omnimodem reuses its min-sum BP + the transcribed sparse graph. Decode is FP/soft-domain (gated on decode-rate), so this is within doctrine §3 — the bit-domain stages (CRC/LDPC generator/tone map) are still asserted bit-exact.
- MSK144's only tunable is the audio centre frequency; carried as a bare-label tail (`msk144:freq=1500`) like FST4's `tr`, so the TUI stays parameterless (default 1500 Hz).
