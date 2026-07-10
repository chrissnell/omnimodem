# Phase 11 — MT63 (64-carrier overlapping-Walsh OFDM) — Executable Plan

> Instantiates the T1–T9 per-mode port template from
> `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md` for the MT63
> family. Reference: `fldigi/src/mt63/{mt63.cxx, mt63base.cxx, dsp.cxx}` +
> `symbol.dat` / `mt63intl.dat` (fldigi 4.1.23 @ 61b97f413). Read the Porting
> Doctrine in the master plan first.

## What MT63 is (from the reference)

MT63 is a 64-carrier OFDM modem carrying **7-bit ASCII** with a **Walsh (64-point)
FEC spread** and a **deep block interleaver**, differentially BPSK-modulated per
carrier and heavily windowed/overlap-added for ISI immunity. Per the reference:

- **Encoder** (`MT63encoder`, mt63base.cxx:403-432): a character `c` (masked to
  7 bits) drives one ±1 spike into a 64-element buffer (`+1` if `c<64`, `-1` at
  `c-64` otherwise), an **inverse Walsh transform** spreads it across all 64
  carriers, the sign of each spread value is a bit, and a **block interleaver**
  (patterns `ShortIntlvPatt`/`LongIntlvPatt`, mt63intl.dat) permutes bits over
  `Intlv` = 32 (short) or 64 (long) past symbols. Output: 64 bits, `1` = no phase
  flip, `0` = phase flip. **This is the sole novel bit-domain stage.**
- **Modulator** (`MT63tx`, mt63base.cxx:249-329): each carrier is DBPSK — a
  running FFT-twiddle phase index `TxVect[i]` (0..511) advances by a fixed
  per-carrier `dspPhaseCorr[i]` every symbol, plus `FFT.Size/2` (a π flip) when
  the encoder bit is 0. Even/odd carriers are IFFT'd into two 512-pt windows
  staggered by half a symbol (`SymbolSepar/2` = 100 baseband samples), windowed by
  `SymbolShape` (symbol.dat) and overlap-added, then interpolated ×`DecimateRatio`
  and band-shifted to the audio carrier by the anti-alias comb filter.
- **Geometry** (symbol.dat): `SymbolLen`=512 (FFT + window), `SymbolSepar`=200
  baseband samples/symbol, `DataCarrSepar`=4 FFT bins between carriers. Bandwidth
  sets `DecimateRatio` (500→8, 1000→4, 2000→2) and `FirstDataCarr`; the FFT stays
  512. At 1000 Hz: 10 symbols/s, 15.625 Hz carrier spacing.
- **Submodes**: bandwidth ∈ {500,1000,2000} × interleave ∈ {short=32, long=64} =
  six modes (MT63-500S/L, 1000S/L, 2000S/L). Parametric over bandwidth +
  interleave depth.

## Two equivalence classes (Doctrine §3)

- **Bit-exact (integer domain):** the `MT63encoder` output bits and the `TxVect`
  per-carrier phase indices (0..511) — these fully determine the DBPSK
  constellation on the wire. Asserted byte-for-byte vs golden vectors extracted
  from the *unmodified* reference (`tests/vectors/mt63.json`,
  `scratch/refvectors/{mt63_dump.cxx,build_mt63.sh}`). The reference encoder is
  re-Preset with `RandFill=0` so it is a pure function of message+config (the live
  modem's rand() interleaver prefill is an anti-strong-carrier startup measure,
  irrelevant to the codec — the differential decoder recovers regardless).
- **FP tolerance / loopback (audio):** the windowed dual-IFFT overlap-add and the
  anti-alias/interpolation are ported faithfully but the audio is gated on a
  loopback decode, never asserted bit-exact — matching the DominoEX/Olivia/PSK
  precedent (fldigi's audio path is entangled with the FLTK/modem runtime, and
  op-ordering/libm differences make sample-exact audio unrealistic — Doctrine §3).

## Tasks

- [x] **T1 — Golden vectors.** `scratch/refvectors/mt63_dump.cxx` +
  `build_mt63.sh` link `mt63base.cxx`+`dsp.cxx` and dump, for MT63-500S/1000S/
  1000L/2000S on a fixed message, the per-char 64-bit encoder output and the 64
  `TxVect` phase indices. → `tests/vectors/mt63.json` (provenance header).
- [x] **T2/T3 — Encoder (Walsh + interleave).** `frontend/ofdm.rs`:
  `walsh_inv_trans`/`walsh_trans`, `SHORT/LONG_INTLV_PATT`, `SYMBOL_SHAPE`, and
  `Mt63Encoder` (bit-exact). Plain-lib KAT: encoder output == golden vector (CI
  does not enable `testutil`, so the bit-exact KAT is *also* a lib unit test).
- [x] **T4 — Modulator.** `Mt63Modulator` phase accumulation → `TxVect` indices
  (bit-exact KAT vs golden vector), then IFFT + `SymbolShape` overlap-add +
  interpolation + up-conversion → audio (FP, loopback-gated).
- [x] **T5 — Demodulator.** Down-convert + decimate → per-symbol 512-pt FFT on the
  staggered even/odd slices → per-carrier differential BPSK soft metric →
  `Mt63Decoder` (deinterleave + Walsh, `CarrOfs=0`) → char. Loopback round-trip
  at high SNR across all six submodes. Full ±8-carrier FEC-scan sync/AFC
  (`SyncProcess`) deferred, per the streaming-mode precedent; the fldigi
  cross-decode is the `#[ignore]` gate.
- [x] **T6 — Daemon registration.** `modes/mt63.rs`, `modes/mod.rs`,
  `mode/{mod.rs,registry.rs}` arms, `proto` `Mt63Params`.
- [x] **T7 — Conformance.** Table-driven submode param test; loopback across all
  six; `#[ignore]` cross-decode note.
- [x] **T8 — TUI.** Six `mt63_*` rows (chat shape) in `internal/app/modes.go`,
  `modeParamsFor` arm, proto regen, Go test. `go test ./...` green.
- [x] **T9 — PR.** Workspace `cargo test` + TUI `go test` green → open the phase PR.

## Reuse

`num_complex`/`rustfft` for the IFFT; `frontend::nco`-style up/down-conversion;
daemon `mode/{mod,registry}.rs`; the TUI `chat` shape. The OFDM engine in
`frontend/ofdm.rs` is written parametrically so Phase 16's OFDM data modes reuse
it.
