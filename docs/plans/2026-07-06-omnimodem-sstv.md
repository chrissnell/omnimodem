# Phase 17 — SSTV (analog line-scan picture modes) via MMSSTV — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Read the Porting Doctrine in `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md` before any task** — it is non-negotiable and governs every task here.

**Goal:** Bring SSTV (Slow-Scan Television, analog FM-subcarrier line-scan picture modes) into omnimodem by **porting the DSP core of MMSSTV** (`n5ac/mmsstv`, LGPLv3), not clean-room reimplementing. Every submode must be bit-exact on the integer/raster domain and within FP tolerance on audio versus vectors extracted from the *unmodified* MMSSTV reference, and must reproduce a known test image end-to-end through our own TX→RX loopback.

**Why its own phase (split from GRA-267 / Phase 15):** the fldigi picture sub-protocols (MFSK/THOR/IFKP/FSQ pic) are *in-band digital* payloads riding an existing digital modem. SSTV is a **self-contained analog modem** — VIS header + FM-subcarrier line scan — structurally unlike anything in the fldigi/WSJT-X tracks. It shares only the raster *output* path (`FramePayload::Image`, the gRPC `Image` message, the TUI `image` shape) established in Phase 10.

**Reference:** `n5ac/mmsstv`, upstream commit **`8060b5f`** ("Added LGPL files received from AA6YQ"). Native sample rate **11025 Hz** (`Main.cpp:212`; all mode timing is `ms * SampFreq / 1000.0`).

**Tech stack:** Rust (edition 2021, workspace). New mode assembly `crates/dsp/src/modes/sstv.rs` + supporting DSP blocks under `crates/dsp/src/`, one arm in `crates/omnimodemd/src/mode/registry.rs`, a `SstvParams` proto message, and the TUI `image` view (already exists from Hell, Phase 10). Reuses `FramePayload::Image`, the `Image` gRPC message, and the conformance harness in `crates/dsp/tests/` (`kat.rs`, `ber.rs`, `loopback.rs`, `vectors/`).

**The hard part (called out up front):** MMSSTV is a **Borland C++ Builder / VCL Windows app** (`.dfm`, `.cbproj`, `__fastcall`, `TColor`, `Graphics::TBitmap`, Shift-JIS source). It **does not build on Linux/CI as-is.** The golden-vector step therefore requires **isolating the DSP core** (`sstv.cpp` + `ComLib.cpp` colour math + `fir.cpp`) into a minimal standalone harness decoupled from the VCL — this is Task Group F0 and it is a prerequisite for every mode's T1.

---

## Reference map (verified against commit `8060b5f`)

All cites below were read directly; T1 for each family re-pins exact line ranges in the vector provenance header.

| Concern | File | Symbols / lines |
|---|---|---|
| Mode enum (`smR36`…`smMC180`, `smEND`) | `sstv.h` | 450–495 |
| Mode name list + display order | `sstv.cpp` | `SSTVModeList[]` 493–503, `SSTVModeOdr[]` 504–548 |
| Narrow-mode predicate | `sstv.cpp` | `IsNarrowMode` 550–563 |
| Per-mode line/channel timing params | `sstv.cpp` | `CSSTVSET::SetSampFreq` 655–1161 |
| Interval / VIS-length table | `sstv.cpp` | `CSSTVSET::InitIntervalPara` 574–586 |
| Raster geometry (w,h,hp per mode) | `sstv.cpp` | `GetBitmapSize` 607–636, `GetPictureSize` 638–653 |
| RX demod pipeline + VIS detect | `sstv.cpp` | `CSSTVDEM::Do` 1819–2337, VIS FSM 1897–2218 |
| TX modulator primitives | `sstv.cpp` | `CSSTVMOD::Write/WriteC/Do/OpenTXBuf/CloseTXBuf` 2794–2916 |
| TX per-mode line generators | `Main.cpp` | `TMmsstv::LineR24/R36/R72/AVT/SCT/MRT/SC2180/PD/P/MP/MR/RM/MN/MC` 6088–6395 |
| TX VIS header + CW/FSK id | `Main.cpp` | VIS emit ~6933–7165 |
| RX raster reconstruction (the big per-mode switch) | `Main.cpp` | `DrawSSTV`→`DrawSSTVNormal`/`DrawSSTVDiff` 3705–4351 |
| Colour math (Y/R-Y/B-Y ↔ RGB, pixel↔freq) | `ComLib.cpp` | `YCtoRGB` 3475, `GetRY` 3650, `ColorToFreq(Narrow)` 3491/3497, `LimitRGB` 3468 |
| DSP building blocks | `sstv.h`/`fir.h` | `CVCO`,`CPLL`,`CFQC`,`CHILL` (FM demod); `CFIR2`,`CIIR`,`CIIRTANK`,`CLMS`; `CLVL`/`CSLVL` (AGC); `CSYNCINT` (sync) |

### Tone plan (constants to transcribe verbatim, `// ref:` cited)
- **Pixel scan (wide modes):** black `1500 Hz` → white `2300 Hz`, linear: `freq = 1500 + value*(2300-1500)/256` (`ComLib.cpp:3491`).
- **Pixel scan (narrow N modes):** `2044 Hz`→`2300 Hz` (`ComLib.cpp:3497`; `NARROW_LOW/HIGH` `sstv.h:441-442`).
- **Sync** `1200 Hz`; **porch/separator** `1500 Hz`; **separator mark** `1900 Hz`.
- **VIS (8/16-bit):** leader `1900 Hz`/300 ms, break `1200 Hz`/10 ms, leader `1900 Hz`/300 ms, start `1200 Hz`/30 ms, then bits @ 30 ms each (`1`=`1100 Hz`, `0`=`1300 Hz`), stop `1200 Hz`/30 ms. Extended 16-bit VIS marks itself with low byte `0x23` (`mode.txt`; RX FSM `sstv.cpp:1993-2122`).
- **N-VIS (narrow):** 24-bit FSK, 6-bit symbols, `1=1900 Hz`/`0=2100 Hz`, 22 ms/bit (`mode.txt` §7).

---

## Final submode list (pinned from `SSTVModeList[]`, `sstv.cpp:493-503`)

**No silently-partial grids (Doctrine §6): every submode below is wired + table-tested, or the family is not listed.** Grouped by colour/line family (families share one decode + colourspace path and are ported once, then a table-driven test enumerates each submode's params from the reference).

| Group | Family | Submodes (enum id) | Colour model | Geometry (w×h, lines `m_L`) |
|---|---|---|---|---|
| A | **Scottie** | Scottie 1 `smSCT1`, Scottie 2 `smSCT2`, Scottie DX `smSCTDX` | RGB sequential (G,B,R; sync before R) | 320×256, 256 |
| A | **Martin** | Martin 1 `smMRT1`, Martin 2 `smMRT2` | RGB sequential (G,B,R; sync first) | 320×256, 256 |
| A | **SC2** | SC2-180 `smSC2_180`, SC2-120 `smSC2_120`, SC2-60 `smSC2_60` | RGB sequential | 320×256, 256 |
| B | **Robot colour** | Robot 36 `smR36`, Robot 72 `smR72`, Robot 24 `smR24` | Y + alternating R-Y/B-Y (36/24) or Y,R-Y,B-Y per line (72) | 320×240, 240 (`hp=240`) |
| B | **Robot B/W** | B/W 8 `smRM8`, B/W 12 `smRM12` | Y-only (mono) | 320×240, 120 |
| B | **AVT** | AVT 90 `smAVT` | Y/colour, *no sync pulses* (special) | 320×240, 240 |
| C | **PD** | PD50 `smPD50`, PD90 `smPD90`, PD120 `smPD120`, PD160 `smPD160`, PD180 `smPD180`, PD240 `smPD240`, PD290 `smPD290` | Y(odd),R-Y,B-Y,Y(even) — two rows/scan | 320×256 / 512×400 / 640×496 / 800×616 |
| D | **Pasokon P** | P3 `smP3`, P5 `smP5`, P7 `smP7` | RGB sequential | 640×496, 496 |
| E | **MMSSTV MR/ML** | MR73/90/115/140/175 `smMR73…smMR175`, ML180/240/280/320 `smML180…smML320` | Y + horiz-compressed R-Y/B-Y (16-bit VIS) | 320×256 (MR) / 640×496 (ML) |
| E | **MMSSTV MP** | MP73 `smMP73`, MP115 `smMP115`, MP140 `smMP140`, MP175 `smMP175` | Y(odd),R-Y,B-Y,Y(even), vert-compressed colour (16-bit VIS) | 320×256, 128 |
| F | **Narrow MN** (MP-N) | MP73-N `smMN73`, MP110-N `smMN110`, MP140-N `smMN140` | MP colour model, 2044–2300 Hz, N-VIS | 320×256, 128 |
| F | **Narrow MC** (RGB-N) | MC110-N `smMC110`, MC140-N `smMC140`, MC180-N `smMC180` | RGB sequential, 2044–2300 Hz, N-VIS | 320×256, 256 |

**Sequencing rationale:** Groups A–D are the classic, real-world-interoperable modes (Scottie/Martin/Robot/PD/Pasokon) and are ported first. Groups E–F are MMSSTV-native extensions (extended VIS + narrow N-VIS) and land last, reusing the same DSP core. Each group closes only on a green KAT vs the reference; a group that cannot pass stays open and is **not** merged behind a stub.

---

## Verified representative timing params (`CSSTVSET::SetSampFreq`)

Values are `ms` (the reference multiplies by `SampFreq/1000`). `m_KS`=main channel (Y/RGB) window; `m_KS2`=half/colour window; `m_OF`/`m_OFP`=first-pixel porch offset (line 0 / subsequent); `m_SG`/`m_CG`/`m_SB`/`m_CB`=absolute sample offsets to channel starts; `m_L`=lines. These anchor the T4 modulator and T5 demod sampling grid.

- **Scottie 1** (`default`, 1097–1107): KS 138.24, OF 10.5, OFP 10.7, SG 139.74, CG KS+SG, SB SG+SG, CB KS+SB, L 256.
- **Martin 1** (712–722): KS 146.432, OF 5.434, OFP 7.2, SG 147.004, CG KS+SG, SB SG+SG, CB KS+SB, L 256.
- **Robot 36** (658–669): KS 88.0, KS2 44.0, OF 12.0, OFP 10.7, SG 89.25, CG 88+SG, SB SG+SG, CB 88+SB, L 240.
- **Robot 72** (670–680): KS 138.0, KS2 69.0, OF 12.0, OFP 10.7, SG 144.0, CG SG+KS2, SB SG+SG, CB KS+SB, L 240.
- **PD90** (774–783): KS 170.240, OF 22.080, OFP 18.900, SG KS, CG KS+SG, SB SG+SG, CB KS+SB, L 128 (rendered ×2 rows/scan).
- **Robot 24 / B/W8 / B/W12** (1005–1035): R24 KS 92/KS2 46/L120; RM8 KS 58.897/L120 mono; RM12 KS 92/L120 mono.
- **Narrow MN/MC** (1036–1090): MN73 KS 140/L128; MC110 KS 140/L256 — colour→freq via `ColorToFreqNarrow`, 2044–2300 Hz.

The **secondary switch** (`m_KSS`/`m_KS2S`/`m_KSB`, 1109–1161) applies the per-family "black adjustment" trim (e.g. `m_KS - m_KS/1280` for MP73/SCTDX). Transcribe verbatim — it feeds the pixel-sampling grid and matters to the raster domain.

### Colourspace constants (`ComLib.cpp`, transcribe verbatim)
- **RX** `YCtoRGB` (3475): `R = 1.164457*(Y-16) + 1.596128*(RY-128)`; `G = 1.164457*(Y-16) - 0.813022*(RY-128) - 0.391786*(BY-128)`; `B = 1.164457*(Y-16) + 2.017364*(BY-128)`; then `LimitRGB` clamp 0–255.
- **TX** `GetRY` (3650): `Y = 16 + 0.256773R + 0.504097G + 0.097900B`; `RY = 128 + 0.439187R - 0.367766G - 0.071421B`; `BY = 128 - 0.148213R - 0.290974G + 0.439187B`.

---

## Two equivalence classes (Doctrine §3 — do not conflate)

1. **Bit-exact (integer / raster domain):** VIS byte codes (8/16-bit + N-VIS symbols), per-line channel sample offsets (`m_SG…m_CB` in integer samples), the decoded **pixel raster** (8-bit Y or RGB), `ColorToFreq` integer table, `YCtoRGB`/`GetRY` **integer** results after clamp. **Assert byte-for-byte** against golden vectors.
2. **Numerically close (FP DSP):** modulated audio samples, FM-demod/PLL/VCO/FIR outputs. Op-ordering and libm differ across Borland C++ / Rust, so **audio is never asserted bit-exact.** Gate on a committed max-abs-error / correlation tolerance vs the reference waveform **and** on the decisive end-to-end loopback (our TX image → our RX → recovered raster ≈ source within a committed pixel-error / correlation threshold).

**Reference-binary cross-decode caveat:** unlike fldigi/WSJT-X, MMSSTV has no Linux CLI decoder, so the "reference decodes our TX" direction is **not** CI-runnable. We substitute: (a) our TX audio, when fed back through the **isolated MMSSTV decoder harness** (F0), reconstructs the source raster within tolerance — this is an `#[ignore]`-gated local test behind the harness binary; and (b) a strong loopback + per-stage KAT chain that localises any break. State this deviation in each vector provenance header and the `sstv.rs` doc comment (Doctrine §7).

---

## Task Group F0 — Isolate the MMSSTV DSP core into a Linux golden-vector harness (prerequisite)

**This unblocks every mode's T1. Do first.** Goal: build a standalone, VCL-free C++ program on Linux that links the *unmodified* MMSSTV DSP + colour sources and dumps stage intermediates to JSON — mirroring `scratch/refvectors/build_feldhell.sh` + `feldhell_dump.cxx`.

> **F0 status (2026-07-06 — feasibility proven, TX landed).** The de-VCL strategy is validated end-to-end: `sstv.cpp` and `fir.cpp` compile and link **unmodified** on Linux g++ against a fake-`<vcl.h>` shim (`scratch/refvectors/sstv_shim/`: `vcl.h`, `ComLib.h`, `inifiles.hpp`, `Fir.h` case-shim), force-included with `-include`, `-Wno-unknown-pragmas -fpermissive`. Verified: `sstv.cpp` needs only `SYSSET sys` + `SampFreq`/`SampBase`/`CLOCKMAX` + Win32 `VirtualLock`/`VirtualUnlock` no-ops + a few ComLib string/`ABS` helpers (all in the shim); its GUI entanglement is zero (the `DrawGraph*` funcs in `fir.cpp` are the only VCL surface and get a no-op canvas). `build_sstv_tx.sh` + `sstv_tx_dump.cxx` drive the **unmodified** `CSSTVSET`/`CSSTVMOD` and emit `crates/dsp/tests/vectors/sstv_scottie1_tx.json` — the first authentic vector: Scottie 1 timing (`m_KS`=1524.096 = 138.24×11025/1000 ✓), the bit-exact VIS+line symbol sequence (VIS 0x3c → `[1300,1300,1100,1100,1100,1100,1300,1300]` ✓, porch 1500+0x2000, pixels @ 0.432 ms), and FP-tolerance PCM (VCO+BPF, stable checksum). **Remaining F0 work:** F0.3 RX dump (`CSSTVDEM::Do` → VIS detect + `DrawSSTVNormal` raster; entry points located: `Start()` sstv.cpp:1717, `Do()` 1819, VIS FSM 1890-2218, result via `SSTVSET.SetMode`), then extend the TX/RX drivers across all families at each T1.

- [ ] **F0.1 — Shim the VCL/Borland surface.** Create `scratch/refvectors/sstv_shim.h` providing the minimal non-DSP symbols `sstv.cpp`/`ComLib.cpp` need: `TColor`/`RGB` macros, `DWORD`/`BYTE`/`BOOL`/`LPCSTR`, `__fastcall`→empty, `#pragma pack` no-ops, a `sys`/`SSTVSET` config stand-in exposing `m_SampFreq=11025`, and stubs for the bitmap/GUI calls the DSP paths touch. **Do not modify the reference `.cpp`/`.h`** — the shim adapts around them (include-order + `-include sstv_shim.h`). Where a reference function is inseparable from VCL (`Graphics::TBitmap` raster writes in `Main.cpp`), reimplement *only that thin sink* in the harness (a plain row-major byte buffer) and route `DrawSSTVNormal`'s `ScanLine[y]` writes to it. Commit.
- [ ] **F0.2 — TX dump driver** `scratch/refvectors/sstv_tx_dump.cxx` + `build_sstv_tx.sh`: instantiate `CSSTVMOD`, drive one mode's `TMmsstv::LineXXX` generator over a fixed synthetic test image (a small deterministic gradient/colour-bar, defined in the driver so it is reproducible), and dump per stage: the VIS tone/duration list, the per-line `(freq,duration)` write sequence (the modulator's *symbol* domain — **bit-exact**), and the rendered PCM (`CSSTVMOD::Do` output, 11025 Hz — **FP tolerance**). Provenance header = upstream `8060b5f` + exact command. Commit.
- [ ] **F0.3 — RX dump driver** `scratch/refvectors/sstv_rx_dump.cxx` + `build_sstv_rx.sh`: feed the F0.2 PCM (or a canned WAV) into `CSSTVDEM::Do`, and dump: detected VIS byte + selected mode, the sync-timing decisions, and the final reconstructed raster from the `DrawSSTVNormal` byte sink (**bit-exact raster**). Commit.
- [ ] **F0.4 — Provenance + README.** Extend `crates/dsp/tests/vectors/README.md` with the SSTV convention (11025 Hz, the symbol-vs-audio split, the isolated-harness note + the "no reference CLI decoder" deviation). Commit.

**F0 exit:** `build_sstv_tx.sh` and `build_sstv_rx.sh` run on Linux from a clean checkout and emit valid JSON for at least Scottie 1; the harness links *unmodified* `sstv.cpp`/`ComLib.cpp`/`fir.cpp`.

## Task Group F1 — Shared DSP blocks (KAT-gated in isolation, before any mode uses them)

Each block is ported + unit-tested against an F0 dump **before** a mode consumes it (Doctrine: new blocks KAT-first).

- [ ] **F1.1 — FM subcarrier demodulator** `crates/dsp/src/frontend/fm_subcarrier.rs` (or reuse existing frontend NCO): port the reference RX chain `CPLL`/`CFQC`/`CHILL` well enough to turn a windowed audio stream into an instantaneous-frequency estimate, then map freq→pixel (inverse of `ColorToFreq`). Reuse existing `frontend` filters where they already exist; only transcribe the `CIIRTANK` resonator design (`fir.cpp:46-63`) if no equivalent exists. FP-tolerance KAT vs an F0 freq-track dump. Commit.
- [ ] **F1.2 — VIS codec** `crates/dsp/src/framing/sstv_vis.rs`: bit-exact encode/decode of the 8-bit, extended-16-bit (`0x23` marker), and 24-bit N-VIS forms, incl. the per-mode VIS↔mode map (transcribed from the reference). KAT: encode(mode)==golden VIS bytes and decode(golden)==mode, **bit-exact**, for every submode. Commit.
- [ ] **F1.3 — Sync detector** (line-timing): port `CSYNCINT`/`SyncFreq` behaviour to lock the 1200 Hz line-sync and drive the pixel-sampling grid. Unit-tested on an F0 line-timing dump. Commit.
- [ ] **F1.4 — Colour convert** `crates/dsp/src/modes/sstv_color.rs`: `YCtoRGB` / `GetRY` / `ColorToFreq(Narrow)` / `LimitRGB`, **integer-exact** ported with `// ref:` cites. Bit-exact KAT vs an F0 colour-table dump (sweep Y/R-Y/B-Y and RGB corners). Commit.

---

## Per-mode-family port tasks (instantiate the uniform T1–T9 template)

Run this sequence **once per family** (A→F). `<FAM>` = the family; the representative submode is the first listed. Submodes within a family are parametric: one `SstvMode` enum with a `params()` method returning the family's timing/geometry/colour-model from the verified tables, and a table-driven test enumerates every submode against the reference.

- [ ] **T1 — Extract golden vectors.** Run F0's TX+RX dumps for `<FAM>`'s representative + every submode's param row; commit `crates/dsp/tests/vectors/sstv_<fam>_*.json` with the provenance header (upstream `8060b5f`, source file:line, exact command). Commit.
- [ ] **T2 — VIS + geometry wiring.** Ensure `sstv_vis.rs` (F1.2) covers `<FAM>`'s VIS codes and `SstvMode::params()` returns `<FAM>`'s geometry from `GetBitmapSize`/`GetPictureSize`. KAT: VIS + geometry match golden. Commit.
- [ ] **T3 — (n/a for analog SSTV — no FEC).** SSTV carries no channel FEC; the "bit-domain" stages are VIS (T2) and the pixel raster (T5). Skip, noting it in the family doc comment.
- [ ] **T4 — Port the modulator** (`SstvMod` for `<FAM>`): emit VIS → per-line `(freq,duration)` write sequence from the `LineXXX` generator, driven by the source raster → `ColorToFreq`. **KAT: the per-line symbol (freq,duration) sequence bit-exact vs golden;** rendered audio within the committed FP tolerance. Commit.
- [ ] **T5 — Port the demodulator** (`SstvDemod` for `<FAM>`): FM demod (F1.1) → VIS detect (F1.2) → sync-locked pixel sampling (F1.3) → colour reconstruct (F1.4) → `FramePayload::Image { width, gray }`. **KAT: decoded raster bit-exact vs the F0 RX raster dump;** loopback (our TX image → our RX) reproduces the source within the committed pixel-error tolerance. Commit.
- [ ] **T6 — Register in the daemon.** `modes/mod.rs` `pub mod sstv;`; `ModeConfig::Sstv { submode, center_hz }` + `parse` arm (`SstvMode::from_label`); `registry.rs` demod + modulator arms; `SstvParams { submode, center_hz }` in `proto/omnimodem.proto` (`ModeParams` oneof). Registry unit test. Commit.
- [ ] **T7 — Conformance gates.** `#[ignore]`-gated cross-check (our TX audio → isolated MMSSTV decoder harness → source raster, within tolerance — behind the harness-binary env var, mirroring the FT8 gate); a `loopback.rs`/`ber.rs`-style sweep recording recovered-raster correlation vs AWGN SNR with a committed threshold. Because CI does not enable `testutil`, the bit-exact VIS + raster KATs **also** exist as plain `#[cfg(test)]` lib unit tests in `sstv.rs` (Doctrine §6 / harness note). Commit.
- [ ] **T8 — Wire into the TUI** (mandatory). Add each `<FAM>` submode as a `modeInfo` row in `clients/omnimodem-tui/internal/app/modes.go` with `shape: "image"` and `params: [{"center", 1500}]` (2044 for narrow N modes); extend `modeParamsFor` with the `SstvParams` oneof arm; regen Go proto via `clients/omnimodem-tui/gen.sh`; reuse the existing Phase-10 `image` raster view (`raster.go`/`view_operate.go`) — SSTV pushes full lines rather than 14-row columns, so verify `rasterBuf.push` handles the wider `width`. `go test ./...` green. Commit.
- [ ] **T9 — (phase close — see below).** Only after **all** families A–F are green.

---

## Phase close — single PR (Doctrine / per-phase PR)

- [ ] Branch: `feature/omnimodem-phase17-sstv` (already created), branched from merged `origin/main`.
- [ ] Exit gate: full workspace `cargo build` + `cargo test` green, TUI `go test ./...` green, every family's VIS+raster KAT bit-exact, loopback + AWGN sweeps meeting committed thresholds, **every submode selectable + operable in the TUI**.
- [ ] Open PR titled `Phase 17 — SSTV (Scottie/Martin/Robot/PD/Pasokon + MMSSTV MR/ML/MP/MN/MC)`; body: submodes ported, reference commit `8060b5f`, new blocks (FM-subcarrier demod, VIS codec, sync, colour convert, isolated MMSSTV vector harness), and conformance evidence (KAT + loopback/AWGN results). Request review. Pin `pr_url` in issue metadata.

**Build/test:** `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0 cargo test --workspace` (+ `--features testutil` locally for KATs; `--ignored` for the harness cross-check). TUI: `cd clients/omnimodem-tui && go test ./...`. **Never run `cargo fmt`.** Commit as `chrissnell` only — no AI attribution.

---

## Scope / open decisions

- **Full submode grid is in scope** (44 modes in `SSTVModeList[]`). If effort forces a cut, the defensible line is Groups A–D (classic interoperable modes) as the first PR and Groups E–F (MMSSTV-native MR/ML/MP/MN/MC) as a fast-follow — but each is still a complete, KAT-green family, never a stub. **Confirm with the issue owner before cutting.**
- **Repeater / FSK-ID / CW-ID / auto-slant (`ClockAdj`) features** of MMSSTV are operator conveniences, not part of the on-air picture format — **out of scope** for parity; note in the `sstv.rs` doc comment.
- **`DrawSSTVDiff` (differential/"Diff" render path)** is an MMSSTV display enhancement, not a distinct on-air mode — port `DrawSSTVNormal` only; note the deviation.
- **Colour vs display height:** Robot/AVT use `hp=240` picture height with `m_L` scan lines; PD/MP render two raster rows per scan. The `SstvMode::params()` must carry both `lines` and `rows_per_scan` so the `Image` raster is assembled at the correct height.

## Self-review
- **Coverage:** every entry in `SSTVModeList[]` (`sstv.cpp:493-503`) maps to a family group A–F; the narrow N-VIS modes (`IsNarrowMode`) are Group F. No submode dropped.
- **Reuse:** raster output reuses P0/Phase-10 `FramePayload::Image` + gRPC `Image` + the TUI `image` shape (no new payload/wire type). Only genuinely-new DSP is the FM-subcarrier demod + VIS/sync/colour blocks — each KAT-gated first (F1).
- **Doctrine compliance:** bit-exact only on VIS + raster + colour-integer; audio strictly FP-tolerance; no stubs (families stay open until green); provenance headers cite `8060b5f` + file:line; the "no Linux reference CLI" deviation is stated and substituted by the isolated-harness cross-check + loopback.
- **Risk:** F0 (de-VCL isolation of Borland source) is the critical-path risk. It is scoped as a standalone prerequisite with its own exit criterion so the modem port does not begin against unverified vectors.
