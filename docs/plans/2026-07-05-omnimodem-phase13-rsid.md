# Phase 13 — RSID (Reed-Solomon Identifier): detect and transmit

> Executable phase plan for GRA-265. Master plan: `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md`
> (read the Porting Doctrine + T1–T9 template there first). Roadmap line:
> `docs/plans/2026-07-02-omnimodem-fldigi-mode-parity.md` §Phase 13.

**Goal.** Port fldigi's RSID both directions: a **TX encoder** that emits the RSID
MFSK burst ahead of a transmission (identifying the active mode), and an **RX
detector** that identifies the mode + audio offset of a received burst and
surfaces it to the daemon. RSID is cross-cutting — it touches every mode's
mode-ID table and the daemon mode-switch path — so it is its own phase, not an
on-air text mode.

**Reference (checked out at `../fldigi`).**
- `fldigi/src/rsid/rsid.cxx` — the `cRsId` modem: `Encode` (RS(15,3)/GF(16)),
  `receive`/`search`/`CalculateBuckets`/`HammingDistance`/`search_amp`/`apply`
  (detect), `assigned`/`send`/`send_eot` (transmit).
- `fldigi/src/rsid/rsid_defs.cxx` — the two ID tables (`RSID_LIST`,
  `RSID_LIST2`) mapping rsid code ↔ tag ↔ fldigi mode.
- `fldigi/src/include/rsid.h` — constants (`RSID_SAMPLE_RATE 11025`,
  `RSID_FFT_SIZE 1024`, `RSID_ARRAY_SIZE 2048`, `RSID_NSYMBOLS 15`,
  `RSID_NTIMES 30`, `RSID_FFT_SAMPLES 512`, `RSID_SYMLEN 1024/11025`).

**New building block.** `crates/dsp/src/frontend/rsid.rs` — RSID both directions.

## The RSID wire format (what we replicate)

- **RS code → 15 symbols.** A 10-bit rsid code is split into 3 nibbles
  (`code>>8`, `(code>>4)&0xF`, `code&0xF`); positions 3..15 start at 0; then 12
  rounds of systematic RS-over-GF(16) encoding using the `Squares` GF-multiply
  table + the 12 generator roots `indices[]`. Output = 15 4-bit symbols = the
  on-air tone indices. **Bit-exact** (ref: rsid.cxx:184-196).
- **TX burst.** 5 symbol-periods of silence, then 15 tones at
  `fr + tone*11025/1024` where `fr = tx_offset - 11025*7/1024`, continuous
  phase, each tone `floor(RSID_SYMLEN*sr)` samples long; ESCAPE-prefixed extended
  codes send a 2nd 15-tone burst after 10 symbol-periods of silence; 5 trailing
  silent periods (ref: rsid.cxx:952-1092). Audio is **FP-tolerance only**.
- **RX detect.** Resample RX audio to 11025; 2048-pt Hamming-windowed FFT hopped
  by 512 samples; per candidate base-bin, `CalculateBuckets` reduces each FFT
  frame to the peak tone-slot (0..15) over a 16-bin window; a 30-deep time ring
  is matched (Hamming distance over the 15 symbols sampled at odd time-rows)
  against every code in the table; a distance ≤ resolution wins. Reported
  frequency `= (bin + 15 - 0.5)*11025/2048` (ref: rsid.cxx:198-740).

## Tasks (T1–T9 instantiated)

- [x] **T1 — Golden vectors.** `scratch/refvectors/rsid_dump.cxx` +
  `build_rsid.sh` transcribe the RS encoder + copy both ID tables verbatim; emit
  the 15 RS symbols for every entry → `crates/dsp/tests/vectors/rsid.json`
  (204 entries + provenance `_meta`). Bit-exact reference for the encoder + table.
- [x] **T2/T3 — RS encoder + ID table.** Port `Squares`/`indices`/`Encode`
  verbatim; the two ID tables with the omnimodem mode-string each code maps to
  (`None` for known-but-unported). **KAT (lib unit test):** `encode(code)` ==
  `rsid.json.syms` for all 204 entries, byte-for-byte.
- [x] **T4 — TX encoder.** `burst_for_mode(mode, tx_offset, sr) -> Vec<f32>` and
  `encode_burst`. KAT: tone-index sequence bit-exact vs the encoder; a loopback
  gate (see T5) covers the audio.
- [x] **T5 — RX detector.** `RsidDetector::feed(&[f32]) -> Vec<RsidDetection>`
  (tag, mode, freq, extended), incl. the ESCAPE→secondary-table two-burst
  sequence. **Loopback KAT:** synth a burst for a known mode at a known offset →
  detector recovers the tag/mode and `freq ≈ offset` (within RSID_PRECISION).
- [x] **T6 — Daemon wiring.** RX detector tap in `rx_worker` behind a per-channel
  `RsidControl` (mirrors `SpectrumControl`); a `TelemetryEvent::RsidDetected`
  surfaced through gRPC (`proto Event.rsid_detected`, `convert.rs`). TX: a
  per-channel `rsid_tx` flag on the channel config; `tx_worker` prepends the
  active mode's burst when set. `ConfigureRsid`/`ConfigureChannel` plumbing.
- [x] **T8 — TUI.** Surface a detected RSID (toast/status) and a per-channel RSID
  RX/TX toggle in the operate view; `go test ./...` green.
- [x] **T9 — PR.** Workspace `cargo test` + TUI `go test` green; open the phase PR.

## Equivalence classes (Doctrine §3)

- **Bit-exact:** RS symbols (T2/T3 KAT vs `rsid.json`); TX tone-index sequence;
  the recovered mode tag on loopback.
- **FP tolerance:** the modulated burst audio and the reported detect frequency
  (asserted within a few Hz / RSID_PRECISION, never bit-exact).

## Notes

- CI does not enable `testutil`, so the encoder KAT is a plain `#[cfg(test)]` lib
  unit test embedding the reference vector (Doctrine §5).
- RSID is not a selectable `ModeConfig` — it is a detector/annotator + a TX
  preamble option — so it adds no `registry.rs` arm; instead it plumbs a
  per-channel enable + a control event, and the ID table is validated against
  `ModeConfig::parse` in an omnimodem unit test.
