# Phase W4 — Port WSJT-X JT4 (legacy EME)

> Executable phase plan instantiating the T1–T9 per-mode template from
> `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md`. Read the
> Porting Doctrine there first. Roadmap line: `2026-07-02-omnimodem-fldigi-mode-parity.md` (W4).

**Goal:** JT4 (WSJT-X's legacy EME/weak-signal 4-FSK mode), submodes A–G, selectable
and operable end-to-end (daemon + TUI), with the bit-domain TX path verified bit-exact
against the unmodified reference.

**Reference:** `wsjtx/lib/{jt4.f90, gen4.f90, encode4.f90, entail.f90, encode232.f90,
conv232.f90, interleave4.f90, jt4_decode.f90, jt4sim.f90}` @ `ccdfaf3c1`.

**New building block:** none. JT4 reuses `fec::fano` (the shared K=32 r=1/2
Layland–Lushbaugh code — WSJT-X's `encode232`; polynomials `0xf2d05351 / 0xe4613c47`
match `conv232.f90`'s `npoly1/npoly2`) and the `frontend::modulate::MFsk` 4-FSK front
end already present for JT9, plus `framing::message77::legacy::{pack72,unpack72}`.

## What JT4 is (from the reference)

- **Message:** the legacy 72-bit pack (`packmsg`), same as JT65/JT9.
- **FEC:** `encode232` = K=32, r=1/2 convolutional, 72 data + 31 zero tail → 206 code
  bits. Identical to our `FanoCode::encode`.
- **Interleave:** `interleave4` — the 8-bit bit-reversal of `0..255` filtered to values
  ≤205, in ascending source order (`out[j0[i]] = code[i]`).
- **Sync:** `gen4` combines each interleaved code bit with the fixed pseudo-random `npr`
  vector: `tone(i) = 2*data(i) + npr(i+1)` — data is the MSB, sync the LSB, so the four
  4-FSK tones carry `(data,sync) = (0,0),(0,1),(1,0),(1,1)`. 206 channel symbols.
- **Waveform:** 12000 Hz, `nsps = 2742` (12000/4.375 truncated), keying rate ≈ 4.376
  baud, tone spacing = keying rate × `nch`, `nch = [1,2,4,9,18,36,72]` for submodes A–G
  (`jt4sim.f90`: `freq = f0 + itone*baud*nch(mode)`). 60 s on the minute.

## Equivalence classes (Doctrine §3)

- **Bit-exact:** the 206 channel-symbol values (Fano encode → interleave → npr sync).
  KAT `vectors/jt4_tones.json` vs a fixed 72-bit payload run through the unmodified
  reference (`build_jt4_tones.sh`). The `packmsg` 72-bit *layout* is NOT reproduced —
  our `pack72` is a self-consistent NBASE-relative port (as for JT9), so the payload is
  fed to the reference as raw 6-bit words to isolate the FEC/interleave/sync stages.
- **FP-tolerance / loopback:** the modulated audio; gated by a same-process loopback
  decode across all seven submodes, never asserted bit-exact.

## Tasks (T1–T9) — all landed

- [x] **T1 — Golden vectors.** `scratch/refvectors/{jt4_tone_dump.f90,build_jt4_tones.sh}`
  dump the 206 reference symbols for a fixed payload; committed to
  `crates/dsp/tests/vectors/jt4_tones.json` with a provenance header.
- [x] **T2 — Source codec.** Reuses `message77::legacy::{pack72,unpack72}` (already
  validated by JT65/JT9); no new code.
- [x] **T3 — FEC + interleave.** `FanoCode` (shared) + `jt4::interleave_table` (ported
  from `interleave4.f90`) + `NPR` sync (transcribed verbatim from `jt4.f90`). KAT
  `tx_symbols_match_reference_vector` asserts the 206 symbols byte-for-byte.
- [x] **T4 — Modulator.** `Jt4Mod`: 4-FSK via `MFsk`, spacing = keying rate × `nch`.
- [x] **T5 — Demodulator.** `Jt4Demod`: per-symbol soft data-LLR using the known sync
  bit (compare the data=0 vs data=1 tone power), de-interleave, `fano_decode`, `unpack72`.
  Loopback round-trip passes for all seven submodes.
- [x] **T6 — Daemon registration.** `ModeConfig::Jt4 { submode }` (labels `jt4a`…`jt4g`),
  `parse`/`to_mode_string`/`label`, `registry` demod+modulator arms. No proto param
  message (submode lives in the label, like the THOR/MFSK grids). Registry + parse tests.
- [x] **T7 — Conformance gate.** `kat.rs::jt4_cross_decode_doc` (`#[ignore]`) documents
  the bidirectional live cross-decode against the WSJT-X `jt4`/`jt4sim` binaries. The
  byte-exact TX KAT is a plain lib unit test, so it runs on CI without `testutil`.
- [x] **T8 — TUI.** `jt4a`…`jt4g` added to `clients/omnimodem-tui/internal/app/modes.go`
  as `sequencer` (60 s), same auto-sequence ladder as JT65/JT9. `go test ./...` green.
- [x] **T9 — PR.** Phase branch `feature/wsjtx-jt4`; workspace `cargo test` + TUI
  `go test ./...` green.

## Notes / deviations

- The exact WSJT-X on-air *message bits* are the `#[ignore]` cross-decode gate, matching
  the JT9 precedent — everything downstream of the 72-bit payload (the JT4-specific FEC,
  interleave, sync, and 4-FSK tone map) is bit-exact to `gen4`.
- No `ber.rs` sweep: consistent with the other JT-family modes (JT65/JT9), whose 60 s
  windows make an AWGN decode-rate sweep prohibitively slow for CI; the loopback gate +
  the byte-exact TX KAT are the committed floors.
