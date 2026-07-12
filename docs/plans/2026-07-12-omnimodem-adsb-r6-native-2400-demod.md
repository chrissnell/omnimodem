# ADS-B R6 — native 2.4 MHz correlating demod core (match readsb/dump1090)

**Status:** Stage A shipped (opt-in, default off); Stages B/C outlined.
**Depends on:** R1–R5 (`crates/dsp/src/modes/adsb`, `adsb_bench`), merged to `main`.
**Base branch:** `main`.
**Origin:** the readsb architecture analysis in GRA-328.

## Why

R1–R5 optimized a structurally weaker core: **downsample 2.4→2.0 MHz → 2-tap
slot-mean slice → 12-phase interpolation ensemble → bespoke confidence gate**.
readsb/dump1090-fa instead demodulate **natively at 2.4 MHz** with a
**matched-filter correlating slicer** (6 samples per 5 symbols, five hand-tuned
FIR functions), pick the **best-scoring phase**, and accept via an **always-on
recently-seen-ICAO filter** with a precomputed syndrome error table. Ours is a
rung below; only a core rewrite closes the measured ~48%-of-dump1090 gap.

## Stage A — native-2.4 correlating slicer (dominant lever), CRC-clean acceptance — **SHIPPED**

New `crates/dsp/src/modes/adsb/demod2400.rs`, a faithful port of readsb
`demod_2400.c`:

- `slice_phase0..4` — the five hand-tuned FIR matched-filter correlators
  (coefficients verbatim from readsb, including the slightly DC-unbalanced ones).
- `slice_byte` — the 6:5 sample:symbol phase walk (byte stride 19/19/19/19/20,
  phase cycling 0→1→2→3→4→0), extracting a whole message byte per call.
- Preamble detection — readsb's cheap pre-check plus the five noise-relative
  preamble correlations (`base_noise * 58 / 32` threshold), each admitting one or
  two sub-sample data phases to slice.
- `Demod2400` — streaming best-of-phase demod that buffers across `feed` calls,
  emits each CRC-clean frame as the same `Packet` payload the R1–R5 core emits.

**Acceptance = CRC-clean only** (residual 0 → DF11/17/18 in practice). The
all-zero slice a flat/quiet stretch produces is rejected. Score-based acceptance
of the address-overlaid DFs (DF0/4/5/20/21) and 1-bit-corrected frames is Stage B.

**Wiring:** `adsb_bench --demod native` (default `legacy`) slices the `|I+jQ|`
magnitude at the **native capture rate with no resample**; `--front`,
`--work-rate`, `--phases`, `--min-conf`, `--repair`, `--roster` do not apply to
the native path. The shipping decoder and the CI regression gate are unchanged
(default `legacy`).

**Tested:** `demod2400.rs` unit tests on synthetic 2.4 Msps waveforms (modulate
at 12 samples/µs → light edge-rounding → decimate by 5): recovery at every
decimation sub-sample phase, DF11 short-frame recovery, exactly-once emission,
CRC-corrupted rejection, silence/noise rejection, and split-across-feeds
reassembly. Plus `adsb_bench` `--demod` CLI parse coverage.

### Stage A gate — owner-run (A6)

The KSLC reference capture and dump1090 are **not in the agent environment**, so
the head-to-head measurement is owner-run:

```
# native core, no resample, against the real 2.4 Msps capture
adsb_bench <ksslc-2400.iq> --demod native --baseline-frames 64 --json
# legacy baseline for the same capture, for the delta
adsb_bench <ksslc-2400.iq> --json
```

Keep Stage A only if `--demod native` moves clean-CRC yield toward the dump1090
baseline (the DF11 all-call gap is the expected lever). If it does not, revert
(Track A rule) — the machinery is opt-in and default-off, so nothing ships until
the number moves.

## Stage B — score-based acceptance (planned)

- Sorted syndrome→error table for 1-bit diagnosis (extend the existing
  `crc::locate_single_bit_error` into a precomputed table).
- Dual-buffer aging ICAO filter (readsb `icao_filter.c`), always-on.
- `score_message` (port of readsb `scoreModesMessage`) fusing CRC-error-count +
  filter membership; replaces the CRC-clean gate in `Demod2400`, recovering
  DF0/4/5/20/21 + 1-bit-corrected frames without inventing aircraft.
- Gate B5: owner-run, same recipe as A6.

## Stage C — daemon wiring (planned)

- Native 2.4 front end into the RX worker: NCO shift to DC (the daemon parks the
  signal ~600 kHz above hardware center to dodge the R820T DC spike; the native
  slicer wants it at DC), feed the un-resampled magnitude to `Demod2400`.
- Promote `native` to default, retire the R2–R5 ensemble once Stages A/B are
  measured as wins.
- Gate C3: owner-run.

## Correctness vs. measurement

Correctness is unit-tested on synthetic 2.4 Msps waveforms with no real capture
needed. Yield is a *measurement* gate on the real recording — owner-run at each
stage checkpoint (A6/B5/C3). The agent ships tested, opt-in machinery and the
recipe above; it does not claim a yield number it cannot measure.
