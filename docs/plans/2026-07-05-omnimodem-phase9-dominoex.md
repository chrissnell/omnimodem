# Phase 9 — DominoEX (IFK+ MFSK), fldigi parity

> Instantiates the per-mode port template (T1–T9) from
> `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md` for the
> DominoEX family. Reference: `fldigi/src/dominoex/{dominoex.cxx,dominovar.cxx}`,
> `fldigi/src/include/dominoex.h` (upstream 4.1.23 @ 61b97f413).

**Goal:** port fldigi's DominoEX submodes into omnimodem as bit-exact-compatible,
cross-decoding modes, verified against golden vectors extracted from the
unmodified fldigi tables. THOR (the DominoEX-core-plus-FEC family) is the natural
follow-up and is carried in **Phase 9b** (see "Deferred", below) — the IFK core
built here is the block it reuses.

## What DominoEX is

MFSK with **IFK+ (incremental frequency keying)**: each symbol is a *tone offset*
relative to the previous tone, not an absolute tone. For a 4-bit varicode symbol
(nibble) `n`, `tone = (prev + 2 + n) % 18`, where the `+2` guard keeps successive
tones ≥2 apart so a repeat is never sent (`dominoex.cxx:651-661`). Text is carried
by the **DominoEX Varicode** — a self-framing 1–3 nibble code where the first
nibble of a character has its MSB clear and continuation nibbles have it set, so
the stream is self-delimiting (`dominovar.cxx`, `dominoex.cxx:368-395,664-681`).

Nine submodes fix `(symlen, doublespaced, samplerate)`; tone spacing
(`samplerate·doublespaced / symlen`), baud (`samplerate / symlen`) and bandwidth
(`18·spacing`) derive from those (`dominoex.cxx:220-279`):

| submode | symlen | 2× | rate | baud | spacing Hz |
|---|---|---|---|---|---|
| dominoexmicro | 4000 | 1 | 8000 | 2.0 | 2.0 |
| dominoex4 | 2048 | 2 | 8000 | 3.9 | 7.8 |
| dominoex5 | 2048 | 2 | 11025 | 5.4 | 10.8 |
| dominoex8 | 1024 | 2 | 8000 | 7.8 | 15.6 |
| dominoex11 | 1024 | 1 | 11025 | 10.8 | 10.8 |
| dominoex16 | 512 | 1 | 8000 | 15.6 | 15.6 |
| dominoex22 | 512 | 1 | 11025 | 21.5 | 21.5 |
| dominoex44 | 256 | 2 | 11025 | 43.1 | 86.1 |
| dominoex88 | 128 | 1 | 11025 | 86.1 | 86.1 |

## Method (doctrine)

- **Golden vectors from the unmodified reference.** `scratch/refvectors/
  dominoex_varicode_dump.cxx` links the untouched `dominovar.cxx` and emits, for
  the whole primary alphabet and two fixed messages: the varicode nibble stream,
  the `decodeDomino` index + decoded char (round-trip), and the IFK+ tone
  sequence. Driver: `build_dominoex_varicode.sh`. Output committed to
  `crates/dsp/tests/vectors/dominoex_varicode.json` with a provenance header.
- **Two equivalence classes.** Bit-exact on the integer/bit domain (varicode
  nibbles, `varidec` indices, IFK+ tone indices) — asserted byte-for-byte.
  FP-tolerance on audio — gated on a loopback + AWGN decode, never bit-exact
  (Doctrine §3): fldigi's `sendtone` path is entangled with the FLTK/modem
  runtime.
- **Verbatim tables.** The `varicode[512][3]` encode table is transcribed
  verbatim with a `// ref:` cite. The redundant 4096-entry `varidecode` lookup is
  **derived by inverting** that verbatim table (each row's `decodeDomino` index →
  its character) and asserted to reproduce `dominoex_varidec` on all 256 primary
  rows (via the golden `idx`/`dec` columns) plus a full 512-row round-trip. The
  one reference quirk — secondary rows 123 and 125 both use `{5,10,12}`
  (`dominovar.cxx:78`) — is documented, not "corrected".
- **CI mirror.** `tests/kat.rs` is gated behind `testutil` (empty in CI), so every
  bit-exact KAT is also a plain lib unit test (`modes::dominoex::tests`,
  `framing::dominoex_varicode::tests`).

## Deliverables (this PR — T1–T8)

- **T1** golden vectors — `scratch/refvectors/dominoex_varicode_dump.cxx` +
  `build_dominoex_varicode.sh` → `tests/vectors/dominoex_varicode.json`.
- **T2** varicode codec — `crates/dsp/src/framing/dominoex_varicode.rs`
  (verbatim encode table + derived decoder + streaming `Framer`), bit-exact KATs.
- **T4** modulator — `modes::dominoex::DominoMod`: IFK+ tone-index sequence
  (bit-exact vs golden) rendered via `frontend::modulate::MFsk` at fldigi's tone
  frequencies (`center + (k − 8.5)·spacing`).
- **T5** demodulator — `modes::dominoex::DominoDemod`: Goertzel tone detection
  (`fsk_util`) → IFK+ inverse → varicode framing. Loopback + AWGN gated across all
  nine submodes. (Symbol-aligned like `olivia`; sync/AFC deferred — see below.)
- **T6** daemon — `ModeConfig::DominoEx`, parse/`to_mode_string`/label,
  `registry.rs` demod+modulator arms, `DominoParams` proto message + `effective_mode`.
- **T8** TUI — nine `modes.go` rows (chat shape), `modeParamsFor` DominoEX arm,
  regenerated Go proto, drift-guard + params tests.

There is no FEC/interleave stage (**T3**) for the on-air DominoEX text path; the
optional MultiPsk (`DOMINOEX_FEC`) secondary/soft path is deferred (below).

## Deferred

- **THOR family → Phase 9b.** THOR reuses this IFK core plus K=7 convolutional
  FEC + interleave + soft decode (`fldigi/src/thor/`). Split out because it adds a
  whole FEC stage and a distinct varicode (`thorvaricode`), and the issue permits
  the split when the core generalises cleanly (it does).
- **MultiPsk `DOMINOEX_FEC` secondary path → Phase 9b.** fldigi's optional
  FEC-coded secondary-text channel (`decodeMuPskEX`/`sendMuPskEX`,
  `dominoex.cxx:751-923`) is a separate toggle from on-air DominoEX; the secondary
  varicode alphabet is ported and round-trip-tested but not yet wired to a
  demodulator.
- **T7 cross-decode + BER curve.** The `#[ignore]` bidirectional cross-decode
  against the fldigi binary and the AWGN/Watterson decode-rate sweep are the
  nightly gate, sequenced with the reference-binary harness (mirrors the PSK/FT8
  `#[ignore]` gates). TX framing envelope (idle/STX/EOT preamble) and RX sync/AFC
  land with that work.

## Verification evidence

- `cargo test --workspace --locked` green (288 dsp lib + 168 daemon + integration).
- `cargo test -p omnimodem-dsp --features testutil` green (adds the two DominoEX
  reference-vector KATs).
- `cargo clippy --workspace --all-targets --locked -- -D warnings` clean.
- TUI `go test ./...` green (drift guard + DominoEX params).
