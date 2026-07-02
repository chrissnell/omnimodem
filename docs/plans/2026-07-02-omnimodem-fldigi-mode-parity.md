# Omnimodem — fldigi Mode Parity Roadmap

> **Type:** program-level roadmap (a plan of plans). Each phase below becomes its own
> task-by-task executable plan (in the style of `2026-06-20-...-phase4-first-modes.md`)
> once its scope is confirmed. This document establishes the full parity gap, groups the
> remaining modes into build-order workstreams, maps every mode to the **fldigi source that
> is its reference implementation**, and names the new building blocks each group needs.

**Goal:** Reach full [fldigi](https://sourceforge.net/projects/fldigi/) keyboard/data/image mode
parity in omnimodem, implementing each mode against fldigi's own DSP as the reference guide, and
gating each with the existing conformance harness (KAT vectors, bidirectional cross-decode with
fldigi, BER/decode-rate curves).

**Reference source:** the fldigi tree is checked out at `omnimodem/fldigi/` (`git@github.com:w1hkj/fldigi.git`).
Per-mode source lives under `fldigi/src/<family>/`.

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

So four of fldigi's ~15 mode families are (wholly or partly) covered. The rest is the parity gap.

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
- **QPSK:** QPSK31/63/125/250/500 — differential QPSK + K=7 convolutional FEC + soft Viterbi (blocks exist).
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

Convolutional FEC, soft Viterbi, Walsh/FHT, interleavers, Golay, Baudot, PSK varicode, HDLC — **already present** and reused as-is.

## 5. Proposed phasing (build order follows shared blocks)

Each phase is an independent, shippable workstream and becomes its own executable plan. Ordered by
value-per-effort given the existing substrate. Priority reflects real on-air usage (EMCOMM/ARES/keyboard).

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

**Definition of done for a mode (unchanged, design §"Conformance"):** its KAT vectors pass,
cross-decode with fldigi works **both** directions, and its BER/decode-rate curve meets the committed
threshold. A loopback demo is necessary but not sufficient.

Phases 7–9 are the high-value core and depend only on blocks that already exist plus small new pieces —
they are the recommended immediate targets. Phase 10 (image framework) unblocks the picture
sub-protocols and WEFAX. The long tail (Phase 14) and the utility tools (§2) are optional.

## 6. Open scope decisions (for the issue owner)

1. **Utility modes** (SSB/WWV/ANALYSIS/FMT/DTMF) — confirm **out of scope**? (Recommended: yes.)
2. **8PSK + OFDM data modes** — full parity, or defer to the long tail? (Recommended: long tail.)
3. **RSID transmit** — detect-only, or also transmit the ID burst? (Recommended: detect first, TX later.)
4. **Image sub-protocols** (MFSK/THOR/IFKP picture TX/RX) — parity target, or text-only for those modes first?
5. **Start point** — confirm Phase 7 (PSK family) as the first executable plan.
