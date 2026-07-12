# Mode catalog

Every mode omnimodem implements, what it is, its submodes, and where the code lives.
Mode implementations are in [`../../crates/dsp/src/modes/`](../../crates/dsp/src/modes/)
(paths below are relative to that directory). How a mode string/params selects an
implementation is in [`dsp-building-blocks.md`](dsp-building-blocks.md#registering-a-mode-daemon-side);
the gRPC parameters are in [`../grpc-api.md`](../grpc-api.md#mode-parameters).

Output type: **text** (UTF-8 in `RxFrame.data`), **packet** (opaque bytes in
`data`), or **image** (raster in `RxFrame.image`).

## Packet

| Mode | File | What it is | RX/TX | Output |
|---|---|---|---|---|
| AFSK 1200 (AX.25) | `afsk1200.rs` | Bell-202 1200-baud AFSK, NRZI/HDLC, AX.25 — the baseline packet mode; drives the multi-slicer ensemble and the KISS bridge | Both | Packet |

## Surveillance (wideband SDR)

Not an audio mode: consumes a wideband magnitude stream from the rtl_tcp SDR
capture (no channelization), not a soundcard. See
[`../design/2026-07-10-adsb-mode-design.md`](../design/2026-07-10-adsb-mode-design.md).

| Mode | File | What it is | RX/TX | Output |
|---|---|---|---|---|
| ADS-B (Mode S) | `adsb/` | 1090 MHz 1 Mbit/s PPM extended squitter; 8 µs preamble, 56/112-bit frames, 24-bit CRC, CPR position + barometric altitude + TC 19 velocity. DSP + KATs and the ICAO-keyed `tracker` (CPR pairing, callsign, altitude, velocity, age-out) landed; the typed `AircraftReport` event is emitted by `core::adsb::AdsbReporter`. Daemon capture wiring (Phase 2) and the TUI flights table (Phase 4) are still open. TX is loopback self-test only (1090 MHz TX is illegal). | RX (+loopback) | Packet → aircraft |

**Decoder benchmark (the ruler).** [`../../crates/adsb_bench/`](../../crates/adsb_bench/)
replays a raw uint8 I/Q recording (2.4 Msps, as `rtl_tcp` streams it) through the
demod and reports CRC-valid frames, unique aircraft, and DF/type-code histograms
— the yardstick the decoder-quality phases (R1–R5) are measured against. Its
`--front` flag picks the front end: `complex` (default) band-limits and decimates
the I/Q 2.4M→2.0M before taking the magnitude; `mag` takes the magnitude at the
capture rate then decimates the envelope (the path the daemon ships today). The
`complex` front end avoids aliasing the sharp Mode S pulse edges that magnitude
decimation folds back, and is the R1 improvement. `ComplexResampler`'s anti-alias
lowpass is centered at DC, so `complex` assumes the signal sits near DC — true of
the `rtl_sdr` reference recording (tuned to 1090 MHz) but **not** of a
daemon-produced capture, which the tuner parks ~600 kHz above center to dodge the
R820T DC spike (`RawMag` bypasses the NCO). Porting `complex` into the daemon
needs an NCO shift to DC first.

**Decoder-quality phases (measured on the reference recording).** The slicer lives
in [`adsb/ppm.rs`](../../crates/dsp/src/modes/adsb/ppm.rs) and its tuned constants
in [`adsb.rs`](../../crates/dsp/src/modes/adsb.rs). R2 replaced the strict preamble
correlator with a noise-floor-relative detector (`PpmDemodulator::preamble_ok`). R3
added the multi-phase slicer ensemble (`ParallelDemodulator`, `ADSB_SLICER_PHASES`)
that runs the demod over sub-sample timing phases and unions the decodes. R4 added a
soft-decision accept/reject gate: `soft_confidence` scores each candidate's mean
per-bit eye (matched-filter pulse metric normalized by a decision-feedback AGC),
and `ADSB_MIN_CONFIDENCE` rejects the CRC-lucky ghosts a wide ensemble would
otherwise admit — which let the phase count rise from 4 to 12 (25 → 31 CRC-valid
frames on the reference, same three aircraft as readsb). The gate reads only the
eye, never the ICAO address, so a strong signal of any origin passes; the ghost it
drops is address-independently identifiable (a lone DF18 hit with reserved control
field CF 7 and the lowest eye, vs 8–13 coherent frames per real aircraft).
`adsb_bench --min-conf 0` disables the gate to show which frames it drops, `--dump`
lists every frame (df/tc/icao/conf/bytes) for per-frame audit, and `--phases N`
sweeps the ensemble width. Gate confidence rides on every `FrameMeta.confidence`.

R5 closes the measured gap to dump1090 with three levers, all **off by default**
(the daemon and CI gate are unchanged) and promoted only once shown to move the
real-capture yield. *Lever 1 — native-rate decode* (`--work-rate 4000000`,
`ADSB_NATIVE_RATE`): the whole PPM stack is rate-parameterized, so instead of
band-limiting the 2.4 Msps capture down to 2.0 MHz (whose anti-alias lowpass smears
the 0.5 µs pulse edges and costs weak/short-frame sensitivity — the DF11 gap) the
front end resamples *up* to 4 MHz, preserving the full captured bandwidth so the
slicer sees the un-smeared pulse. `AdsbDemod::with_rate_phases_min_conf` builds the
demod at the chosen rate; 2.4 MHz cannot be used directly (a half-µs slot must be a
whole number of samples). *Lever 2a — single-bit CRC repair* (`--repair`):
`crc::locate_single_bit_error` finds a *unique* single-bit error via the GF(2)
syndrome (dump1090's default `--fix`), which `ppm::PpmDemodulator::classify`
corrects when the flip preserves the frame's short/long length class; two-bit
search is deliberately omitted (it fabricates frames). *Lever 2b — ICAO-roster recovery* (`--roster`): the
address-overlaid DFs (DF0/4/5/16/20/21) fold their ICAO into the parity, so a
correct frame checksums to the address — nothing to validate. `IcaoRoster` holds the
addresses seen in clean DF11/17/18 decodes, and an overlaid frame is accepted only
when its recovered address is on the roster (the confidence gate still applies), so
surveillance frames are recovered without inventing aircraft from noise. The gate
decision (keep or revert each lever) is made on the reference capture via
`adsb_bench --baseline-frames/--baseline-aircraft`; wiring the kept levers into the
daemon is the production follow-up.

**R6 replaces the demod core** ([`adsb/demod2400.rs`](../../crates/dsp/src/modes/adsb/demod2400.rs),
`Demod2400`) — the R1–R5 path (downsample 2.4→2.0, 2-tap slot-mean slice, 12-phase
interpolation ensemble) was structurally a rung below readsb/dump1090-fa. The native
core is a faithful port of readsb `demod_2400.c`: it demodulates **at the 2.4 Msps
capture rate with no resample**, slicing each bit with one of five hand-tuned FIR
matched-filter correlators (`slice_phase0..4`) on a 6-samples-per-5-symbols phase
walk (`slice_byte`), and keeps the best-scoring of five preamble phases. Acceptance
is readsb's `scoreModesMessage` model (`score_candidate`): a fused CRC + recently-
seen-ICAO-roster score, **not CRC alone**. That is load-bearing — the Mode S CRC is
shift-invariant (`2·m = g·2q`), so a 1-bit-misaligned slice of a real frame usually
also checksums clean and lands on an address-overlaid DF (a real DF11 right-shifted
is a clean DF5); the roster gate rejects the ghost (its "address" was never seen in
the clear) while recovering genuine DF0/4/5/16/20/21 surveillance replies from
aircraft already seen via a clean DF11/17, plus single-bit-corrected frames (DF bits
excluded from the fix, as readsb does). `Demod2400` is the **daemon default**
(`registry.rs`, `ModeConfig::Adsb`): its `caps().native_rate` is the capture rate, so
the RX worker feeds the magnitude untouched — ~8× cheaper than the resample-then-
ensemble path (measured ~20× real-time vs ~2.5× in release), which also fixes the
rtl_tcp capture overruns the legacy path caused. Measure it against the legacy core
with `adsb_bench --demod native` vs `--demod legacy` on the reference capture.

## WSJT-X weak-signal family (windowed / time-aligned)

Block-demod modes that buffer a time slot and decode multi-pass. LDPC or K=32
convolutional FEC; require an accurate host clock (surfaced as `ClockOffset`).

| Mode | File | What it is | Submodes | RX/TX | Output |
|---|---|---|---|---|---|
| FT8 | `ft8.rs` | 8-GFSK, 79 symbols, 15 s slot, LDPC(174,91)+CRC-14, Costas-array sync | — | Both | Text |
| FT4 | `ft4.rs` | 4-GFSK, 7.5 s slot, LDPC(174,91)+CRC-14 | — | Both | Text |
| FST4 | `fst4.rs` | LF/MF 4-GFSK, LDPC(240,101) | (long T/R periods) | Both | Text |
| JT4 | `jt4.rs` | 4-FSK, K=32 Fano, 72-bit legacy msg, 60 s | A–G (tone spacing) | Both | Text |
| JT65 | `jt65.rs` | 65-FSK, RS(63,12) over GF(2⁶) soft-decode, 60 s | — | Both | Text |
| JT9 | `jt9.rs` | 9-FSK, K=32 Fano, 60 s | — | Both | Text |
| WSPR | `wspr.rs` | 4-FSK beacon, 162 symbols, K=32 Fano, ~110 s | — | Both | Text |
| JS8 | `js8.rs` | FT8-derived keyboard/ARQ mode, LDPC | Normal / Fast / Turbo / Slow | Both | Text |
| MSK144 | `msk144.rs` | Meteor-scatter offset-MSK, LDPC(128,90) | — | Both | Text |

## fldigi keyboard modes (streaming text)

Continuous streaming demods carrying Varicode/Baudot text. `center_hz` is the audio
carrier for most; parametric families take a `submode` label.

| Mode | File | What it is | Submodes / variants | RX/TX | Output |
|---|---|---|---|---|---|
| PSK31 | `psk31.rs` (wraps `psk.rs`) | Differential BPSK, 31.25 baud, PSK31 Varicode | — (legacy wrapper) | Both | Text |
| PSK family | `psk.rs` | Differential BPSK/QPSK PSK family | psk31/63/125/250/500/1000 + QPSK variants | Both | Text |
| RTTY | `rtty.rs` | 2-FSK, 5-bit Baudot/ITA2, start/stop framed | configurable baud/shift/center/reverse | Both | Text |
| CW | `cw.rs` | Morse; adaptive squelch; PARIS-timing fuzzy decoder | parametric wpm / tone | Both | Text |
| Olivia | `olivia.rs` | MFSK tone bank + 64-chip Walsh/FHT soft decode | tones × bandwidth grid (default 32/1000) | Both | Text |
| Contestia | `contestia.rs` | Olivia's sibling, 32-chip Walsh | tones × bandwidth grid | Both | Text |
| MFSK | `mfsk.rs` | M-ary FSK + K=7 conv + diagonal interleave + MFSK Varicode | mfsk4/8/11/16/22/31/32/64/128 + 64L/128L | Both | Text |
| DominoEX | `dominoex.rs` | 18-tone IFK+ (differential), DominoEX Varicode | micro/4/5/8/11/16/22/44/88 | Both | Text |
| THOR | `thor.rs` | 18-tone IFK+ + conv FEC + interleave + soft decode | micro/4/5/8/11/16/22/25x4/50x1/50x2/100 | Both | Text |
| IFKP | `ifkp.rs` | 33-tone IFK + self-framing IFKP Varicode | slow / normal / fast | Both | Text |
| FSQ | `fsq.rs` (+ `fsq/directed.rs`) | 33-tone IFK + CRC8-keyed directed (FSQCALL) protocol | fsq-1.5 / 2 / 3 / 4.5 / 6 | Both | Text |
| Throb | `throb.rs` | Dual-tone MFSK under a pulse envelope | throb1/2/4, throbx1/2/4 | Both | Text |
| MT63 | `mt63.rs` | 64-carrier overlapping-Walsh OFDM + deep interleave | 500/1000/2000 × short/long | Both | Text |

`ifk33.rs` and `fsk_util.rs` are shared helpers (33-tone IFK core; Goertzel tone
power / soft-demap) used across several of these families.

## Image / facsimile modes (raster output)

Populate `RxFrame.image` (see [`../grpc-api.md`](../grpc-api.md#rasters-receiving-and-sending-images)),
appending one on-air column per row incrementally.

| Mode | File | What it is | Submodes | RX/TX | Output |
|---|---|---|---|---|---|
| Hellschreiber | `hell.rs` | Column-scan facsimile, bitmap font, OOK/2-FSK | feldhell / slowhell / hellx5 / hellx9 / hell80 | Both | Image |
| SSTV | `sstv.rs` | Analog FM line-scan with VIS header | many (Robot/Scottie/Martin/PD/…) | Both | Image |
| NAVTEX / SITOR-B | `navtex.rs` | 100-baud 2-FSK maritime broadcast, CCIR-476 FEC-B | navtex / sitorb | Both | Text |
| WEFAX | `wefax.rs` | HF weather facsimile, FM pixel-luma, IOC-based raster | wefax576 (IOC 576) / wefax288 (IOC 288) | Both | Image |

(NAVTEX/SITOR-B decode to text despite living with the facsimile modes.)

## Picture sub-protocols (in-band images over keyboard modes)

Four keyboard modes can carry a raster image in-band: they switch out of their text
state machine, emit a mode-specific header, then send pixel-FSK where each 8-bit
pixel maps to a carrier offset. The daemon assembles these in
[`../../crates/omnimodem/src/mode/picture_tx.rs`](../../crates/omnimodem/src/mode/picture_tx.rs)
(`PictureSend::build`), driven by the `TransmitImage` RPC; the shared pixel-FSK codec
is `modes/picture.rs`.

| Sub-protocol | File | Header | Sizes | Notes |
|---|---|---|---|---|
| MFSK picture | `mfsk_pic.rs` | explicit `W×H` in the text stream | any size | 3 samples-per-pixel options (8/4/2) |
| THOR picture | `thor_pic.rs` | mode-char token | fixed size table | BT.601 luma |
| IFKP picture | `ifkp_pic.rs` | mode-char token | fixed size table | BT.601 luma |
| FSQ picture | `fsq_pic.rs` | directed-message header (`%` trigger) | fixed image modes | BGR plane order, FSQ-linear pixel scaling |

Shared codec `picture.rs` (`PictureCodec`) handles the pixel↔frequency mapping
(`Deviation256` for MFSK/THOR/IFKP, `FsqLinear` for FSQ), luma reduction, and plane
ordering. This is distinct from the standalone raster modes (Hell/SSTV/WEFAX), which
carry the image as their whole waveform rather than in-band over a keyboard mode.

## RSID

Independent of the mode: any channel can set `rsid_tx`/`rsid_rx`
([`../grpc-api.md`](../grpc-api.md#channel-configuration)). When enabled, the daemon
prepends the active mode's RSID (Reed-Solomon Identifier) burst before each TX and/or
runs the RSID detector over received audio, surfacing matches as `RsidDetected`
events (fldigi-compatible tags). Detector/generator: `frontend/rsid.rs`.

`rsid_tx` is sticky per channel, so it survives a mode switch. TX gating uses
`ModeConfig::rsid_tx_key` (not `rsid_key`), which announces only for the fldigi
sound-card modes. It returns `None` for **CW** (keyed by ear) and **JT65** (a WSJT-X
slot mode with its own sync — a burst ahead of it is meaningless and would shift the
timed slot); every other WSJT-X mode already has no `rsid_key`, so those were the only
two that could leak a burst when the flag carried over from a prior digital mode
(GRA-318). RX detection still uses the full `rsid_key` table, so CW and JT65 remain
identifiable on receive. The emitting set is locked by
`tx_rsid_never_announces_on_cw_or_wsjtx_modes` in `mode/mod.rs`.
