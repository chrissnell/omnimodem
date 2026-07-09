# Glossary

Domain terms as omnimodem uses them, each pointing at where it is implemented or
canonically described in *this* project. For general amateur-radio / DSP background,
the reference implementations (WSJT-X, fldigi, Direwolf) are the starting point.

## Architecture

| Term | In this project | Pointer |
|---|---|---|
| Control edge | The async tonic/tokio surface: gRPC handlers + subscription streams. No DSP. | [`grpc-edge.md`](grpc-edge.md), `crates/omnimodemd/src/grpc/` |
| Sync core | The synchronous `std::thread` command loop owning audio/DSP/PTT. No async on the sample path. | `crates/omnimodemd/src/core/mod.rs` |
| `CoreHandle` | The edge's handle to the core: bounded command sender + the two broadcast event senders. | `core/mod.rs` |
| `Command` | One variant per RPC action, carrying operands + a `oneshot` reply channel. | `core/command.rs` |
| Supervisor | State owner on the core thread: channels, device cache, PTT registry, interlock, SQLite store. | `supervisor/mod.rs` |
| Channel | A logical pipeline (audio → demod → frames on RX; frame → mod → audio+PTT on TX), identified by a `uint32`. | `supervisor/channel.rs`, `proto` |
| Lossless / lossy event class | Decoded frames are never dropped (disconnect a laggard); telemetry drops intermediates. | `core/event.rs`, [`invariants.md`](invariants.md) |
| Snapshot-on-subscribe | `SubscribeEvents` replays full state as the first message, then streams live. | `grpc/subscribe.rs` |
| RX / TX worker | Per-channel `std::thread` running the demod (RX) or the modulation queue (TX). | `core/rx_worker.rs`, `core/tx_worker.rs` |
| TX lease | Optional per-rig exclusive transmit right for non-interleavable sessions. | `ptt/lease.rs` |
| Interlock | Per-rig, nesting-safe muting of RX while the rig is keyed. | `ptt/interlock.rs` |

## Hardware & platform

| Term | In this project | Pointer |
|---|---|---|
| `DeviceId` | Stable cross-platform device identity (USB VID:PID:serial, ALSA card name, USB topology, `/dev/serial/by-id`, or placeholder). The persistence key. | `ids.rs`, [`audio-devices-ptt.md`](audio-devices-ptt.md) |
| `AudioBackend` | Trait abstracting capture/playback (cpal / file / stdin / null). | `audio/backend.rs` |
| `plughw` trap | ALSA advertising synthetic rates the codec can't honor → bit-timing desync. Avoided by the 48 kHz capture ceiling. | `audio/alsa.rs`, `audio/mod.rs` |
| Fanout | One capture stream feeding several demods (opt-in). | `audio/fanout.rs` |
| PTT | Push-to-talk: the control line that keys the transmitter. | `ptt/mod.rs` |
| CM108 | USB HID audio codec with GPIO, keyed via a 5-byte HID report (pins 1–8). | `ptt/cm108.rs` |
| VOX | Voice-operated TX — keyed by audio, no control line (a no-op driver). | `ptt/none.rs` |
| `PortRegistry` | Caches PTT drivers by `DeviceId`; evicts + reopens on hotplug. | `ptt/registry.rs` |
| `SO_PEERCRED` | Linux socket peer-credential check; UDS authz requires peer uid == daemon uid. | `authz/uds.rs` |
| udev rule | Device-matching rule creating a stable `/dev/omnimodem/<label>` symlink; the daemon only *suggests* the text. | `ptt/udev.rs`, `SuggestUdevRule` |
| KISS | FEND/FESC TNC framing; the bridge lets legacy packet apps drive an AFSK-1200 channel over TCP. | `kiss/` |

## DSP & coding

| Term | In this project | Pointer |
|---|---|---|
| LLR (soft value) | `ln(P(bit=0)/P(bit=1))`; positive ⇒ bit 0. The detector↔FEC boundary type. | `dsp/types.rs`, [`dsp-building-blocks.md`](dsp-building-blocks.md) |
| `SoftBits` | A vector of LLRs carried demapper → FEC decoder. | `dsp/types.rs` |
| Streaming vs windowed demod | `feed(samples)` continuous (AFSK/PSK/RTTY) vs `decode_window()` time-aligned multi-pass (FT8/JS8/WSPR). | `dsp/mode.rs` (`DemodShape`) |
| `ModeCaps` | A mode's declared native rate, bandwidth, TX support, duplex, and shape. | `dsp/mode.rs` |
| Ensemble ("hydra") | `ParallelDemodulator<D>` runs N decoder configs and unions/dedups their output. | `dsp/ensemble.rs` |
| Costas loop | Coherent carrier-phase/frequency recovery for PSK. | `dsp/sync/costas.rs` |
| Costas array | A frequency-hop sync pattern (FT8/FT4) with a thumbtack autocorrelation — different from the loop. | `dsp/sync/costas_array.rs` |
| DPLL | Digital PLL bit-clock recovery with locked/searching inertia. | `dsp/sync/dpll.rs` |
| DCD | Data-carrier detect (hysteresis-scored carrier presence). | `dsp/sync/dcd.rs` |
| Candidate finder | Wideband passband sweep producing a `(freq, time, metric)` list for multi-decode. | `dsp/sync/candidate.rs` |
| STFT | Overlapped short-time FFT engine (waterfall + tone detection). | `dsp/frontend/stft.rs` |
| RSID | Reed-Solomon Identifier — a burst that announces the mode + audio offset; detected/generated regardless of mode. | `dsp/frontend/rsid.rs`, `rsid_tx`/`rsid_rx` |
| LDPC / BP / OSD | Low-density parity-check code, belief-propagation decode, ordered-statistics decode (last-dB layer). | `dsp/fec/ldpc.rs`, `dsp/fec/osd.rs` |
| Fano decoder | Sequential decoder for deep-constraint (K=32) convolutional codes (JT9/WSPR). | `dsp/fec/fano.rs` |
| Reed-Solomon (fcr) | GF(256) RS with parametric `(nroots, fcr, prim)` — fcr=1 for FX.25, fcr=0 for IL2P; GF(2⁶) soft variant for JT65. | `dsp/fec/rs.rs`, `dsp/fec/rs_gf64.rs` |
| NRZI | Non-return-to-zero inverted differential line code. | `dsp/fec/nrzi.rs` |
| Scrambler | Whitening LFSR — self-synchronizing (G3RUH) vs frame-reset additive (IL2P). | `dsp/fec/scramble.rs` |
| Varicode | Self-framing variable-length text code; PSK/MFSK/DominoEX/THOR/IFKP/FSQ each have a variant. | `dsp/framing/*varicode*.rs` |
| Baudot / ITA2 | 5-bit teleprinter code (LTRS/FIGS shift) for RTTY. | `dsp/framing/baudot.rs` |
| HDLC | Flag/bit-stuff/FCS framing (CRC-16/X.25) under AX.25. | `dsp/framing/hdlc.rs` |
| FX.25 / IL2P | RS-FEC layers over/replacing AX.25 HDLC. | `dsp/framing/fx25.rs`, `dsp/framing/il2p.rs` |
| Pack77 | The WSJT-X 77-bit message codec + callsign hashing (FT8/FT4). | `dsp/framing/pack77.rs` |

## Modes & RF (see [`mode-catalog.md`](mode-catalog.md) for the full list)

| Term | In this project | Pointer |
|---|---|---|
| AFSK 1200 (AX.25) | Bell-202 packet, the baseline mode + KISS bridge. | `dsp/modes/afsk1200.rs` |
| FT8 / FT4 / FST4 | WSJT-X GFSK weak-signal modes, LDPC + Costas-array sync, time-slotted. | `dsp/modes/ft8.rs`, `ft4.rs`, `fst4.rs` |
| JT65 / JT9 / JT4 / WSPR | Legacy WSJT-X modes (soft-RS / K=32 Fano). | `dsp/modes/jt65.rs`, `jt9.rs`, `jt4.rs`, `wspr.rs` |
| JS8 | FT8-derived keyboard/ARQ mode. | `dsp/modes/js8.rs` |
| MSK144 | Meteor-scatter offset-MSK burst mode. | `dsp/modes/msk144.rs` |
| IFK / IFK+ | Incremental Frequency Keying (differential tone offset); 18-tone IFK+ (DominoEX/THOR), 33-tone IFK (IFKP/FSQ). | `dsp/modes/{dominoex,thor,ifkp,fsq,ifk33}.rs` |
| FSQCALL / directed | FSQ's CRC8-keyed station-routing layer (selective call / allcall / CQ). | `dsp/modes/fsq/directed.rs` |
| CCIR-476 / SITOR-B | 4-of-7 constant-ratio FEC-B for NAVTEX/SITOR-B maritime text. | `dsp/fec/ccir476.rs`, `dsp/modes/navtex.rs` |
| IOC | Index of Cooperation — the WEFAX facsimile line-resolution parameter (576/288). | `dsp/modes/wefax.rs` |
| Picture sub-protocol | In-band image over a keyboard mode (MFSK/THOR/IFKP/FSQ) via a header + pixel-FSK. | `dsp/modes/*_pic.rs`, `dsp/modes/picture.rs`, `mode/picture_tx.rs` |
| Raster / `Image` | A decoded image: `width`, row-major `pixels`, `channels` (1 grayscale / 3 RGB), appended per on-air column. | `proto` `Image`, [`../grpc-api.md`](../grpc-api.md#rasters-receiving-and-sending-images) |
| ClockOffset | Host clock-discipline metric (NTP offset/error/synchronized); windowed modes need an accurate clock. | `core/clock.rs`, `proto` `ClockOffset` |
