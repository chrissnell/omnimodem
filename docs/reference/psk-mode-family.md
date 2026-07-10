# PSK mode family — code map

Reference for the fldigi-compatible PSK family (Phase 7). Purpose: let a future
agent find the right code quickly. All modes are bit-exact-compatible with
fldigi 4.1.23 and cross-decode against it. Native samplerate is 8 kHz for every
mode here (`PSK_RATE`). 16 kHz modes (8PSK/16PSK/OFDM) are out of scope.

## Where everything lives

| Concern | Location |
| --- | --- |
| Mode table, mod/demod, submode enum | `crates/dsp/src/modes/psk.rs` |
| `PskVariant` (enum + `params()` + `from_label()` + `label()`) | psk.rs |
| `PskMod` (modulator) / `PskDemod` (demodulator) | psk.rs |
| `MultiCarrierRx` (nX front-end) + `McDecode` backend | psk.rs |
| `RobustRx` (K=7 two-phase Viterbi + deinterleave) | psk.rs |
| Two-stage decimating matched filter `wsinc_blackman` | `crates/dsp/src/frontend/fir.rs` |
| Carrier layout (`MultiCarrier`, spacing 1.4·sc_bw) | `crates/dsp/src/frontend/multicarrier.rs` |
| Convolutional FEC + streaming Viterbi | `crates/dsp/src/fec/conv.rs` |
| Diagonal interleaver (`DiagInterleaver`) | `crates/dsp/src/fec/interleave.rs` |
| PSK31 + MFSK (IZ8BLY) Varicode | `crates/dsp/src/framing/varicode.rs` |
| Daemon mode-string parse (delegates to `from_label`) | `crates/omnimodem/src/mode/mod.rs` |
| TUI mode list + drift-guard test | `clients/omnimodem-tui/internal/app/modes.go` (+ `_test.go`) |

fldigi reference source is checked out in-tree at `fldigi/src/psk/{psk.cxx,
pskvaricode.cxx,pskcoeff.cxx}`. Every ported table carries a `// ref: ...:<lines>`
cite back to it.

## The modes

- **Plain BPSK rates:** `psk31/63/125/250/500/1000` — differential BPSK + PSK31
  Varicode, no FEC.
- **QPSK rates:** `qpsk31/63/125/250/500` — 2 bits/symbol, K=5 FEC, detected
  non-coherently (Costas mis-locks the 4-fold symmetry).
- **Robust `+F`/PSK-R:** `psk63f` (no interleaver) and `psk125r/250r/500r/1000r`
  — K=7 FEC + MFSK Varicode + 2×2×idepth diagonal interleaver.
- **Multi-carrier robust `nX_PSKnnnR`:** `psk63rc{4,5,10,20,32}`,
  `psk125rc{4,5,10,12,16}`, `psk250rc{2,3,5,6,7}`, `psk500rc{2,3,4}` — the PSK-R
  core distributed over N frequency-offset carriers.
- **Multi-carrier uncoded `nX_PSKnnn`:** `psk125c12`, `psk250c6`, `psk500c2`,
  `psk500c4`, `psk1000c2` — plain BPSK + PSK31 Varicode, no FEC. Label rule:
  `c` = uncoded, `rc` = robust (distinct namespaces).

Param table (`symbollen/dcdbits/qpsk/robust/idepth/carriers`) is `PskVariant::params()`,
transcribed verbatim from `psk.cxx:382-884`.

## Multi-carrier receiver architecture (`MultiCarrierRx`)

Adjacent carriers are only 1.4·baud apart while each BPSK signal is ~baud wide,
so a full-rate matched filter cannot reject the neighbour — the in-band
inter-carrier interference (ICI) pushes odd carrier counts (and all uncoded
modes) over threshold. The fix is fldigi's **two-stage decimating windowed-sinc
matched filter** (`wsinc_blackman`, a byte-for-byte port of `wsincfilt`):

1. Per carrier, down-convert, then **fir1** (cutoff `1/symbollen`) decimating by
   `symbollen/mf_sps` to `mf_sps = min(16, symbollen)` samples/symbol.
2. **fir2** (cutoff `1/mf_sps`) on the decimated stream. Decimation makes fir2
   sharp relative to the symbol rate, which is what actually cuts the ICI.

Symbol timing is fldigi's **`bitclk` loop** (`psk.cxx:2058-2145`): a leaky
envelope histogram (`syncbuf`, `mf_sps` bins) over one symbol, with a clock
accumulator nudged by the lower-vs-upper-half amplitude imbalance. It tracks
transmitter clock drift (~±2 % tolerated) instead of a fixed grid.

Decode backend is the `McDecode` enum: `Robust(Box<RobustRx>)` (K=7 two-phase
Viterbi, per-phase MFSK framing, path-metric vote) for the FEC modes, or
`Bpsk { pending, synced, zrun }` (plain PSK31 `00`-framer over hard per-carrier
bits) for the uncoded modes. Carriers recombine round-robin (inverse of the TX
distribution). `sample_offset` on emitted frames is the **raw input-sample
count**, not the decimated index (the ensemble dedup window needs raw offsets).

## Golden-vector methodology

Bit-exact equivalence classes (varicode bits, FEC codewords, interleaver
permutations, symbol/phase indices) are asserted byte-for-byte against vectors
captured by compiling the **unmodified** fldigi source. Audio/matched-filter
output is FP-tolerance only, gated by loopback + AWGN decode tests — never assert
bit-exact audio.

- Reference drivers: `scratch/refvectors/build_psk_{varicode,qpsk,robust,interleave}.sh`
  (each links the specific `fldigi/src/psk/*.cxx` TUs; provenance header names the
  upstream commit + exact command).
- Captured vectors: `crates/dsp/tests/vectors/psk_{bpsk,qpsk,robust,mfsk,interleave}.json`.

## Tests

- **Lib unit tests** (`psk.rs mod tests`): per-mode loopback grids + the
  bit-exact vector KATs. These run under the default `cargo test`.
- **`testutil`-gated KATs** (`crates/dsp/tests/kat.rs`): loopback + AWGN gates.
  **CI does not enable `testutil`**, so any bit-exact reference KAT must ALSO
  exist as a plain lib unit test or it won't run in CI.
- Timing: `nx_tracks_clock_drift` proves the `bitclk` loop follows a ±1.5 %
  resampled (clock-offset) signal that a fixed grid would drop.

## Known follow-ups

- Symbol timing on the multi-carrier stream is drift-tolerant (bitclk) but the
  loop constants are fldigi's defaults; no per-mode tuning has been done.
- 8PSK/16PSK/OFDM (16 kHz) are not ported (out of Phase-7 scope).
