# Omnimodem — fldigi + WSJT-X Mode Parity Roadmap

> **Type:** program-level roadmap (a plan of plans). Each phase below becomes its own
> task-by-task executable plan (in the style of `2026-06-20-...-phase4-first-modes.md`)
> once its scope is confirmed. This document establishes the full parity gap, groups the
> remaining modes into build-order workstreams, maps every mode to the **upstream source that
> is its reference implementation** (fldigi for the keyboard/data/image modes, WSJT-X for the
> weak-signal modes), and names the new building blocks each group needs.

**Goal:** Reach full [fldigi](https://sourceforge.net/projects/fldigi/) **and**
[WSJT-X](https://sourceforge.net/p/wsjt/wsjtx/) mode parity in omnimodem, implementing each mode
against its upstream DSP as the reference guide, and gating each with the existing conformance harness
(KAT vectors, bidirectional cross-decode with the reference binary, BER/decode-rate curves).

**Reference source:**
- fldigi is checked out at `omnimodem/fldigi/` (`git@github.com:w1hkj/fldigi.git`); per-mode source under `fldigi/src/<family>/`.
- WSJT-X is checked out at `omnimodem/wsjtx/` (`git@github.com:WSJTX/wsjtx.git`); the DSP is Fortran under `wsjtx/lib/`, the mode enum is `wsjtx/models/Modes.hpp`.
- JS8Call is checked out at `omnimodem/js8call/` (`git@github.com:js8call/js8call.git`); JS8-specific source at the repo root (`JS8*.cpp`, `jsc.*`, `varicode.*`) and DSP under `js8call/lib/js8/`.

---

## 1. Where we are

Omnimodem implements ten modes today (`crates/dsp/src/modes/`): `afsk1200`, `cw`, `ft4`, `ft8`,
`jt65`, `jt9`, `olivia`, `psk31`, `rtty`, `wspr`. Of these, **FT8/FT4/JT65/JT9/WSPR** are the
WSJT-X family (not fldigi) and **AFSK1200** is packet. The overlap with fldigi's catalog is:

| fldigi family | omnimodem status |
|---|---|
| CW (Morse) | ✅ implemented |
| RTTY (2-FSK + Baudot) | ✅ implemented (baud/shift/center/reverse parametric) |
| BPSK (PSK31) | ⚠️ **BPSK31 only** — the rest of the PSK family is missing |
| Olivia (parametric tones/bandwidth) | ✅ implemented (covers the OLIVIA submode grid) |

So four of fldigi's ~15 mode families are (wholly or partly) covered. On the **WSJT-X** side
(`wsjtx/models/Modes.hpp` enumerates JT65, JT9, JT4, WSPR, Echo, MSK144, FreqCal, FT8, FT4, FST4,
FST4W, Q65), omnimodem has FT8, FT4, JT65, JT9, WSPR — so the WSJT-X gap is **JT4, MSK144, FST4,
FST4W, Q65** (Echo and FreqCal are utilities, not data modes → out of scope, same call as fldigi's
WWV/FMT). The rest of both catalogs is the parity gap.

The substrate is in good shape. The Phase-5 building-block groups already landed in `crates/dsp/src/fec/`
(`conv` + soft Viterbi, `fano`, `fht` Walsh/Hadamard, `interleave`, `golay`, `rs_gf64`) and
`crates/dsp/src/framing/` (`varicode`, `baudot`, `morse`, HDLC/AX.25). Most of the remaining modes are
**assemblies over blocks we already have** — the genuinely new building blocks are enumerated in §4.

The full fldigi mode enumeration is `fldigi/src/include/globals.h` (the `MODE_*` enum).

## 2. The parity gap — remaining fldigi mode families

Grouped by the shared DSP they need (which is also the natural build order). "Reference" = the fldigi
source directory to study for that family.

### PSK family expansion — reference `fldigi/src/psk/` (`psk.cxx`, `pskvaricode.cxx`, `pskeval.cxx`)
- **BPSK rates:** PSK63, PSK63F, PSK125, PSK250, PSK500, PSK1000 — our `psk31` assembly re-parametrized by symbol rate; `F` variants add a convolutional FEC layer (`fec::conv`).
- **QPSK:** QPSK31/63/125/250/500 — differential QPSK + convolutional FEC + soft Viterbi. Note: fldigi's QPSK uses **K=5** (`POLY 0x17/0x19`, `psk.cxx:66-68`); the robust/`+F` PSK modes use **K=7** (`0x6d/0x4f`). (Corrected from an earlier "K=7 for QPSK" — the reference wins.)
- **PSK-R (robust):** PSK125R…1000R plus the multi-carrier `nX_PSK*R` grid — PSK + convolutional FEC + interleaver, run over N parallel carriers.
- **Multi-carrier PSK:** `nX_PSK*` (e.g. `12X_PSK125`, `2X_PSK500`) — N BPSK carriers in parallel; needs a small multi-carrier harness (§4).
- **8PSK:** 8PSK125…1200F — 8-ary PSK, with/without FEC. Lower priority (rare on the air).

### MFSK family — reference `fldigi/src/mfsk/` (`mfsk.cxx`, `mfskvaricode.cxx`, `interleave.cxx`, `mfsk-pic.cxx`)
- MFSK8/16/32/4/11/22/31/64/128 (+ `64L`/`128L` long-interleave) — M-ary FSK + convolutional FEC + interleaver + **MFSK varicode**. Includes an **image (picture) sub-protocol** (`mfsk-pic.cxx`).

### DominoEX + THOR (incremental frequency keying) — reference `fldigi/src/dominoex/`, `fldigi/src/thor/`
- **DominoEX** Micro/4/5/8/11/16/22/44/88 — IFK (Incremental Frequency Keying) + DominoEX varicode (`dominovar.cxx`). No FEC.
- **THOR** Micro/4/5/8/11/16/22/25x4/50x1/50x2/100 — IFK+ (THOR = DominoEX + convolutional FEC + soft decode + interleaver) + THOR varicode (`thorvaricode.cxx`) + image (`thor-pic.cxx`).
- Shared new block: an **IFK tone tracker + varicode framework** (§4). Build DominoEX first (no FEC), then THOR reuses it.

### Hellschreiber — reference `fldigi/src/feld/` (`feld.cxx`, `feldfonts.cxx`, `Feld*-14.cxx`)
- Feld Hell, Slow Hell, Hell X5/X9, FSK-Hell 245/105, Hell 80 — on/off-keyed (or FSK) column-scan **facsimile text**. Output is a **glyph raster**, not decoded characters — needs the image/raster payload (§4) and the fldigi bitmap fonts.

### Contestia — reference `fldigi/src/contestia/` (`contestia.cxx`)
- Contestia submode grid — Olivia's sibling (MFSK + 32-symbol Walsh/Hadamard vs Olivia's 64). Parametric like our `olivia` assembly; reuses `fec::fht`.

### MT63 — reference `fldigi/src/mt63/` (`mt63.cxx`, `mt63base.cxx`, `dsp.cxx`)
- MT63-500/1000/2000 × Short/Long — 64-carrier overlapping-Walsh OFDM with deep interleaving; very robust, heavy EMCOMM use. Needs the **OFDM / overlap-add core** (§4).

### NAVTEX / SITOR-B — reference `fldigi/src/navtex/` (`navtex.cxx`)
- 100-baud FSK maritime broadcast text with SITOR **FEC-B** (CCIR 476, time-diversity 4-of-7). Needs the **CCIR-476 / FEC-B** block (§4). (SYNOP weather decode rides on this — `fldigi/src/synop-src/`.)

### WEFAX — reference `fldigi/src/wefax/` (`wefax.cxx`, `wefax-pic.cxx`)
- WEFAX-576 / WEFAX-288 — HF weather fax; FM-modulated grayscale image at IOC 576/288 with APT start/phasing/stop. Image mode — needs the raster payload (§4) plus FM demod + line phasing.

### FSQ + IFKP — reference `fldigi/src/fsq/`, `fldigi/src/ifkp/`
- **FSQ** — 33-tone IFK low-speed mode with a **directed/selective-call protocol** (FSQCALL: triggers, directed messages, heard-list). Protocol layer is the bulk of the work.
- **IFKP** — Incremental Frequency Keying + Prometheus; IFK text+image mode. Reuses the IFK core from DominoEX/THOR.

### Throb / ThrobX — reference `fldigi/src/throb/` (`throb.cxx`)
- Throb1/2/4, ThrobX1/2/4 — slow 2-of-N tonal text mode. Niche/legacy; low priority.

### OFDM data modes — reference `fldigi/src/psk/` (fldigi folds OFDM into the psk modem)
- OFDM-500F/750F/2000F/2000/3500 — recent high-rate OFDM data modes. Reuses the OFDM core (§4). Lower priority (uncommon).

### Utilities (out of parity scope, flag only)
- SSB (audio passthrough), WWV (time-signal analysis), ANALYSIS (freq analysis), FMT (frequency-measurement test), DTMF. These are fldigi tools, not text/data modes. Recommend **out of scope** for mode parity; revisit individually if wanted.

## 2b. The parity gap — remaining WSJT-X modes

Reference source is Fortran under `wsjtx/lib/`. Omnimodem already has the FT8 windowed-decode path
(STFT → candidate → Costas sync → soft-LLR → LDPC + OSD → CRC → 77-bit unpack) plus the Fano
sequential decoder (JT9/WSPR), so these modes are mostly new **codes and waveforms** bolted onto
existing windowed infrastructure.

- **JT4** — reference `wsjtx/lib/jt4.f90`, `jt4_decode.f90`, `jt4code.f90`. EME/weak-signal 4-tone FSK, submodes A–G (tone spacing). K=32 convolutional + Fano — **reuses our existing `fec::fano` + 4-FSK**; lowest-effort of the group.
- **MSK144** — reference `wsjtx/lib/decode_msk144.f90`, `genmsk_128_90.f90`, `msk144code.f90`, `msk144decodeframe.f90`. Meteor-scatter; offset-MSK waveform + LDPC(128,90). Needs a new **MSK/OQPSK waveform** block; reuses the LDPC BP decoder.
- **FST4 / FST4W** — reference `wsjtx/lib/fst4/`, `fst4_decode.f90`. LF/MF 4-GFSK with the **(240,101)** and **(240,74)** LDPC codes and long, selectable T/R periods (15/30/60/120/300/900/1800 s). Needs both LDPC table sets (`ldpc_240_101_*`, `ldpc_240_74_*`) + CRC-24; reuses the BP decoder + windowed path (with variable window length). FST4W is the beacon variant of the same waveform.
- **Q65** — reference `wsjtx/lib/q65_decode.f90`, `q65params.f90`, `qra64code.f90`, and `wsjtx/lib/qra/` (`qra65`, `qracodes`). EME/troposcatter; 65-tone with the **QRA65 (Q-ary Repeat-Accumulate) code** and submodes A–E × T/R periods. Needs a new **QRA65 soft decoder** — the one genuinely new FEC family in this group.

### WSJT-X utilities (out of scope)
- **Echo** (moon-echo measurement) and **FreqCal** (frequency calibration) — tools, not QSO/data modes. Same out-of-scope call as fldigi's WWV/FMT.

### Adjacent: JS8
- **JS8** (JS8Call) is WSJT-X-*derived* — it reuses the FT8-class core (Costas sync + LDPC + 77-bit) but is its own app. Now checked out at `omnimodem/js8call/` (`git@github.com:js8call/js8call.git`), so it references the same way the others do. Reference source:
  - **Submodes** (`js8call/JS8Submode.*`, `lib/js8/`): JS8 ships four speeds — **Normal (A, 15 s)**, **Fast (B, 10 s)**, **Turbo (C, 6 s)**, **Slow (E, 30 s)** — each with its own decoder module (`lib/js8a_decode.f90`, `js8b_decode.f90`, `js8c_decode.f90`, `js8e_decode.f90`) and Costas/frame parameters.
  - **Keyboard text codec** (`js8call/jsc.cpp/.h`, `jsc_list.cpp`, `jsc_map.cpp`, `varicode.cpp/.h`): JS8's JSC compressed character set — the analogue of a varicode, this is the bulk of the encode/decode work beyond the shared FT8 DSP.
  - **Directed protocol**: heartbeat/CQ, directed calls, relay/store-forward, ACK/SNR exchanges (message handling in `js8call/varicode.cpp` + `mainwindow.cpp`).

## 3. Cross-cutting parity work (not modes, but needed for real fldigi interop)

- **Image/raster payload.** `crates/dsp/src/types.rs::FramePayload` is `Packet | Text | Message77 | Vocoder`. Hell, WEFAX, and the MFSK/THOR/IFKP/FSQ picture sub-protocols all emit **pixels**. Add a `FramePayload::Image { width, rows, gray }` (or similar) variant and thread it through the daemon RX event path and the gRPC proto. This is a prerequisite for the Hell and WEFAX phases.
- **RSID (Reed-Solomon Identifier).** fldigi's auto mode/frequency detection — a short MFSK burst (`fldigi/src/rsid/`) that IDs the mode + offset. For genuine on-air interop, omnimodem should **detect** RSID (auto-switch/report) and optionally **transmit** it. Cross-cutting; slot after the first MFSK-family modes exist so there's something to switch to.
- **Video/text ID** (the small waterfall ident) — minor; bundle with RSID.

## 4. New building blocks (the real new DSP)

Everything else is assembly. These are the blocks that don't exist yet, with the phase that introduces each:

| Block | New file (proposed) | Feeds | Reference |
|---|---|---|---|
| Multi-carrier PSK harness | `frontend/multicarrier.rs` | PSK-R, nX-PSK | `psk.cxx` |
| IFK tone tracker + varicode framework | `modes/ifk.rs`, `framing/ifk_varicode.rs` | DominoEX, THOR, IFKP, FSQ | `dominoex.cxx`, `dominovar.cxx` |
| Image/raster `FramePayload` | `types.rs` (+ proto) | Hell, WEFAX, *-pic | `mfsk-pic.cxx`, `wefax-pic.cxx` |
| Hell raster codec + fonts | `modes/hell.rs`, `framing/hellfont.rs` | Hell family | `feld.cxx`, `feldfonts.cxx` |
| OFDM / overlap-add core | `frontend/ofdm.rs` | MT63, OFDM modes | `mt63base.cxx`, `dsp.cxx` |
| CCIR-476 / SITOR FEC-B | `fec/ccir476.rs` | NAVTEX/SITOR-B | `navtex.cxx` |
| WEFAX FM demod + IOC phasing | `modes/wefax.rs` | WEFAX | `wefax.cxx` |
| RSID encode/detect | `frontend/rsid.rs` | cross-cutting | `rsid/` |
| MFSK varicode | `framing/mfsk_varicode.rs` | MFSK | `mfskvaricode.cxx` |
| MSK / OQPSK waveform | `frontend/msk.rs` | MSK144 | `wsjtx/lib/genmsk_128_90.f90`, `decode_msk144.f90` |
| FST4 LDPC (240,101)+(240,74) tables | `fec/ldpc_fst4.rs` | FST4/FST4W | `wsjtx/lib/fst4/ldpc_240_*` |
| QRA65 soft decoder | `fec/qra65.rs` | Q65 | `wsjtx/lib/qra/`, `qra64code.f90` |

Convolutional FEC, soft Viterbi, Fano, Walsh/FHT, interleavers, Golay, the LDPC BP decoder + OSD, the
STFT/Costas windowed path, 77-bit message pack/unpack, Baudot, PSK varicode, HDLC — **already present**
and reused as-is. JT4 needs no new block (Fano + 4-FSK); FST4/FST4W reuse the LDPC BP decoder with new
code tables; only MSK144's waveform and Q65's QRA65 code are net-new DSP on the WSJT-X side.

## 5. Proposed phasing (build order follows shared blocks)

Each phase is an independent, shippable workstream and becomes its own executable plan. Ordered by
value-per-effort given the existing substrate. Priority reflects real on-air usage (EMCOMM/ARES/keyboard).

**fldigi track** — reference `fldigi/src/`:

| Phase | Modes | New blocks | Priority | Rough effort |
|---|---|---|---|---|
| **7 — PSK family** | PSK63/125/250/500(+F), QPSK31–500, PSK-R + multi-carrier | multicarrier harness | **High** (very common) | Low–Med (reuses psk31 + conv) |
| **8 — MFSK + Contestia** | MFSK8/16/32/…, Contestia grid | MFSK varicode; (Contestia reuses fht) | **High** | Med |
| **9 — IFK: DominoEX + THOR** | DominoEX + THOR grids | IFK core + varicodes | **High** (robust NVIS/EMCOMM) | Med |
| **10 — Image framework + Hell** | Feld Hell + variants | Image payload; Hell raster+fonts | **Med** (also unblocks WEFAX/pic) | Med |
| **11 — MT63** | MT63 500/1000/2000 S/L | OFDM/overlap-add core | **High** (heavy EMCOMM) | Med–High |
| **12 — NAVTEX/SITOR-B + WEFAX** | NAVTEX, SITOR-B, WEFAX 576/288 | CCIR-476 FEC-B; WEFAX FM/phasing | **Med** (SWL/utility) | Med |
| **13 — RSID + FSQ + IFKP** | RSID auto-ID; FSQ; IFKP | RSID; FSQ protocol | **Med** | Med–High (FSQ protocol) |
| **14 — Long tail** | Throb/ThrobX, 8PSK, OFDM data modes | OFDM core (reused) | **Low** | Low–Med |

**WSJT-X track** — reference `wsjtx/lib/`. Independent of the fldigi track (reuses the FT8 windowed
path, not fldigi blocks), so these run **in parallel** and can be interleaved by priority:

| Phase | Modes | New blocks | Priority | Rough effort |
|---|---|---|---|---|
| **W1 — FST4 / FST4W** | FST4, FST4W (all T/R periods) | LDPC (240,101)+(240,74) tables | **High** (active LF/MF + growing HF QSO) | Low–Med (reuses BP decoder + windowed path) |
| **W2 — MSK144** | MSK144 | MSK/OQPSK waveform | **Med–High** (the VHF meteor-scatter mode) | Med |
| **W3 — Q65** | Q65 (submodes A–E) | QRA65 soft decoder | **Med** (EME/troposcatter) | Med–High (net-new FEC) |
| **W4 — JT4** | JT4 (submodes A–G) | none (Fano + 4-FSK) | **Low** (legacy EME) | Low |
| **W5 — JS8** | JS8 Normal/Fast/Turbo/Slow (A/B/C/E) | JSC text codec + directed protocol | **Med** | Med (reuses FT8 core; `js8call/lib/js8/`, `jsc.*`) |

**Definition of done for a mode (unchanged, design §"Conformance"):** its KAT vectors pass,
cross-decode with the reference binary (fldigi **or** WSJT-X) works **both** directions, and its
BER/decode-rate curve meets the committed threshold. A loopback demo is necessary but not sufficient.

fldigi Phases 7–9 and WSJT-X W1 are the high-value core, depending only on existing blocks plus small
new pieces — the recommended immediate targets. Phase 10 (image framework) unblocks the picture
sub-protocols and WEFAX. The long tail (Phase 14, W4) and the utility tools (§2, §2b) are optional.

## 6. Open scope decisions (for the issue owner)

1. **Utility modes** (SSB/WWV/ANALYSIS/FMT/DTMF) — confirm **out of scope**? (Recommended: yes.)
2. **8PSK + OFDM data modes** — full parity, or defer to the long tail? (Recommended: long tail.)
3. **RSID transmit** — detect-only, or also transmit the ID burst? (Recommended: detect first, TX later.)
4. **Image sub-protocols** (MFSK/THOR/IFKP picture TX/RX) — parity target, or text-only for those modes first?
5. **Start point** — confirm Phase 7 (PSK family) and/or WSJT-X W1 (FST4/FST4W) as the first executable plan(s). *(JS8/W5 is now in scope — JS8Call repo added.)*
