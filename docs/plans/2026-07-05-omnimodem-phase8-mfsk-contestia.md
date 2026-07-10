# Phase 8 — MFSK family + Contestia grid, fldigi parity

> Instantiates the per-mode port template (T1–T9) from
> `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md` for the
> MFSK family and the Contestia grid. References:
> `fldigi/src/mfsk/{mfsk.cxx,mfskvaricode.cxx,interleave.cxx}`,
> `fldigi/src/contestia/contestia.cxx`, `fldigi/src/filters/viterbi.cxx`,
> `fldigi/src/misc/misc.cxx` (upstream 4.1.23 @ 61b97f413).

**Goal:** port fldigi's MFSK submodes and the Contestia submode grid into
omnimodem, verified against golden vectors extracted from the unmodified fldigi
units. MFSK is the named prerequisite for **Phase 13 (RSID)** — "so there is a
mode to identify." The MFSK/THOR/IFKP/FSQ **picture** sub-protocol (`mfsk-pic.cxx`)
is out of scope here and lands in **Phase 15**.

## What MFSK is

M-ary FSK where the data path is: **MFSK Varicode → NASA K=7 rate-1/2
convolutional code → diagonal interleaver → gray-coded tone**. `numtones ==
2^symbits`, so each MFSK symbol carries `symbits` code bits (two code bits per
data bit — poly1 parity then poly2). The interleaver width is `symbits`; its depth
is per-submode. ref: `mfsk.cxx:919-961` (`sendsymbol`/`sendbit`/`sendchar`),
`mfsk.h:54-56` (`NASA_K=7`, `POLY1=0x6d`, `POLY2=0x4f`).

fldigi's `grayencode` (`misc.cxx:123`) is the full XOR-cascade `d ^ (d>>1) ^ … ^
(d>>7)` — the *inverse* of the conventional `n ^ (n>>1)`, i.e. our
`fec::gray::gray_decode`. The RX inverse is our `gray_encode`. Getting this
backwards is the one subtle wire trap in the family.

Eleven submodes fix `(symlen, symbits, depth, samplerate)`; `numtones`, tone
spacing (`samplerate/symlen`), baud and bandwidth derive. ref: `mfsk.cxx:180-289`:

| submode | symlen | symbits | depth | numtones | rate | preamble |
|---|---|---|---|---|---|---|
| mfsk4 | 2048 | 5 | 5 | 32 | 8000 | 107 |
| mfsk8 | 1024 | 5 | 5 | 32 | 8000 | 107 |
| mfsk11 | 1024 | 4 | 10 | 16 | 11025 | 107 |
| mfsk16 | 512 | 4 | 10 | 16 | 8000 | 107 |
| mfsk22 | 512 | 4 | 10 | 16 | 11025 | 107 |
| mfsk31 | 256 | 3 | 10 | 8 | 8000 | 107 |
| mfsk32 | 256 | 4 | 10 | 16 | 8000 | 107 |
| mfsk64 | 128 | 4 | 10 | 16 | 8000 | 180 |
| mfsk128 | 64 | 4 | 20 | 16 | 8000 | 214 |
| mfsk64l | 128 | 4 | 400 | 16 | 8000 | 2500 |
| mfsk128l | 64 | 4 | 800 | 16 | 8000 | 5000 |

## What Contestia is

Olivia's faster sibling: an MFSK tone bank where each character is spread across a
**32-chip Walsh/Hadamard block** (a 5-bit symbol) rather than Olivia's 64. fldigi
drives the shared Olivia engine with `Tones = 2·2^idx`, `bandwidth = 125·2^bw`
(`contestia.cxx:268-325`); the grid is enumerated in `globals.h:57-64` — 19
submodes over tones ∈ {4,8,16,32,64} × bandwidth ∈ {125..2000} Hz.

## Method (doctrine)

Building blocks already present from earlier phases carry most of the family:

- **MFSK Varicode** — already ported and KAT-gated in `framing/varicode.rs`
  (`mfsk_encode`/`mfsk_decode`, Phase 7 for PSK-R). No new work; reused directly.
- **K=7 conv (0x6d/0x4f)** — `fec::conv::ConvCode`, already fldigi-verified
  (`k7_pskr_matches_fldigi_vector`). MFSK uses a *streaming* (never tail-flushed)
  encode; the framing flush drains it.
- **gray** — `fec::gray::{gray_decode,gray_encode}` (note the naming inversion).
- **fht** — `fec::fht::{walsh_encode,walsh_soft_decode}`, reused parametrically at
  N=32 for Contestia (exactly as `olivia` uses it at N=64).
- **MFsk modulate / fsk_util tone detector** — `frontend::modulate::MFsk`,
  `modes::fsk_util::{tone_powers,argmax}`.

New building block:

- **`fec::interleave::MfskInterleaver`** — the `size`-parametric generalisation of
  the existing `DiagInterleaver` (which is this with `size==2`, PSK-R). Ported
  verbatim from `interleave.cxx:57-90`; KAT-gated in isolation against the golden
  `symbols` field before any mode uses it.

### T1 — Golden vectors

`scratch/refvectors/mfsk_dump.cxx` + `build_mfsk.sh` compose the **unmodified**
fldigi `encoder` (`viterbi.cxx`), `interleave` (`interleave.cxx`) and `varienc`
(`mfskvaricode.cxx`) in the exact order `sendchar`→`sendbit`→`sendsymbol` use
them, with `parity`/`grayencode` inlined verbatim from `misc.cxx` (the same
technique `psk_robust_dump.cxx` uses for `parity`). Output → `tests/vectors/
mfsk.json`, one object per representative submode with the stage intermediates
(`varicode` / `coded` / `symbols` / `tones`) for break localisation.

Data portion only — the STX/EOT/preamble framing envelope + AFC are **deferred**,
matching the Phase-9 DominoEX / Phase-7 PSK-R precedent. The wire-determining
arithmetic (varicode/FEC/interleave/gray) is asserted bit-exact; the framing
envelope and cross-decode are the `#[ignore]` nightly gate.

### T2–T5 — Port

- `fec/interleave.rs::MfskInterleaver` (new block, KAT first).
- `modes/mfsk.rs`: `MfskVariant` enum + `params()`; `text_tones` (bit-exact TX
  chain); `MfskMod` (chain + flush → `MFsk`); `MfskDemod` (Goertzel tone →
  gray/interleave inverse → streaming Viterbi → MFSK Varicode reframe).
- `modes/contestia.rs`: `ContestiaVariant` grid; `ContestiaMod`/`ContestiaDemod`
  mirroring `olivia` at Walsh N=32.

### T6 — Daemon registry

`ModeConfig::{Mfsk,Contestia}` variants + `parse`/`to_mode_string`/`label`;
`registry.rs` demod/modulator arms; `grpc/service.rs` `Params::{Mfsk,Contestia}`
arms; proto `MfskParams`/`ContestiaParams` messages.

### T7 — Conformance gates

`tests/kat.rs` Group 11: `mfsk_tx_chain_matches_fldigi_vector` (bit-exact),
`mfsk_submode_loopback_and_awgn`, `contestia_grid_loopback_and_awgn`. The
bit-exact KATs are **also** plain lib unit tests (`modes::mfsk::tests`,
`fec::interleave::tests`) since CI does not enable `testutil`.

### T8 — TUI

All 11 MFSK submodes + all 19 Contestia submodes added to
`clients/omnimodem-tui/internal/app/modes.go` (`chat` shape), `modeParamsFor`
extended with the `Mfsk`/`Contestia` oneof arms, Go proto regenerated,
`TestAllDaemonModesAreExposed` + new `TestMfskModeParams`/`TestContestiaModeParams`.

### T9 — PR

Branch `feature/omnimodem-phase8-mfsk-contestia`; workspace `cargo build` +
`cargo test` (plain + `--features testutil`) green, TUI `go test ./...` green.

## Equivalence classes (Doctrine §3)

- **Bit-exact (asserted byte-for-byte):** MFSK Varicode bits, K=7 code bits,
  interleaved symbol stream, gray tone indices (`mfsk.json`).
- **FP / loopback (never bit-exact):** modulated audio — gated on the loopback
  decode (clean + light AWGN) only. Cross-decode against fldigi is the `#[ignore]`
  nightly gate.

## Deferred

- **MFSK picture sub-protocol** (`mfsk-pic.cxx`) → Phase 15.
- **TX framing envelope + AFC/sync** (preamble/STX/EOT, `synchronize`/`afc`) —
  the streaming loopback is the CI gate; full-envelope cross-decode is nightly.
- **Contestia real MTEXT charset + PRBS whitening** — the self-consistent charset
  mirrors the `olivia` precedent; the real alphabet is the cross-decode concern.

## Self-review

- Every listed submode is wired, registered, and table-tested (no partial grid).
- `numtones == 2^symbits` holds for all submodes (asserted).
- The interleaver is KAT-gated in isolation before the mode consumes it.
- The gray naming inversion (`grayencode` == `gray_decode`) is cited at every use.
