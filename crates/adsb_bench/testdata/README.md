# adsb_bench test data

## `adsb_ci_clip.iq` — ADS-B decode regression fixture

A small, bit-exact uint8 interleaved I/Q clip that drives the CI regression gate
in `../tests/regression_gate.rs`. It is **synthetic** — rendered by the loopback
modulator, never captured off the air (1090 MHz is protected aeronautical
spectrum).

Contents: 5 DF11 all-call replies + 3 DF17 identification squitters across 3
aircraft (`4BA956`, `A31A76`, `A6C88E`) → **8 CRC-valid frames**. That shape
mirrors the KSLC reference slice the bench was first calibrated against.

The gate asserts the decode yield stays at or above a floor, so a change that
silently drops frames (broken preamble detection, CRC, PPM slicer, or message
parse) fails CI. It runs as part of `cargo test --workspace`, so no dedicated CI
step is needed. It is a floor, not an exact match — a change that *increases*
yield is fine; ratchet the floors in `regression_gate.rs` up to match.

### Why the working rate, not 2.4 Msps?

The clip is written at the 2 MHz **working** rate, not the 2.4 Msps capture
rate. A 0.5 µs Mode S pulse is a single sample at 2 MHz; round-tripping that
impulse through a non-integer 2.0↔2.4 resample filters it below the demod's
preamble threshold. Writing at the working rate keeps the gate deterministic and
focused on the decode core. The 2.4→2.0 resample stage is covered separately by
the resampler's own unit tests and by real-capture benchmarking during the
R-phases.

### Regenerating

Bit-exact by construction: a seeded integer PRNG plus only IEEE-754 arithmetic
(no `sin`/`cos`, whose libm results vary by platform), no time or entropy.

```
cargo run -p adsb_bench --bin make_fixture
```

writes `adsb_ci_clip.iq` here. Rerunning reproduces identical bytes on any
IEEE-754 platform. If you change the fixture, update the floors in
`../tests/regression_gate.rs` to match.
