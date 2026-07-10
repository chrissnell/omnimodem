# ADS-B (Mode S) mode — design

Status: **Phase 1 landed** (DSP mode + KAT tests). Phases 2-4 planned.

Adds ADS-B / Mode S 1090 MHz reception to omnimodem: aircraft broadcasts
(identity, position, altitude) decoded from the RTL-SDR I/Q stream (GRA-293,
`docs/design/2026-07-06-rtl-tcp-sdr-input-design.md`) and surfaced as a live
flights table in the TUI.

## Why ADS-B is different from the audio modes

ADS-B is not an audio mode. It is 1 Mbit/s pulse-position modulation (PPM) at
1090 MHz: an 8 µs preamble (pulses at 0.0/1.0/3.5/4.5 µs) then a 56- or 112-bit
frame guarded by a 24-bit Mode S CRC. Recovering the 0.5 µs pulses needs ~2 MHz
of bandwidth — dump1090's 2 Msps — so it cannot come through a soundcard or a
narrowband FM radio. The only real source is a wideband SDR delivering I/Q, i.e.
the rtl_tcp path GRA-293 built.

Two consequences shape the design:

1. **Wideband magnitude input, no channelization.** The voice SDR path
   (`frontend/sdr_demod.rs`) tunes + decimates I/Q down to a 48 kHz audio
   channel. ADS-B does the opposite — it wants the *full* 2 MHz. It consumes a
   **magnitude** stream `|I + jQ|` at a 2 MHz `native_rate`; the daemon computes
   the magnitude from the capture `Cplx` and feeds it straight to the mode.
   Because slicing/preamble correlation are relative comparisons, the magnitude
   need not be normalized.

2. **Structured output, not text/packet.** A decoded frame is aircraft state
   (ICAO, callsign, lat/lon, altitude), which the flights table renders as
   columns. Rather than re-decode Mode S in the Go TUI, the daemon decodes once
   (Mode S fields + CPR even/odd pairing) and emits a typed `AircraftReport` on
   the event stream — mirroring the existing typed `Image` payload.

## Component design

```
rtl_tcp IQ (Cplx @2MHz) ──|·|──► AdsbDemod.feed(&[f32]) ──► Frame::Packet(mode_s)
                                                               │
                                             daemon: ModeS decode + CPR pairing
                                                               │
                                                     Event::AircraftReport ──► TUI flights table
```

- **`crates/dsp/src/modes/adsb/`** (Phase 1, landed):
  - `crc.rs` — dump1090-compatible Mode S 24-bit parity.
  - `ppm.rs` — `PpmModulator` / `PpmDemodulator` on an `f32` magnitude stream;
    `scan()` handles frames straddling `feed` chunk boundaries.
  - `message.rs` — `ModeS` field view (DF/ICAO/type-code/callsign),
    airborne-position CPR decode, barometric altitude, DF17 frame construction.
  - `adsb.rs` — `AdsbDemod` (`Demodulator`, streaming, 2 MHz native rate,
    emits `Packet` frames) and `AdsbMod` (`Modulator`, **loopback/self-test
    only** — 1090 MHz TX is illegal, this exists to drive the demod in CI and to
    render offline vectors).
  - KAT tests: the KLM1023 identification frame, CPR 52.2572/3.91937, 38000 ft,
    short/long frames, streaming split, corrupted-frame gating.

## Plan

- **Phase 1 — DSP mode (DONE).** `adsb` mode in `omnimodem-dsp`, KAT-tested,
  clippy-clean. Not yet daemon-registered.
- **Phase 2 — daemon wideband path.** A capture→mode binding that feeds full-rate
  magnitude to a mode whose `native_rate == capture_rate` (no `sdr_demod`
  channelization); register `ModeConfig::Adsb` in `mode::registry`. Depends on
  the GRA-293 capture seam.
- **Phase 3 — structured decode + proto.** Daemon decodes `Packet(mode_s)` into
  aircraft state, pairs CPR even/odd per ICAO, ages out stale contacts; additive
  `AircraftReport` event in `omnimodem.v1`.
- **Phase 4 — TUI flights table.** A `view` in `clients/omnimodem-tui` rendering
  flight / lat / lon / speed / altitude, updated from `AircraftReport`.

Live rtl_tcp reception lands with Phase 2; Phases 1/3/4 are testable against the
loopback modulator and recorded I/Q without hardware.
