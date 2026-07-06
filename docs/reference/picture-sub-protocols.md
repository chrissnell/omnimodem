# Picture sub-protocols (MFSK / THOR / IFKP / FSQ)

> Wiki reference page (Phase 15). Its purpose is to let a future agent jump
> straight to the code. Mirror this into the GitHub wiki once the wiki is seeded
> (see "Publishing to the wiki" at the bottom).

fldigi's in-band **picture (image) sub-protocols** ported into omnimodem. These
are not new modems: each rides inside its already-ported text mode as a special
TX/RX state, entered by an in-band header in the text stream and then carrying
raw **pixel-FSK** — each 8-bit pixel is a frequency deviation held for a fixed
number of samples, no varicode, no FEC. Reference: fldigi 4.1.23 @ `61b97f413`
(`fldigi/src/{mfsk/mfsk-pic,thor/thor-pic,ifkp/ifkp-pic,fsq/fsq-pic}.cxx`).

## Where the code lives

| Concern | Location |
|---|---|
| Shared pixel-FSK engine (`PictureCodec`, `PixelScale`, `LumaKind`, `PlaneOrder`, `RasterRef`) | `crates/dsp/src/modes/picture.rs` |
| Analytic RX front-end (`design_hilbert`) | `crates/dsp/src/frontend/fir.rs` |
| MFSK picture state + header codec + `build_tx` | `crates/dsp/src/modes/mfsk_pic.rs` |
| THOR picture state | `crates/dsp/src/modes/thor_pic.rs` |
| IFKP picture state | `crates/dsp/src/modes/ifkp_pic.rs` |
| FSQ picture state (`FsqLinear`, directed header) | `crates/dsp/src/modes/fsq_pic.rs` |
| Colour raster payload | `FramePayload::Image { width, channels, pixels }` in `crates/dsp/src/types.rs`; proto `Image` in `proto/omnimodem.proto` |
| Daemon picture-send dispatch | `crates/omnimodemd/src/mode/picture_tx.rs` |
| gRPC transport (`TransmitImage`) | `proto/omnimodem.proto`; `crates/omnimodemd/src/grpc/service.rs`; `Command::TransmitImage` + `transmit_image()` in `crates/omnimodemd/src/core/{command,mod}.rs`; prebuilt-audio job in `crates/omnimodemd/src/core/tx_worker.rs` |
| TUI colour raster render (`▀` half-block) | `clients/omnimodem-tui/internal/app/imgrender.go` (`renderImageHalfBlock`) |
| Golden vectors | `crates/dsp/tests/vectors/{mfsk,thor,ifkp,fsq}pic.json` (drivers `scratch/refvectors/build_*pic.sh`) |
| Conformance | `crates/dsp/tests/ber.rs` (SNR sweep), `crates/dsp/tests/kat.rs` (`picture_cross_decode_doc`) |

## The shared engine

`PictureCodec` writes the wire math once; each family supplies four parameters:

- **Pixel scale** (`PixelScale`):
  - `Deviation256 { bandwidth_hz }` — MFSK/THOR/IFKP: `f = fc ± bw·(px−128)/256`.
  - `FsqLinear` — FSQ: TX `dev = −200 + px·1.5`, RX `byte = dev/1.5 + 128`. These
    affines are **deliberately not inverse** (fldigi's RX mixes at the carrier),
    so a clean loopback lands ~6 counts low. That is pinned by the golden vector,
    not treated as a bug.
- **Luma** (`LumaKind`): MFSK integer `(31R+61G+8B)/100` vs BT.601 `0.3/0.6/0.1`
  for the rest — transcribed verbatim, never unified.
- **Plane order** (`PlaneOrder`): R→G→B (MFSK/THOR/IFKP) vs B→G→R (FSQ).

### Rate-robust analytic RX front-end

`PictureCodec::decode` forms the **analytic** signal (real input + its Hilbert
quadrature via `design_hilbert`) *before* down-conversion, so the baseband has no
−(2fc+dev) image at any sample rate. This is what closed the IFKP (16 kHz) and
FSQ (12 kHz) RX loopbacks that a plain mix-then-low-pass front-end could not.
A short per-pixel settling guard skips the FM phase-step transient at pixel edges.

## Per-family cheat sheet

| | MFSK | THOR | IFKP | FSQ |
|---|---|---|---|---|
| Header (on air) | `\nSending Pic:WxH[C][pN];` | `pic%X\n` | ` pic%X` | directed `% X` |
| Size source | explicit W×H in header | fixed char table | fixed char table | fixed char table |
| Sample rate | 8 kHz | 8 kHz | 16 kHz | 12 kHz |
| Samples/pixel | 8 / 4 / 2 | 10 | 8 | 10 |
| Scale · luma · planes | Dev256 · MFSK · RGB | Dev256 · BT.601 · RGB | Dev256 · BT.601 · RGB | FsqLinear · BT.601 · BGR |

Avatar ('A', 59×74 RGB) is recognised but out of scope; the header char is
reserved, not claimed.

## Transmit and receive, headless

- **RX**: the picture RX state decodes the raster and emits a typed
  `FramePayload::Image` → `RxFrame.image` on the event stream.
- **TX**: each family has a `build_tx` assembler that renders the in-band header
  through the mode's live text modulator, then appends the pixel-FSK. The daemon
  path: `TransmitImage` gRPC → `mode::picture_tx::build` (maps the channel's
  configured `ModeConfig` onto the right assembler; MFSK takes any W×H, the
  others validate against the fixed size table) → `Command::TransmitImage` →
  `transmit_image()` enqueues the pre-built audio on the channel's TX worker,
  which plays it verbatim (resampled to the sink) and keys the rig.

## Colour on the `Image` wire

Phase 10's grayscale-only payload was extended once: `FramePayload::Image {
width, channels, pixels }` / proto `Image { width, pixels, channels }`, where
`channels` is 1 (grey) or 3 (RGB interleaved), `0 ⇒ 1` for older grayscale
producers. Grayscale Hell/WEFAX are unchanged.

## Tests / evidence

- **Bit-exact KATs** (plain lib tests): header build/parse, colour plane order,
  and the integer quantiser for each family, asserted against golden vectors
  transcribed from unmodified fldigi source.
- **Loopback**: grey + colour raster within a per-pixel tolerance across all four
  rates (the analytic front-end makes this rate-independent).
- **SNR sweep** (`ber.rs`, `--features testutil`): raster fidelity vs AWGN for
  MFSK (8 kHz) and IFKP (16 kHz), with committed regression floors.
- **fldigi cross-decode** (`kat.rs::picture_cross_decode_doc`): `#[ignore]`d
  bidirectional interop gate documenting the exact procedure per family.

## Publishing to the wiki

This repo's GitHub wiki was not yet initialised at authoring time (a wiki's
`.wiki.git` only exists after the first page is created via the web UI, and there
is no API to seed it). Once the wiki has any page, publish this content with:

```
git clone ssh://git@github.com/chrissnell/omnimodem.wiki.git
cp docs/reference/picture-sub-protocols.md <wiki>/Picture-Sub-Protocols.md
cd <wiki> && git add . && git commit -m "Picture sub-protocols reference" && git push
```
