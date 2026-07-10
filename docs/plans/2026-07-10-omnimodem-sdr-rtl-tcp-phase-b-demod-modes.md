# RTL-SDR Phase B — AM / WFM / SSB demod modes

**Status:** in progress
**Depends on:** Phase A (PRs #100/#104/#105/#106), merged to `main`.
**Base branch:** `main`.

## Goal

Fill in the AM, WFM and SSB (USB/LSB) demodulators for the `rtl_tcp` SDR input so
that `ConfigureSdr.demod_mode` works for every value of the already-shipped
`DemodMode` enum, not just NBFM. Phase A shipped the source, tuning, RF waterfall,
gRPC control surface and the NBFM demod; AM/WFM/SSB currently return `UNIMPLEMENTED`.

## Architecture — factoring

Phase A's `NbfmReceiver` (`crates/dsp/src/frontend/nbfm.rs`) hard-wires one
back-end (FM discriminator) behind the shared front-end:

```
raw IQ @ capture_rate
  → DownConverter (NCO channel-select, tuned offset → DC)
  → ComplexResampler (decimate capture_rate → channel_rate)
  → PowerSquelch (gate on channel power)
  → FmDiscriminator × gain  → audio @ channel_rate
```

Rather than clone this front-end into four new receivers, introduce a
mode-agnostic **`SdrDemod`** in the DSP crate that owns the shared front-end and
dispatches a per-mode back-end. `NbfmReceiver` stays as-is (its Phase-A tests keep
passing); the daemon's capture thread switches to `SdrDemod` and dispatches on
`demod_mode()`.

### `SdrDemod` (new `crates/dsp/src/frontend/sdr_demod.rs`)

```
pub enum DemodKind { Nbfm, Am, Wfm, Usb, Lsb }   // DSP-local; daemon maps its DemodMode → this

pub struct SdrDemod {
    nco: DownConverter,
    front: ComplexResampler,   // capture_rate → if_rate
    squelch: PowerSquelch,
    backend: Backend,          // per-mode
    audio: ComplexResampler-ish // if_rate → channel_rate (passthrough unless WFM)
}
```

- **IF rate.** All modes except WFM demodulate at `if_rate == channel_rate`
  (48 kHz), so the front decimates straight to the audio rate and the audio
  resampler is a pass-through. **WFM** needs a wider pre-detection bandwidth to
  pass ±75 kHz broadcast deviation, so `if_rate` is the largest multiple of
  `channel_rate` that is ≤ `capture_rate` and ≥ a ~180 kHz target (e.g.
  192 kHz for 48 kHz audio inside a 240 kHz capture). WFM discriminates at
  `if_rate`, then a real resampler brings the recovered *audio* down to
  `channel_rate`.

- **Back-ends** (operate on the decimated complex channel):
  - **Nbfm** — `FmDiscriminator × gain`, `gain = if_rate / (2π·deviation)`;
    optional de-emphasis stage (default OFF so AFSK/APRS twist is preserved).
  - **Am** — envelope `|z|` → DC block (one-pole high-pass removes the carrier
    term, leaving the modulation).
  - **Wfm** — `FmDiscriminator × gain` at the wide `if_rate` with a broadcast
    deviation constant, then decimate audio to `channel_rate`; optional
    de-emphasis (default OFF per the design table).
  - **Usb / Lsb** — sideband select by a one-sided **complex band-pass** around
    +BW/2 (USB) or −BW/2 (LSB) applied to the DC-centred channel, then take the
    real part → real audio. This is the "real part of the frequency-shifted
    channel" method with explicit sideband rejection.

- **De-emphasis** — new reusable one-pole low-pass (`Deemphasis`, τ configurable,
  default 75 µs US) added to `detector.rs`. Wired into the FM back-ends behind a
  builder toggle, **default OFF**. **DC block** (`DcBlock`, one-pole high-pass)
  also added to `detector.rs` for the AM path.

- **API** mirrors `NbfmReceiver`: `new(kind, capture_rate, channel_rate,
  offset_hz, deviation_hz, squelch)`, `retune`, `set_squelch`, `push_iq`.

### Daemon dispatch (`crates/omnimodem/src/audio/rtlsdr.rs`)

- Capture thread builds `SdrDemod` from `control.demod_mode()` instead of
  `NbfmReceiver`. Track `seen_mode` alongside `cur_rate`; on a generation change,
  rebuild the chain when **either** the capture rate **or** the demod mode
  changed, otherwise just `retune` + `set_squelch`.
- Add `DemodMode::to_dsp(self) -> DemodKind` mapping.

### Gate removal (`core/mod.rs`, `grpc/service.rs`)

- Delete the `mode != Nbfm ⇒ Unimplemented` branch in `configure_sdr`. Every
  defined mode is now selectable. `bias_tee` / `direct_sampling` stay gated
  (Phase C). Update the docstring.
- `grpc/service.rs` already lets defined modes through; no change beyond the
  comment.

## TDD steps — one mode at a time (AM → WFM → SSB)

Build/test env: `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0`. Never run
`cargo fmt`. Audio equivalence tests synthesize a modulated IQ signal per mode and
assert the modulating tone is recovered (zero-crossing / Goertzel), never
bit-exact audio.

1. **DC block + de-emphasis** (`detector.rs`): `DcBlock` removes a constant
   offset but passes a tone; `Deemphasis` attenuates a high tone more than a low
   one. Unit tests.
2. **`SdrDemod` scaffold + NBFM parity**: NBFM path through `SdrDemod` recovers a
   1200 Hz tone from synthetic FM IQ (mirror `NbfmReceiver`'s test) and squelch
   silences weak input. Proves the shared front-end + dispatch.
3. **AM**: synthesize `(1 + m·sin)·carrier` IQ at an offset; assert the tone is
   recovered and the DC term is gone (mean ≈ 0).
4. **WFM**: synthesize wide-deviation FM IQ; assert the tone is recovered through
   the wide IF + audio decimation.
5. **SSB**: synthesize a single-sideband tone; assert USB recovers it and LSB
   rejects it (and vice-versa) — proves sideband selection, not just detection.
6. **Daemon dispatch**: `rtlsdr.rs` capture test — an AM-modulated stream over the
   fake server delivers audio; a runtime mode change rebuilds the chain.
7. **Gate removal + gRPC round-trip**: `configure_sdr` succeeds for AM/WFM/USB/LSB
   and the mode is selectable end-to-end; update `sdr_control.rs` (the AM
   `UNIMPLEMENTED` assertion becomes success), keep bias-tee gated.

## Done criteria (per mode, non-negotiable)

Each mode closes only when (a) its demod recovers a known synthetic signal, (b)
its `UNIMPLEMENTED` gate is removed, and (c) a gRPC round-trip proves it selectable
via `ConfigureSdr.demod_mode`. Full `cargo test` and
`cargo clippy --all-targets -- -D warnings` clean before completion; open a PR.
