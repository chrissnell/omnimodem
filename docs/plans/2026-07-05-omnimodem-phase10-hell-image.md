# Phase 10 — Hellschreiber (Feld Hell) + raster/image framework

> Executable phase plan generated from the cited references per the Porting
> Doctrine in `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md`
> (§"The Porting Doctrine", T1–T9 template) and the Phase 10 line of
> `docs/plans/2026-07-02-omnimodem-fldigi-mode-parity.md`. Implement via
> subagent-driven-development; each tranche closes on a green gate, never a stub.

**Goal.** Port fldigi's Hellschreiber family into omnimodem as bit-exact-compatible
modes **and** stand up the raster/image framework end-to-end (TUI `image`
interaction shape + a structured gRPC `Image` message that replaces P0's interim
`frame_bytes` flatten). Hellschreiber is a *facsimile* mode: characters are
painted as pixel columns from a bitmap font, never decoded to text on the wire —
so the "decode" output is a raster (`FramePayload::Image`), and this phase is as
much about the image pipeline as the modem.

**Reference.** `fldigi/src/feld/{feld.cxx, feldfonts.cxx, Feld*-{12,14}.cxx}`,
`fldigi/src/include/feld.h`, `fldigi/src/include/globals.h` (MODE_*HELL*),
upstream **4.1.23 @ 61b97f413**.

**Submodes in scope** (issue GRA-260): `FELDHELL`, `SLOWHELL`, `HELLX5`,
`HELLX9`, `HELL80`. (`FSKH245` / `FSKH105` appear in the roadmap but are *not* in
this issue's scope; leave hooks for them but do not claim them.)

---

## 1. What goes on the wire (from the reference)

Feld Hell scans each character as a stream of **pixel columns**. A transmit
column is `FELD_COLUMN_LEN = 14` rows tall (feld.h:42). For each column the modem
emits 14 on/off *pixel symbols* (one per row, LSB = row 0) at the pixel rate; a
character is a leading null column, its glyph columns (from the font), and a
trailing null column; a space is five null columns (feld.cxx:633-659).

The glyph bitmap comes from the selected font. fldigi's default TX font is
**FeldHell-12** (`progdefaults.feldfontnbr` default = 4 → `feldhell_12`;
configuration.h:989, feld.cxx:537). `get_font_data(c, col)` (feld.cxx:521-561)
returns the 14-bit column value or the "-1" completion signal.

### Per-submode parameters (feld.cxx:146-259, restart())

Sample rate is fixed at `FeldSampleRate = 8000`; `TxColumnLen = 14`;
`txpixrate = 14 * feldcolumnrate`; `upsampleinc = txpixrate / samplerate`;
baud = `0.5 * txpixrate`.

| Submode   | `feldcolumnrate` | Keying | Notes (feld.cxx) |
|-----------|------------------|--------|------------------|
| FELDHELL  | 17.5             | AM, raised-cosine on/off edges (`OnShape`/`OffShape`) | `hell_bw = txpixrate = 245`; :153 |
| SLOWHELL  | 2.1875           | AM, shaped | narrowband; :163 |
| HELLX5    | 87.5             | AM, **hard** (`Amp = currsymb`) | :173, feld.cxx:592-594 |
| HELLX9    | 157.5            | AM, **hard** | :183 |
| HELL80    | 35               | **FSK** ±`bandwidth/2` (=±150), `hell_bandwidth = 300` | :218; `cap |= CAP_REV`; feld.cxx:586-587 |

The AM path (FELDHELL/SLOWHELL) shapes rising/falling edges with a raised-cosine
`OnShape`/`OffShape` selected by `HellPulseFast` (feld.cxx:717-741); HELLX5/X9 key
hard. HELL80 is 2-FSK: the pixel bit shifts the tone by `±bandwidth/2`
(feld.cxx:586-587), with `phi2freq = samplerate / π / (bw/2)` used on RX.

### Two equivalence classes (Doctrine §3)

- **Bit-exact (integer/pixel domain):** the FeldHell-12 font glyph columns and the
  on-air column raster for a message. Asserted byte-for-byte.
- **FP tolerance / loopback (audio domain):** the modulated audio. fldigi's
  `send_symbol` audio path (nco, `OnShape`/`OffShape`, `ModulateXmtr`) is entangled
  with its modem/FLTK runtime and op-ordering; **never assert bit-exact audio.**
  The gate is a loopback whose *decoded raster columns match the reference glyph
  columns*, plus an `#[ignore]` cross-decode against fldigi.

---

## 2. Golden vectors (T1) — ✅ DONE

- `scratch/refvectors/feldhell_dump.cxx` links the **unmodified** fldigi font
  tables (`feld/feldfonts.cxx` + the fifteen `Feld*-{12,14}.cxx` it `#include`s)
  and transcribes the two pure-integer functions `get_font_data` (feld.cxx:521-561)
  and the `tx_char` column loop (feld.cxx:633-659) with cites — feld.cxx itself
  cannot link standalone (drags in fltk/modem/`ModulateXmtr`).
- `scratch/refvectors/build_feldhell.sh` compiles + runs it (mirrors
  `build_dominoex_varicode.sh`); provenance header names upstream commit + command.
- Output committed at `crates/dsp/tests/vectors/feldhell.json` (JSON-lines: a
  `_meta` provenance record, one `glyph` line per printable ASCII char, and one
  `stream` line per test message: `"CQ DE K1ABC"`, `"The quick brown fox …"`).
  Fields: `cols` are 14-bit column values, bit `r` set iff pixel row `r` is lit.

## 3. Font + framing port (T2) — ✅ DONE

`crates/dsp/src/framing/hellfont.rs`:
- `FONT`: FeldHell-12 table transcribed **verbatim** from `Feld*-12.cxx`
  (mechanically extracted → no transcription drift).
- `get_font_data(c, col) -> Option<u16>`: feld.cxx:521-561 reproduced byte-for-byte.
- `glyph_columns(c)`, `push_char_columns`, `on_air_columns(text, xmt_width)`:
  the bit-exact TX column raster (tx_char framing).
- Bit-exact unit KATs vs `feldhell.json` (run in plain `cargo test`; CI does not
  enable `testutil`). **5 tests green.**

Font scope: FeldHell-12 is the canonical/default TX font and the only one needed
for a bit-exact TX raster. Porting the other 14 fonts is a mechanical follow-on
(same extractor path) **only if** multi-font TX selection is later desired; it is
not required for the modes in scope.

---

## 4. Remaining tranches

### T4 — Modulator (`crates/dsp/src/modes/hell.rs`)
- `HellVariant { FeldHell, SlowHell, HellX5, HellX9, Hell80 }` with a `params()`
  method returning `(feldcolumnrate, keying, samplerate=8000)` and derived
  `txpixrate`, `upsampleinc`, `baud`, bandwidth (mirror the `DominoVariant` shape
  in `dominoex.rs`; `from_label`/`label`/`all` for the registry + TUI).
- `HellMod`: `text -> on_air_columns -> per-column 14 pixel symbols -> audio`.
  - AM (FeldHell/SlowHell): single carrier, amplitude = pixel bit, raised-cosine
    on/off edges (port `initKeyWaveform` OnShape/OffShape, feld.cxx:717-741).
  - AM hard (HellX5/X9): amplitude = pixel bit, no edge shaping.
  - FSK (Hell80): 2-FSK, tone `± bandwidth/2` per pixel bit (feld.cxx:586-587).
  - Reuse `frontend::nco`/`frontend::modulate` where they fit; add a small
    Hell-specific keyer for the amplitude-envelope path (no existing block covers
    shaped single-tone OOK). No per-sample alloc (`alloc_guard`).
- **Gate:** symbol/pixel-column sequence bit-exact vs `feldhell.json`; audio only
  sanity-bounded (never bit-exact).

### T5 — Demodulator (facsimile RX → raster)
- Port `feld::rx_process` → `mixer`/hilbert → `lpfilt` → per-pixel magnitude +
  peak-hold AGC → downsample to pixel rate → CLAMP to 0..255 → accumulate columns
  (feld.cxx:378-510). FSK path uses `FSKH_rx` (feld.cxx:312-376, arg-diff FM
  discriminator).
- Output `FramePayload::Image { width, gray }`: `width` = 14 rows (native TX
  column height); `gray` accumulates columns row-major. Emit incrementally so the
  TUI can scroll.
- **Gate (Doctrine §3):** loopback — the decoded raster columns (thresholded)
  reproduce the reference glyph columns for a known message across every submode;
  `#[ignore]` cross-decode against the fldigi binary.

### T6 — Daemon registration
- `crates/dsp/src/modes/mod.rs`: `pub mod hell;`.
- `crates/omnimodemd/src/mode/mod.rs`: `ModeConfig::Hell { variant, center_hz }`
  + `parse`/`to_mode_string`/`label` arms (mirror `DominoEx`).
- `crates/omnimodemd/src/mode/registry.rs`: `demod_kind` + `build_modulator` arms.
- Registry unit test.

### T7 — Raster/Image wire format (the deferred-from-P0 half)
- **Structured gRPC `Image` message** in `proto/omnimodem.proto`, designed against
  Hell's *continuous column stream* and reused by WEFAX (Phase 12) + picture
  sub-protocols (Phase 15): carry `width`, `gray` (row-major 8-bit), and enough
  framing to append columns incrementally (e.g. a `column_start`/`is_delta` or a
  running row offset). Add it to `RxFrame` (typed field) and replace the interim
  `frame_bytes` 2-byte-BE-width flatten in
  `crates/omnimodemd/src/core/rx_worker.rs` with it. `HellParams { variant,
  center_hz }` in the `ModeParams` oneof.
- Keep `FramePayload::Image` (P0) as the DSP-side type; the proto message is the
  wire representation.

### T8 — TUI `image` interaction shape (mandatory)
- New `image` shape in `clients/omnimodem-tui/internal/app/`: a scrolling raster
  view (alongside `chat`/`sequencer`/`beacon`), rendering the `Image` proto
  columns as they arrive.
- `internal/app/modes.go`: five Hell rows with `shape: "image"` + `{center}` (and
  Hell80 note). `modeParamsFor` arm → `HellParams`. Regenerate Go proto
  (`clients/omnimodem-tui/gen.sh`) since T7 added a params + Image message.
- Extend `TestAllDaemonModesAreExposed` and add a Hell params round-trip test.
  `go test ./...` green.

### T9 — Close the phase
- Full `feldhell.json` KAT in `crates/dsp/tests/kat.rs` (every glyph + both
  streams, `--features testutil`); loopback + `#[ignore]` cross-decode; a BER/
  raster-fidelity sweep note in `ber.rs` (raster match-rate vs SNR, not text BER).
- `cargo test --workspace --locked` + `clippy -D warnings` green; TUI `go test`
  green; every submode selectable in the TUI. Open the phase PR
  `feature/omnimodem-phase10-hell-image`, body summarizing modes + reference
  commit + the image-framework change.

---

## 5. Sequencing & status

Phase 9 (DominoEX, #65) merged to `main` — this phase's gate is cleared. T1–T3
(golden vectors, extractor, font+framing port, bit-exact green) are **done** on
`feature/omnimodem-phase10-hell-image`. Remaining: T4 modulator, T5 raster demod,
T6 registration, T7 typed `Image` proto, T8 TUI `image` shape, T9 gates + PR.
Build/test env: `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0`; never run
`cargo fmt`.

## 6. Wiki

On completion, add a Hellschreiber/raster-framework page to the wiki pointing
future agents at `modes/hell.rs`, `framing/hellfont.rs`, the `Image` proto
message, and the TUI `image` shape.
