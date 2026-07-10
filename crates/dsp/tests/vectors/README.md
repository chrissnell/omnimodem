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
- **MMSSTV (SSTV, Borland C++Builder/VCL):** the reference won't build on Linux as-is, so
  the DSP core (`sstv.cpp` + `fir.cpp`) is compiled *unmodified* against a fake-`<vcl.h>`
  shim in `scratch/refvectors/sstv_shim/` (zero edits to the reference tree) and driven by
  `sstv_tx_dump.cxx` / `build_sstv_tx.sh`. The truly VCL-bound sequencing — the
  `TMmsstv::LineXXX` TX layouts and `DrawSSTVNormal` RX raster assembly, which are methods of
  the GUI form class in `Main.cpp` — is **transcribed** into the driver with exact
  `// ref: Main.cpp:NNNN` cites and a byte-buffer bitmap in place of the VCL canvas. Each
  SSTV vector's provenance block lists what is *linked-unmodified* (timing tables, tone
  renderer, demod) versus *transcribed* (line layout, raster assembly, colour helpers).

  SSTV vectors split the two equivalence classes explicitly: the `symbols` field
  (per-write `[freq_hz, ms]` list) and the decoded pixel `raster` are **bit-exact**; the
  `pcm` field (VCO + BPF audio) is **FP-tolerance** (stats + checksum, never asserted
  bit-exact). MMSSTV ships no Linux CLI decoder, so the "reference decodes our TX" direction
  is substituted by feeding our TX audio back through the isolated `CSSTVDEM` harness.

Each driver's invocation is recorded in the vector file's provenance block so the vector is
reproducible from the pinned upstream commit.
