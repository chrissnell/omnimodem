# Reference conformance vectors

Golden vectors for the mode-parity ports (plan: `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md`).
Every mode we port from fldigi / WSJT-X / JS8Call is checked stage-by-stage against
vectors extracted from that upstream, so a regression localises to one pipeline stage
instead of "the decode is wrong somewhere".

## Two file formats

- **`*.snap`** — `insta` snapshots produced by `crates/dsp/tests/snapshots.rs`. Used for
  our own modulator/encoder golden output (e.g. `mfsk16.snap`, `diff_bpsk.snap`). The `---`
  header records the source test. Regenerate deliberately with `cargo insta review`.
- **`*.json`** — hand-authored / tool-extracted **reference** vectors with explicit
  per-record fields, one record per test message (e.g. `ft8_reference.json`, whose records
  carry `msg`, `payload`, `crc14`, `tones`). This is the format for cross-implementation
  vectors: the fields are the intermediate values at each stage boundary.

## Provenance is mandatory

A reference `*.json` vector file MUST begin with a provenance block (a leading `_meta`
record, or a sibling `<name>.provenance.txt`) naming:

1. the upstream project + **commit hash** it was generated from,
2. the exact reference file(s) the values come from (e.g. `wsjtx/lib/fst4/genfst4.f90`),
3. the exact command / driver program that produced it (see `scratch/refvectors/`).

Without provenance a vector is not a reference — it is just our own output, which proves
nothing about compatibility.

## Stage boundaries to capture (TX side)

Capture the value at every boundary so a break is localised (plan Doctrine §4):

| Stage | Field | Domain |
|---|---|---|
| source/char encode (varicode, JSC, Baudot, 77-bit pack) | `bits` / `payload` | **bit-exact** |
| FEC encode (conv / LDPC / RS / QRA) | `codeword` | **bit-exact** |
| interleave | `interleaved` | **bit-exact** |
| symbol / tone map | `tones` / `symbols` (indices) | **bit-exact** |
| modulated audio | `wav` (sample file ref) | **FP tolerance only** — never asserted bit-exact |

Integer/bit-domain fields are compared byte-for-byte. The audio field is compared within a
committed numerical tolerance (max abs error / correlation), because FP op-ordering and
libm `sin`/`cos` differ across Fortran/C++/Rust. See plan Doctrine §3.

## Extraction drivers

Per-toolchain driver programs live under `scratch/refvectors/` (not shipped):

- **fldigi (C++):** a small `main()` linking the mode's `.cxx` that prints the intermediates.
- **WSJT-X (Fortran):** reuse the shipped `*sim` / `*code` / `gen*` programs
  (`msk144sim`, `jt4code`, `genfst4`, `qratest`, …) — they already print intermediates.
- **JS8Call:** the `jsc` / `JS8` units for the JSC codec, `lib/js8*` for symbol vectors.

Each driver's invocation is recorded in the vector file's provenance block so the vector is
reproducible from the pinned upstream commit.
