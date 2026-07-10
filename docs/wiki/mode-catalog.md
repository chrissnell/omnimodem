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
| ADS-B (Mode S) | `adsb/` | 1090 MHz 1 Mbit/s PPM extended squitter; 8 µs preamble, 56/112-bit frames, 24-bit CRC, CPR position + barometric altitude. DSP + KATs landed; daemon wiring / typed `AircraftReport` / TUI flights table are Phases 2-4. TX is loopback self-test only (1090 MHz TX is illegal). | RX (+loopback) | Packet → aircraft |

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
