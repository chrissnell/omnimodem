# Phase 15 — MFSK / THOR / IFKP / FSQ in-band picture sub-protocols

> Executable phase plan generated from the cited references per the Porting
> Doctrine in `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md`
> (§"The Porting Doctrine", T1–T9 template) and the Phase 15 line of
> `docs/plans/2026-07-02-omnimodem-fldigi-mode-parity.md`. Implement via
> subagent-driven-development; each tranche closes on a green gate, never a stub.

**Goal.** Port fldigi's in-band **picture (image) sub-protocols** for MFSK, THOR,
IFKP, and FSQ into omnimodem. These are not new modems: each rides inside its
already-ported text mode as a special TX/RX state, entered by an in-band header
and carrying raw pixel-FSK. The deliverable is picture TX/RX for each family,
reusing Phase 10's `FramePayload::Image` + the typed gRPC `Image` message + the
TUI `image` interaction shape — **extended once here to carry colour**.

**Reference.** `fldigi/src/mfsk/mfsk-pic.cxx` (+ `mfsk.cxx`), `fldigi/src/thor/thor-pic.cxx`
(+ `thor.cxx`), `fldigi/src/ifkp/ifkp-pic.cxx` (+ `ifkp.cxx`),
`fldigi/src/fsq/fsq-pic.cxx` (+ `fsq.cxx`), upstream **4.1.23 @ 61b97f413**.

**Depends on** (Doctrine §"Sequencing"):
- **Phase 8 MFSK** — landed (`modes/mfsk.rs`, registered). ✅ MFSK-pic unblocked.
- **Phase 9 THOR** — landed (`modes/thor.rs`, `ThorVariant`/`ThorParams`, registered). ✅ THOR-pic unblocked.
- **Phase 14 IFKP + FSQ (text)** — **not yet merged** (no `modes/ifkp.rs`/`modes/fsq.rs`;
  GRA-266 in progress). ⛔ **IFKP-pic and FSQ-pic are blocked** until their text
  modes land — they need the IFK/FSQ carrier, symbol clock, varicode header
  encode, and daemon registration those phases provide.
- **Phase 10 image framework** — landed: `FramePayload::Image { width, gray }`
  (`crates/dsp/src/types.rs`), typed `Image` gRPC message on `RxFrame`
  (`proto/omnimodem.proto:277-292`), TUI `image` shape (`clients/omnimodem-tui/internal/app/modes.go:21,86-90`).

**This phase therefore ships in two waves:** Wave A = MFSK-pic + THOR-pic (now);
Wave B = IFKP-pic + FSQ-pic (after Phase 14 merges). Both waves close in **one
phase PR** if Phase 14 lands during execution; otherwise Wave A ships first and
Wave B follows on the same branch. Sequencing is called out in §6.

---

## 1. What goes on the wire (from the references)

All four sub-protocols share one shape — **an in-band header in the text stream
switches the modem into a picture state, then each 8-bit pixel is sent as a raw
FSK deviation** (no varicode, no FEC), one frequency held for a fixed number of
samples per pixel. RX phase-differentiates the carrier back to a frequency, maps
it to a byte, and rasterises. The families differ only in the header syntax, the
pixel→frequency scaling, samples-per-pixel, sample rate, the phasing preamble,
and the colour-plane order. This shared core is the reusable block in §3.

### 1a. The common pixel codec (all four)

- **TX pixel → frequency.** MFSK/THOR/IFKP: `f = fc ± bandwidth·(px−128)/256`
  (MFSK `mfsk.cxx:1000-1002`; THOR `thor.cxx:1334,1354`; IFKP `ifkp.cxx:753,821`).
  FSQ differs: `f = fc − 200 + px·1.5` (Hz) (`fsq.cxx:1432,1452`).
- **RX frequency → pixel.** Phase-difference demod: mix to baseband at `fc`,
  FIR low-pass, `dφ = arg(conj(prevz)·currz)`, average over spp, scale back:
  MFSK/THOR/IFKP `byte = px·256/bandwidth + 128` then `CLAMP(0,255)`
  (THOR `thor.cxx:974`; IFKP `ifkp.cxx:556-579`); FSQ `byte = px/1.5 + 128`
  (`fsq.cxx:1206`). Pixel is `CLAMP`ed to 0..255.
- **Grayscale luma.** THOR/IFKP/FSQ use `0.3·R + 0.6·G + 0.1·B`
  (THOR `thor.cxx:1329-1331`; IFKP `ifkp.cxx:815-817`; FSQ `fsq.cxx:1426-1428`).
  **MFSK differs:** `(31·R + 61·G + 8·B)/100` (`mfsk-pic.cxx:244`) — transcribe
  each verbatim, do not unify.
- **Colour = separate planes**, one pixel per symbol, RGB order per row.
  **MFSK/THOR/IFKP:** plane order **R→G→B** (MFSK `mfsk-pic.cxx:198-202`;
  THOR `thor.cxx:1349-1362`; IFKP `ifkp.cxx:836-850`). **FSQ:** plane order
  **B→G→R** (`fsq.cxx:1445`, `RGB[]={2,1,0}` `fsq.cxx:1210`). Scan is row-major;
  for colour each row sends all-R then all-G then all-B (BGR for FSQ).

### 1b. Per-mode header, sizing, timing

| | MFSK | THOR | IFKP | FSQ |
|---|---|---|---|---|
| **SR** | 8000 | 8000 (THORFIRSTIF 2000) | 16000 | 12000 |
| **spp** | TXspp 8/4/2, RXspp from hdr | IMAGEspp 10 (`thor.h:68`) | IMAGESPP 8 (`ifkp.h:48`) | RXspp 10 (`fsq.cxx:273`) |
| **Header** | `"\nSending Pic:WxH[C][p N];"` — **explicit W×H in ASCII**, `C`=colour, `pN`=speed (`mfsk-pic.cxx:205-207,246-248`); RX matches `"Pic:"` in a 64-byte window (`mfsk.cxx:378`) | `"pic%X"` single mode char (`thor.cxx:399-432`) | `"\npic%X"` single mode char (`ifkp.cxx:377-420`) | directed `CALL% X` — `%` trigger (`fsq.cxx:603,876`) |
| **Size source** | header W×H (≤4095, `mfsk.cxx:400,419`) | fixed table by char (below) | fixed table by char | fixed table by char |
| **Colour flag** | `C` in header | char **case**: upper=colour, lower=grey | char case | mode char → `image_mode` 0-7 table (`fsq.cxx:876-902`) |
| **Preamble** | prologue/epilogue, viterbi delay 352 (`mfsk.cxx:64`) | PHASE_CORR=20 symbols @ `fc−0.6·bw` (`thor.cxx:1277,1311`) | 7×½-symlen @ `fc−0.6·bw` (`ifkp.cxx:796-803`) | PHASE_CORR=200 samples @ `fc−200` (`fsq.cxx:1411`) |
| **RX sync** | header match → delay counter → RX_STATE_PICTURE (`mfsk.cxx:497,856`) | FSM START→SYNC→IMAGE on sync thresholds `−0.59/−0.5·bw` (`thor.cxx:957-970`) | FSM on `−0.59/−0.51·bw` (`ifkp.cxx:565-575`) | `%`→state IMAGE (`fsq.cxx:911`) |

**THOR/IFKP fixed-size table** (mode char → W×H; upper=colour, lower=grey unless noted):
`T/t 59×74 · S/s 160×120 · L/l 320×240 · V/v 640×480 · F 640×480 grey ·
P/p 240×300 · M/m 120×150 · A 59×74 avatar-RGB` (THOR `thor.cxx:407-420`;
IFKP `ifkp-pic.cxx:299-310`). THOR/IFKP also define an **avatar** ('A', fixed
59×74 RGB) with its own RX callback — **out of scope for this phase** (leave the
header char reserved, do not claim it). **FSQ table** (`S L F V P p M m` →
`image_mode` 0-7, `fsq.cxx:876-902`, `fsq-pic.cxx:412-419`).

### 1c. Two equivalence classes (Doctrine §3)

- **Bit-exact (integer/pixel domain):** the header byte string; the pixel→byte
  and byte→pixel **integer** mappings (the `256/bandwidth`/`×1.5` scaling +
  `CLAMP` + luma weights); the colour-plane raster ordering; the RX pixel-index
  walk (`pixelnbr` arithmetic). Asserted byte-for-byte against golden vectors.
- **FP tolerance / loopback (audio domain):** the modulated audio (cosine NCO,
  FIR/moving-average filters, phase-difference demod) — entangled with fldigi's
  op-ordering and libm. **Never assert bit-exact audio.** The gate is a loopback
  whose *decoded raster matches the reference raster within a per-pixel
  tolerance* + an `#[ignore]` cross-decode against the fldigi binary.

---

## 2. Golden vectors (T1)

The `*-pic.cxx` files cannot link standalone (they drag in FLTK, the modem
runtime, and `ModulateXmtr`) — mirror the Phase 10 precedent
(`scratch/refvectors/feldhell_dump.cxx`): a standalone dump program that
**transcribes the pure-integer functions with `// ref:` cites** and links only
what compiles free-standing. Per family, capture:

1. **Header bytes** for a fixed `(W,H,colour,speed)` — MFSK the full ASCII
   string; THOR/IFKP/FSQ the `pic%X` / `% X` bytes.
2. **The colour-plane raster**: for a fixed small test image (e.g. an 8×4 RGB
   swatch), the exact ordered byte stream the TX would emit — MFSK/THOR/IFKP
   R→G→B row-major, FSQ B→G→R — plus the grayscale reduction using **that
   family's** luma weights.
3. **The pixel→byte round trip**: for pixel values `{0,1,64,127,128,192,255}`,
   the frequency (analytic) and the RX `byte` after the integer scale+CLAMP, so
   the quantiser is pinned exactly (esp. FSQ's `×1.5` vs the `256/bandwidth`
   families).

`scratch/refvectors/build_<mode>pic_*.sh` compiles + runs each dump (provenance
header = upstream commit `61b97f413` + exact command), mirroring
`build_feldhell.sh` / `build_dominoex_varicode.sh`. Output →
`crates/dsp/tests/vectors/<mode>pic_*.{snap,json}`. These are the bit-exact KATs
and, per Doctrine §5, **also run as plain lib unit tests** (CI has no `testutil`).

---

## 3. Shared picture-codec block (build first, KAT in isolation)

Before any mode uses it, add `crates/dsp/src/modes/picture.rs` (or
`framing/picture_fsk.rs`) — the reusable pixel-FSK core the four families
parametrise, so the wire math is written and tested **once**:

- `PictureParams { samplerate, spp, center_hz, bandwidth_hz, scale: PixelScale }`
  where `PixelScale` is either `Deviation256 { bandwidth }` (MFSK/THOR/IFKP) or
  `FsqLinear` (`fc−200 + px·1.5`) — the only two quantiser families.
- `fn tx_pixel_freq(px: u8, p: &PictureParams, reverse: bool) -> f32` and
  `fn rx_byte(freq_est: f32, p: &PictureParams, reverse: bool) -> u8` — the
  bit-exact integer edges (scale + `CLAMP`), cited to each reference line.
- `fn luma(r,g,b, weights: LumaWeights) -> u8` with the two weight sets
  (`{31,61,8}/100` for MFSK, `{0.3,0.6,0.1}` for the rest) — transcribed verbatim.
- `fn plane_raster(img, order: PlaneOrder, colour) -> Vec<u8>` producing the
  ordered TX byte stream (R→G→B or B→G→R, row-major) and the matching RX
  `pixelnbr` walk.
- A phase-difference demodulator helper (mix → `picfilter` FIR low-pass →
  `Cmovavg` over spp → `arg(conj·)` → scale) reusing `frontend` FIR/NCO; no
  per-sample alloc (`alloc_guard`).

**Gate:** bit-exact KAT of `tx_pixel_freq`/`rx_byte`/`luma`/`plane_raster`
against the §2 vectors for all quantiser+weight+order combinations. The audio
demod helper is loopback-only.

---

## 4. Extend the image payload/proto to carry colour (do before RX wiring)

Phase 10's `FramePayload::Image { width, gray }` and the gRPC `Image { width, gray }`
message are **grayscale-only**; all four picture modes transmit colour. Extend
both once, additively (grayscale Hell/WEFAX keep working):

- **DSP type** (`crates/dsp/src/types.rs`): add colour to `FramePayload::Image`
  — either a `channels: u8` (1 or 3) with `gray` reinterpreted as interleaved
  RGB when `channels==3`, or a sibling `rgb: Option<Vec<u8>>`. Prefer the
  `channels` field (smallest change; `gray.len()==width*rows*channels`). Update
  `hash_into`, `payload_kind` ("image"), and the `frame_bytes`/typed-message
  path. Keep existing grayscale callers unchanged (default `channels=1`).
- **Proto** (`proto/omnimodem.proto` `message Image`): add `uint32 channels = 3;`
  (default 0→treat as 1). Regenerate Rust + Go (`clients/omnimodem-tui/gen.sh`).
- **TUI `image` view**: render 3-channel rasters in colour (or a documented
  grayscale fallback) — extend the existing scrolling raster surface, don't add a
  new shape.

This is a genuine bit-domain addition (Doctrine §6: no stub) with its own unit
test (colour `Image` round-trips + hashes; grayscale unchanged).

---

## 5. Per-family tranches (T1–T9 each; families are parametric, not per-size)

Each family is **one** port task template run; the size table is a parametric
selector, not one task per size. Reuse §3 + §4; only the header codec, scaling
choice, luma weights, plane order, preamble, and RX sync FSM differ.

### 5A. MFSK-pic (Wave A — unblocked)
- **T1** header + raster + quantiser vectors from `mfsk-pic.cxx`/`mfsk.cxx`.
- **T2** header codec: TX build `"\nSending Pic:WxH[C][pN];"`; RX `"Pic:"`
  window parser → `(W,H,colour,RXspp)` (`mfsk.cxx:366-422`). Bit-exact KAT.
- **T3** n/a (no FEC on pixels).
- **T4** modulator: hook `modes/mfsk.rs` TX to emit header (varicode via the
  existing MFSK text path) → prologue → §3 pixel-FSK at TXspp; `Deviation256`,
  MFSK luma, R→G→B. Symbol/byte stream bit-exact vs vector; audio FP-only.
- **T5** demod: MFSK RX state machine → picture state on header match + delay
  (352) → §3 phase-diff demod → `FramePayload::Image`. Loopback raster matches.
- **T6** daemon: extend `ModeConfig` for MFSK so a picture TX is expressible
  (e.g. a picture-send control op / param); registry arm. Unit test.
- **T7** conformance: `#[ignore]` cross-decode vs fldigi MFSK16; raster
  match-rate-vs-SNR note in `ber.rs`.
- **T8** TUI: MFSK picture selectable via the `image` shape (colour); params
  (size, colour, speed). `go test ./...`.
- **T9** folded into the phase PR (§6).

### 5B. THOR-pic (Wave A — unblocked)
- Same template; reference `thor-pic.cxx`/`thor.cxx`. `pic%X` header codec
  (`thor.cxx:399-432`), `Deviation256`, THOR luma, IMAGEspp=10, PHASE_CORR=20
  preamble @ `fc−0.6·bw`, RX FSM START→SYNC→IMAGE (`thor.cxx:957-970`). Wire the
  picture state onto the existing `modes/thor.rs` IFK carrier. **Avatar out of
  scope** (reserve 'A'). Fixed-size table parametric.

### 5C. IFKP-pic (Wave B — blocked on Phase 14 `modes/ifkp.rs`)
- Same template; reference `ifkp-pic.cxx`/`ifkp.cxx`. `\npic%X` header, SR 16000,
  IMAGESPP=8, 7×½-symlen preamble, RX FSM on `−0.59/−0.51·bw`
  (`ifkp.cxx:565-575`). Rides the Phase-14 IFKP text mode.

### 5D. FSQ-pic (Wave B — blocked on Phase 14 `modes/fsq.rs`)
- Same template; reference `fsq-pic.cxx`/`fsq.cxx`. Directed `CALL% X` trigger
  (`fsq.cxx:603,876`), **`FsqLinear` quantiser** (`px·1.5`, `fsq.cxx:1432,1206`),
  FSQ luma, **B→G→R** plane order (`fsq.cxx:1445`, `RGB[]={2,1,0}`), PHASE_CORR=200
  @ `fc−200`, `image_mode` 0-7 size/colour table (`fsq.cxx:876-902`). Rides the
  Phase-14 FSQ directed-protocol layer (the picture header is a directed message).

---

## 6. Sequencing, gates, PR

- **Wave A (MFSK-pic + THOR-pic)** can start immediately: §2 vectors → §3 shared
  block → §4 colour extension → 5A/5B. This alone is a shippable phase increment.
- **Wave B (IFKP-pic + FSQ-pic)** starts once Phase 14 (GRA-266) merges to `main`
  and rebased in; 5C/5D reuse §3/§4 unchanged. If Phase 14 lands during Wave A,
  fold both into the single phase PR; otherwise ship Wave A first, Wave B as a
  follow-up commit on the same `feature/omnimodem-phase15-picture-subprotocols`
  branch (or a Phase 15b PR if the owner prefers a hard split).
- **Definition of done (per family, Doctrine §5 + T8):** header + quantiser +
  plane-raster bit-exact KATs green as **plain lib tests**; loopback raster
  within tolerance across every size; `#[ignore]` cross-decode vs fldigi;
  selectable + operable in the TUI `image` shape (colour). No stubs, no
  partially-wired size grid.
- Build/test env: `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0`; never
  run `cargo fmt`. `cargo test --workspace --locked` + `clippy -D warnings` +
  TUI `go test ./...` green before the PR. Commit as `chrissnell` only.
- Open `feature/omnimodem-phase15-picture-subprotocols`; PR body summarises the
  four sub-protocols, upstream commit `61b97f413`, the shared picture block, the
  colour extension to the image framework, and the KAT/loopback evidence.

## 7. Wiki

On completion, add a "picture sub-protocols" page pointing future agents at
`modes/picture.rs` (shared codec), the four modes' picture states, the colour
extension to `FramePayload::Image` / the `Image` proto message, and how the TUI
`image` shape renders colour rasters.

## 8. Decisions

- **Colour on the `Image` wire — RESOLVED (owner delegated the call, 2026-07-06).**
  Chose the ubiquitous raster-library model: an explicit `(width, channels,
  interleaved samples)` triple. `FramePayload::Image { width, channels, pixels }`
  and proto `Image { width, pixels, channels }` — `channels` 1=grayscale,
  3=RGB; the sample buffer was renamed `gray`→`pixels` (it now holds colour),
  wire field number 2 kept so it stays compatible; `channels==0` reads as 1 for
  older grayscale producers. TUI folds RGB to luma for the mono surface (true
  colour rendering is a follow-up). **Shipped** on this branch (§4 done).
- **Avatar ('A') mode:** THOR/IFKP define a fixed 59×74 RGB avatar with a
  distinct RX path — excluded here; fold in later only if wanted.
- **Wave B trigger:** whether to hard-split into a Phase 15b PR or keep one
  branch — depends on Phase 14's merge timing.
- **SSTV:** tracked separately in **GRA-289** (MMSSTV reference now in workspace);
  not part of this phase.
