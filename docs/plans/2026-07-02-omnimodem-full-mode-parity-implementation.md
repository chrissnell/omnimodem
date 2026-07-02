# Omnimodem Full Mode Parity (fldigi + WSJT-X + JS8) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring omnimodem to full mode parity with fldigi, WSJT-X, and JS8Call by **porting each mode from its upstream reference implementation** — not clean-room reimplementing — so every mode is bit-exact-compatible on the air and cross-decodes with the reference binary in both directions.

**Architecture:** Each mode is a self-contained assembly under `crates/dsp/src/modes/` that wires existing/new building blocks into a `Demodulator`/`BlockDemodulator` + `Modulator`, then one arm in `crates/omnimodemd/src/mode/registry.rs`. The **port** is verified stage-by-stage against golden vectors extracted from the reference, plus an end-to-end bidirectional cross-decode gate. This is a **program of independent phases** (per the writing-plans scope check): a shared porting/verification harness (Phase P0), then two parallel mode tracks — fldigi (Phases 7–14) and WSJT-X/JS8 (Phases W1–W5). Each phase produces shippable, tested modes on its own and gets its own bite-sized executable plan when picked up; **this document is the master plan** — it specifies P0 in full and defines the uniform per-mode port task template + per-phase scope that the phase plans instantiate.

**Tech Stack:** Rust (edition 2021, workspace). Reuses the `omnimodem-dsp` blocks (`frontend`, `sync`, `fec`, `framing`, `ensemble`) and the conformance harness in `crates/dsp/tests/` (`kat.rs`, `ber.rs`, `loopback.rs`, `roundtrip.rs`, `vectors/`, `testutil`). Reference sources are checked out in-tree: `fldigi/src/`, `wsjtx/lib/`, `js8call/`.

**Spec:** `docs/plans/2026-07-02-omnimodem-fldigi-mode-parity.md` (the parity roadmap — gap analysis, per-mode reference mapping, building-block list). Read it first.

---

## The Porting Doctrine (read before any task)

This is the non-negotiable core of the plan. "Perfect compatibility" means we **replicate the reference, we do not reinterpret it.**

1. **One named reference file per mode.** Every mode task cites the exact upstream file(s) (tables below). That file is the source of truth for constants, tables, bit orderings, symbol/tone maps, sync/preamble patterns, timing, and pre-emphasis/pulse shaping. When the reference and a published spec disagree, **the reference wins** (fldigi/WSJT-X/JS8 interop is the goal, not the spec).

2. **Port the behavior, write it as Rust — not a transliteration.** We replicate what goes on the wire, expressed in idiomatic Rust. Transcribe **data verbatim** (varicode tables, interleave permutations, FEC generator/parity matrices, sync/Costas patterns, tone maps) with a `// ref: <file>:<lines>` cite — never "clean up" a table. But re-express the **code** the Rust way: the references are stateful C++ modem objects (fldigi) and Fortran with COMMON blocks + 1-based arrays (WSJT-X) — a line-by-line transliteration of either is *non-idiomatic and forbidden*. See "What idiomatic-Rust porting means" below.

3. **Two classes of equivalence — do not conflate them.** Getting this wrong is the most likely correctness trap in the whole program:
   - **Bit-exact (integer / bit domain):** varicode output, FEC codewords, interleaver permutations, symbol/tone *indices*, sync words, packed 77-bit messages, CRCs. These are deterministic and **must match the reference byte-for-byte**.
   - **Numerically close (floating-point DSP):** modulated audio samples, filter/NCO/AGC outputs, soft LLRs. FP op-ordering and libm `sin`/`cos` differ across C/Fortran/Rust, so **bit-exact audio is not a realistic target and must not be asserted.** Gate these on a tight tolerance (e.g. max abs error / correlation vs the reference waveform) **and** on the decisive end-to-end cross-decode.

4. **Stage-level golden vectors, not just end-to-end**, so a break localizes to one stage. Extract from the reference at every pipeline boundary — TX: message → source-encode → FEC → interleave → symbol/tone indices → audio; RX: reference soft metrics / decoded intermediates where extractable. Vectors land in `crates/dsp/tests/vectors/<mode>_*.{snap,json}` with a provenance header (reference commit + exact generating command); P0 gives the extraction tooling.

5. **Two gates per mode (Definition of Done):**
   - **KAT parity:** every bit-domain stage matches its golden vector byte-for-byte; every FP stage is within its committed tolerance. (`kat.rs`)
   - **Bidirectional cross-decode:** our TX decodes in the reference binary AND the reference's TX decodes in ours (`#[ignore]`-gated behind the reference binary in `kat.rs`), plus the BER/decode-rate curve meets the committed threshold at every SNR point (`ber.rs`, AWGN + Watterson CCIR). A loopback round-trip (`loopback.rs`) is necessary but **not** sufficient.

6. **No stubs, ever — a task is done only when its gate is green.** This is a hard rule, not a preference:
   - No `todo!()`, `unimplemented!()`, `unreachable!()` on any reachable path; no `panic!("not yet")`; no functions that return a canned/zeroed result to make a test pass.
   - No silently-partial submode grids: if a family claims MFSK8/16/32, every listed submode is wired and table-tested, or it is not listed.
   - A port task closes on a **passing conformance gate against the reference vector**, never on "it compiles" or "loopback works." A stage that cannot yet pass its gate stays open — it is not merged behind a stub.
   - `crates/dsp` already runs an `alloc_guard` / no-per-sample-alloc discipline and full CI KAT/BER; ported code meets that bar or it does not land.

7. **Provenance in code.** Each mode file opens with a doc comment: reference file(s), upstream commit hash, and any deviation (there must be none affecting the wire; if an FP tolerance is chosen, state it and why).

### What idiomatic-Rust porting means (concrete)

Preserve the *observable behavior* of the reference; discard its *code shape*. Applied to this codebase:

- **Map onto the existing traits, not new god-objects.** fldigi's `modem` subclasses and WSJT-X's `decodeXXX` subroutines become our `Demodulator`/`BlockDemodulator` + `Modulator` impls (`crates/dsp/src/mode.rs`), reusing `frontend`/`sync`/`fec`/`framing`/`ensemble`. No mutable global state — per-instance owned scratch buffers (the trait already forbids per-sample allocation).
- **Types:** `Complex32`/`f32` (`crate::types`), not hand-rolled real/imag pairs; `Llr`/`SoftBits` for soft info; `FramePayload` variants for output. Fortran 1-based indexing is rebased to 0; `integer*1` bit arrays become `&[u8]`/`bitvec` as appropriate.
- **Control flow:** iterators/slices/`chunks`/`windows` over C-style index loops; `enum` submodes with a `params()` method over `switch (mode_id)`; `Result<_, ModError>` over sentinel return codes; `const` tables (`static [...]`) over runtime-initialized arrays.
- **What must NOT change:** the arithmetic that determines a bit-domain output (integer ops, table lookups, bit orderings, rounding/scaling that feeds a slicer). Where the reference's exact operation order matters to an FP result within tolerance, keep that order and note it. Idiom is for structure; it never changes the wire.

### The uniform per-mode port task template

Every mode in every phase is ported with this same task sequence (the phase plans expand `<MODE>` with real code from the cited reference):

- [ ] **T1 — Extract golden vectors.** Run the P0 extraction recipe against `<reference file>` for a fixed test message; commit `vectors/<mode>_*.{snap,json}` with provenance header. Commit.
- [ ] **T2 — Port the source/char codec** (varicode / JSC / Baudot / 77-bit pack) with `// ref:` cites. Failing KAT test asserting encode == golden vector → implement → pass. Commit.
- [ ] **T3 — Port the FEC + interleave** (reuse existing `fec::*` where the code family already exists; transcribe generator/parity tables otherwise). KAT: encoder output == golden vector, **bit-exact** (bit-domain stage). Commit.
- [ ] **T4 — Port the modulator** (symbol/tone *indices* → `frontend::modulate`/NCO, pulse shaping). KAT: symbol/tone-index sequence **bit-exact** vs golden vector; modulated audio within the committed **FP tolerance** (never asserted bit-exact — see Doctrine §3). Commit.
- [ ] **T5 — Port the demodulator** (front end → sync → soft demap → FEC decode → unpack). Loopback round-trip at high SNR passes; where extractable, soft-metric KAT vs reference within tolerance. Commit.
- [ ] **T6 — Register the mode in the daemon** (`modes/mod.rs` `pub mod`, `ModeConfig` variant + `parse`, `registry.rs` arms; a params proto message in `proto/*.proto` if the mode has tunable params). Registry unit test. Commit.
- [ ] **T7 — Conformance gates.** Add the `#[ignore]` bidirectional cross-decode test + the `ber.rs` decode-rate sweep with a committed threshold. Run the AWGN/Watterson sweep; record the curve. Commit.
- [ ] **T8 — Wire the mode into the TUI** (mandatory — a mode is not "done" until it is selectable and usable in `clients/omnimodem-tui`). Add a `modeInfo` row to `internal/app/modes.go` (label + interaction `shape` + editable `params`), extend `modeParamsFor` with the mode's proto-oneof arm, regenerate the Go proto (`clients/omnimodem-tui/gen.sh`) if T6 added a params message, and add/extend the Go test (`modes`/`view_operate` tests). For a mode whose interaction differs from the existing `chat`/`ft8` shapes (image/fax → new `image` shape; a directed-protocol mode like FSQ/JS8 → its own shape), add that shape's view. `go test ./...` green. Commit.
- [ ] **T9 — Close the phase with a PR** (see "Per-phase PR" below): ensure the whole workspace builds and `cargo test` + TUI `go test ./...` are green, then open the PR for the phase branch.

Submode grids (Olivia/Contestia/MFSK/THOR/DominoEX/PSK-rate/Q65/JS8/…) are **parametric**: port the family once (T1–T8 on one representative submode), then a table-driven test enumerates every submode's parameters against the reference's parameter table, and the TUI exposes the submode selector. One task per family, not per submode.

**Definition of done for a mode now includes T8:** bit-exact/tolerance KAT parity, bidirectional cross-decode, committed BER curve, **and** the mode is selectable + operable in the TUI with its params. A daemon-only mode that the TUI can't drive is not done.

---

## Phase P0 — Porting & verification harness (shared prerequisite, do first) — ✅ landed

**Status:** implemented on branch `feature/mode-parity-p0-harness` (P0.1 + P0.2 done, verified: `cargo test -p omnimodem-dsp --lib` = 236 passed incl. the new Image test). P0.3 doc note is folded into the vectors README. **TUI:** no change required — P0 adds no *selectable mode*, only the DSP `Image` payload type and an interim opaque encoding; the TUI mode registry and proto are untouched (the `image` view + typed proto arrive with the first facsimile mode, Phase 10). **PR:** ready on the P0 branch, blocked only by sandbox push access (see "Per-phase PR").

**Goal:** the tooling every later phase depends on: the image/raster payload the picture/fax modes need, and the reference-vector extraction convention. This is the one phase written to full bite-sized granularity here; it unblocks all others.

**Files:**
- Create: `crates/dsp/tests/vectors/README.md` (provenance/extraction convention) — ✅
- Create: `scratch/refvectors/` extraction driver programs that dump reference intermediates (scratch per CLAUDE.md) — per-family, authored at each phase's T1
- Modify: `crates/dsp/src/types.rs` (add `FramePayload::Image`) — ✅
- Modify: `crates/omnimodemd/src/core/rx_worker.rs` (`frame_bytes` Image arm) — ✅
- Test: `crates/dsp/src/types.rs` inline tests — ✅

**gRPC surfacing — deliberately deferred to Phase 10 (YAGNI).** There are zero `Image` producers until the first facsimile mode (Hell, Phase 10), and the raster semantics differ by mode (Hell emits a continuous column stream; WEFAX a full IOC-scaled frame). Adding a structured proto `Image` message now would be speculative surface with no producer. Instead `frame_bytes` flattens `Image` losslessly onto the existing opaque `RxFrame.data` (2-byte big-endian `width` prefix + row-major gray), and the structured proto message is designed alongside Phase 10 when the semantics are known.

### Task P0.1 — Image/raster payload

- [ ] **Step 1: Write the failing test** (`crates/dsp/src/types.rs`, in `#[cfg(test)] mod tests`)

```rust
#[test]
fn image_payload_roundtrips_and_hashes() {
    let img = FramePayload::Image { width: 3, gray: vec![0, 128, 255, 1, 2, 3] }; // 2 rows × 3 cols
    let f = Frame { payload: img.clone(), meta: FrameMeta::default() };
    assert_eq!(f.payload, img);
    // hash_into must cover the new variant (dedup key)
    let mut h = std::collections::hash_map::DefaultHasher::new();
    f.payload.hash_into(&mut h);
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test -p omnimodem-dsp types::tests::image_payload_roundtrips_and_hashes`
Expected: FAIL — no `Image` variant.

- [ ] **Step 3: Add the variant + hash arm** (`crates/dsp/src/types.rs`)

```rust
// in enum FramePayload:
    /// Raster/scanline image (Hell, WEFAX, MFSK/THOR/IFKP/FSQ picture). `gray`
    /// is row-major 8-bit luminance, `gray.len() == width * rows`.
    Image { width: u16, gray: Vec<u8> },
```
```rust
// in hash_into match:
            FramePayload::Image { width, gray } => {
                4u8.hash(h);
                width.hash(h);
                gray.hash(h);
            }
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test -p omnimodem-dsp --lib types::tests`
Expected: PASS. Then satisfy the two other exhaustive matches the new variant touches: `payload_kind` in `crates/dsp/src/modes/afsk1200.rs` (add `Image { .. } => "image"`) and `frame_bytes` in `crates/omnimodemd/src/core/rx_worker.rs` (width-prefixed encoding, see gRPC note above). `tx_worker`/`wavtool` need no change (they construct payloads or use a wildcard).

- [ ] **Step 5: Commit**

```bash
git add crates/dsp/src/types.rs crates/dsp/src/modes/afsk1200.rs crates/omnimodemd/src/core/rx_worker.rs
git commit -m "feat(dsp): add Image raster FramePayload for facsimile/fax modes"
```

### Task P0.2 — Reference-vector extraction convention

- [ ] **Step 1:** Write `crates/dsp/tests/vectors/README.md` documenting the existing `.snap`/`.json` provenance-header format (see `ft8_reference.json`, `mfsk16.snap`) and the rule: every new vector names its reference source, upstream commit, and the exact command that produced it.
- [ ] **Step 2:** For each reference toolchain, capture a minimal extraction recipe under `scratch/refvectors/`:
  - fldigi (C++): a small `main()` linking the mode's `.cxx` that prints varicode/encoder/symbol output for a fixed input.
  - WSJT-X (Fortran): reuse the mode's `*sim`/`*code`/`gen*` programs already in `wsjtx/lib/` (e.g. `msk144sim`, `jt4code`, `qratest`) to dump intermediates.
  - JS8Call: the `jsc`/`JS8` units for the JSC codec; `lib/js8*` for symbol vectors.
- [ ] **Step 3: Commit** the README + recipes.

```bash
git add crates/dsp/tests/vectors/README.md scratch/refvectors/
git commit -m "docs(test): reference-vector extraction convention for mode ports"
```

### Task P0.3 — Cross-decode runner note

- [ ] **Step 1:** Extend the `#[ignore]` gate doc in `crates/dsp/tests/kat.rs` header to name the reference binaries per mode family (fldigi CLI, `jt9`/`wsjtx` decoders, JS8Call) and the env var that points at them, mirroring the existing FT8 gate. Commit.

**P0 exit criterion:** `Image` payload lands and threads through gRPC; the vector convention + one working extraction recipe per toolchain exist; `cargo test -p omnimodem-dsp --features testutil` is green.

---

## fldigi track — reference `fldigi/src/`

Each phase below instantiates the **per-mode port task template** (T1–T9) for its modes. File structure is uniform: `Create: crates/dsp/src/modes/<mode>.rs`, `Modify: crates/dsp/src/modes/mod.rs`, `crates/omnimodemd/src/mode/{mod.rs,registry.rs}`, `proto/*.proto` (params message, if any), `crates/dsp/tests/{kat.rs,ber.rs,loopback.rs,vectors/}`, **and the TUI: `clients/omnimodem-tui/internal/app/modes.go` (+ `internal/pb` regen, + a new view under `internal/app/` for any non-`chat`/`ft8` shape)**. New building blocks are listed per phase and each is KAT-gated in isolation **before** any mode uses it.

### Phase 7 — PSK family
- **Reference:** `fldigi/src/psk/{psk.cxx,pskvaricode.cxx,pskcoeff.cxx,pskeval.cxx}`.
- **New block:** `frontend/multicarrier.rs` (N parallel carriers) — KAT first.
- **Modes (port tasks):** BPSK rates (PSK63/125/250/500/1000, parametric over `psk31`) · `+F` FEC variants (add `fec::conv` layer) · QPSK31–500 (differential QPSK + Viterbi) · PSK-R robust + multi-carrier `nX_PSK*R` · (8PSK → Phase 14). Varicode is ported verbatim from `pskvaricode.cxx`; RX matched filter coeffs from `pskcoeff.cxx`.

### Phase 8 — MFSK + Contestia
- **Reference:** `fldigi/src/mfsk/{mfsk.cxx,mfskvaricode.cxx,interleave.cxx}`; `fldigi/src/contestia/contestia.cxx`.
- **New block:** `framing/mfsk_varicode.rs` (port `mfskvaricode.cxx`). Contestia reuses `fec::fht` (32-Walsh) parametrically, like `olivia`.
- **Modes:** MFSK family (MFSK8/16/32/4/11/22/31/64/128/64L/128L, parametric) · Contestia submode grid (parametric). Text first; the MFSK picture sub-protocol (`mfsk-pic.cxx`) is Phase 15.

### Phase 9 — IFK: DominoEX + THOR
- **Reference:** `fldigi/src/dominoex/{dominoex.cxx,dominovar.cxx}`; `fldigi/src/thor/{thor.cxx,thorvaricode.cxx}`.
- **New block:** `modes/ifk.rs` (incremental-frequency-keying tone tracker) + `framing/ifk_varicode.rs`. Port DominoEX first (no FEC), then THOR reuses the IFK core + adds `fec::conv` + interleave + soft decode.
- **Modes:** DominoEX Micro/4/5/8/11/16/22/44/88 · THOR Micro/4/5/8/11/16/22/25x4/50x1/50x2/100 (both parametric).

### Phase 10 — Image framework + Hellschreiber
- **Reference:** `fldigi/src/feld/{feld.cxx,feldfonts.cxx,Feld*-14.cxx}`.
- **New block:** `modes/hell.rs` + `framing/hellfont.rs` (port the bitmap fonts verbatim). Uses `FramePayload::Image` from P0.
- **Modes:** Feld Hell, Slow Hell, Hell X5/X9, FSK-Hell 245/105, Hell 80. Output is a glyph raster; the "golden vector" is the column/pixel stream for a known text.
- **TUI (T8) + wire format:** this phase introduces the **`image` interaction shape** in the TUI (a scrolling raster view) and the **structured gRPC `Image` message** (deferred from P0) — designed here against Hell's continuous column stream and reused by WEFAX (Phase 12) and the picture sub-protocols (Phase 15). The daemon's `frame_bytes` interim encoding is replaced by the typed message in the same change.

### Phase 11 — MT63
- **Reference:** `fldigi/src/mt63/{mt63.cxx,mt63base.cxx,dsp.cxx}`.
- **New block:** `frontend/ofdm.rs` (64-carrier overlapping-Walsh OFDM + overlap-add + deep interleave) — the heaviest new block; KAT against `mt63base.cxx` intermediates first.
- **Modes:** MT63-500/1000/2000 × Short/Long (parametric over carrier count + interleave depth).

### Phase 12 — NAVTEX/SITOR-B + WEFAX
- **Reference:** `fldigi/src/navtex/navtex.cxx`; `fldigi/src/wefax/{wefax.cxx,wefax-pic.cxx}`.
- **New blocks:** `fec/ccir476.rs` (CCIR-476 / SITOR FEC-B, 4-of-7, time-diversity) for NAVTEX; `modes/wefax.rs` (FM demod + IOC 576/288 phasing) emitting `FramePayload::Image`.
- **Modes:** NAVTEX, SITOR-B, WEFAX-576, WEFAX-288.

### Phase 13 — RSID (detect **and** transmit)
- **Reference:** `fldigi/src/rsid/{rsid.cxx,rsid_defs.cxx}`.
- **New block:** `frontend/rsid.rs` — RSID (Reed-Solomon Identifier) **both directions** per the scope decision: an RX detector that reports the identified mode + audio offset (feeding daemon auto-switch), and a TX encoder that emits the RSID burst ahead of a transmission. Its own phase because it is cross-cutting (touches every mode's mode-ID table and the daemon's mode-switch path), not a single on-air text mode.
- **Deliverables:** RSID encode + detect KAT against `rsid_defs.cxx`'s ID table; daemon hook to (a) announce the active mode's RSID on TX when enabled, (b) surface a detected RSID as a control event. Sequence after the first MFSK-family modes exist (Phase 8) so there is something to identify/switch to.

### Phase 14 — FSQ + IFKP (text)
- **Reference:** `fldigi/src/fsq/{fsq.cxx,fsq_varicode.cxx}`; `fldigi/src/ifkp/{ifkp.cxx,ifkp_varicode.cxx}`.
- **New block:** FSQ directed/selective-call protocol layer (FSQCALL: triggers, directed messages, heard-list). IFKP reuses the Phase-9 IFK core + its own varicode.
- **Modes:** FSQ · IFKP (text first; their picture sub-protocols are Phase 15).

### Phase 15 — Picture / image sub-protocols
- **Reference:** `fldigi/src/mfsk/mfsk-pic.cxx`, `fldigi/src/thor/thor-pic.cxx`, `fldigi/src/ifkp/ifkp-pic.cxx`, `fldigi/src/fsq/fsq-pic.cxx`.
- **Depends on:** the text modes from Phases 8/9/14 and P0's `FramePayload::Image` — plus the structured gRPC `Image` message (designed in Phase 10 for Hell/WEFAX, reused here).
- **Modes:** the in-band picture TX/RX for MFSK, THOR, IFKP, FSQ. Added as its own phase per the scope decision ("add images as a later phase"), so the text modes ship first.
- **SSTV:** answered below — **not** an fldigi/WSJT-X mode, so it is out of the current parity scope. It can be added here as an extra image mode **if** an open-source SSTV reference is added to the workspace (QSSTV / slowrx / pySSTV — none is a workspace repo today). Flagged for a decision, not assumed in scope.

### Phase 16 — Long tail
- **Reference:** `fldigi/src/throb/throb.cxx`; 8PSK + OFDM data modes in `fldigi/src/psk/`.
- **Modes:** Throb1/2/4, ThrobX1/2/4 · 8PSK125–1200F · OFDM-500F/750F/2000F/2000/3500 (reuse the Phase-11 OFDM core). Lowest priority.

---

## WSJT-X / JS8 track — reference `wsjtx/lib/`, `js8call/`

Independent of the fldigi track (reuses the FT8 windowed path — STFT → Costas → LDPC+OSD → 77-bit — not fldigi blocks), so it runs in parallel. Same T1–T7 template; the "modulated audio" golden vectors come from the reference `gen*`/`*sim`/`*code` programs already in `wsjtx/lib/`.

### Phase W1 — FST4 / FST4W
- **Reference:** `wsjtx/lib/fst4/` — `genfst4.f90`, `gen_fst4wave.f90`, `fst4_decode.f90`, `encode240_101.f90` + `ldpc_240_101_{generator,parity}.f90`, `encode240_74.f90` + `ldpc_240_74_{generator,parity}.f90`, `get_crc24.f90`.
- **New block:** `fec/ldpc_fst4.rs` — transcribe the **two** LDPC codes FST4/FST4W use: **(240,101)** (standard, 77-bit + CRC24) and **(240,74)** (the low-`Keff` variant). Reuse the existing LDPC BP decoder + OSD; add these generator/parity tables + the CRC-24. 4-GFSK waveform + long/selectable T/R periods.
- **Modes:** FST4 (QSO) and FST4W (beacon), all T/R periods (15/30/60/120/300/900/1800 s) — the block demod's `window_s`/`period_s` become parametric.

### Phase W2 — MSK144
- **Reference:** `wsjtx/lib/{decode_msk144.f90,genmsk_128_90.f90,msk144code.f90,msk144decodeframe.f90}`.
- **New block:** `frontend/msk.rs` (offset-MSK/OQPSK waveform + matched filter); reuses LDPC(128,90) via the BP decoder with ported tables.
- **Modes:** MSK144 (streaming/ping-buffered demod — short bursts, not the 15 s grid).

### Phase W3 — Q65
- **Reference:** `wsjtx/lib/{q65_decode.f90,q65params.f90,qra64code.f90}`, `wsjtx/lib/qra/` (qra65, qracodes).
- **New block:** `fec/qra65.rs` — the QRA65 (Q-ary Repeat-Accumulate) soft decoder; the one genuinely new FEC family. KAT against `qratest` output first.
- **Modes:** Q65 submodes A–E × T/R periods (parametric).

### Phase W4 — JT4
- **Reference:** `wsjtx/lib/{jt4.f90,jt4_decode.f90,jt4code.f90}`.
- **New block:** none — reuses `fec::fano` (K=32) + 4-FSK front end already present for JT9.
- **Modes:** JT4 submodes A–G (tone-spacing parametric). Lowest WSJT-X priority (legacy EME).

### Phase W5 — JS8
- **Reference:** `js8call/{JS8Submode.*,jsc.cpp,jsc_list.cpp,jsc_map.cpp,varicode.cpp}`, `js8call/lib/js8/`, `js8call/lib/js8{a,b,c,e}_decode.f90`.
- **New blocks:** `framing/jsc.rs` (JS8's JSC compressed text codec — the bulk of the work) + the directed protocol layer (heartbeat/CQ, directed calls, relay/store-forward, ACK/SNR).
- **Modes:** JS8 Normal/Fast/Turbo/Slow (submodes A/B/C/E) — reuse the FT8 core (Costas + LDPC + 77-bit) with per-submode Costas/timing params from `JS8Submode.*`.

---

## Scope decisions (resolved by the issue owner 2026-07-02)

- **Utility modes excluded:** SSB/WWV/ANALYSIS/FMT/DTMF (fldigi) and Echo/FreqCal (WSJT-X) are **not** in this plan.
- **8PSK + OFDM data modes** → Phase 16 (long tail), reusing the Phase-11 OFDM core. ✅ confirmed.
- **RSID** → **both** detect and transmit, promoted to its own **Phase 13**. ✅ (owner wanted both.)
- **Picture sub-protocols** (MFSK/THOR/IFKP/FSQ images) → their own **Phase 15**; the text modes ship first. ✅ (owner: "add images as a later phase").
- **SSTV** → confirmed **not** present in fldigi or WSJT-X (grep of `fldigi/src` + `wsjtx/lib` finds no SSTV modem). Out of current parity scope; can be folded into Phase 15 only if an OSS SSTV reference repo (QSSTV/slowrx/pySSTV) is added to the workspace. **Open — awaiting owner.**
- **JS8** → in scope as Phase W5; `js8call` repo is checked out and cited. ✅.

## Sequencing & parallelism

P0 first (✅ landed). Then the two tracks run in parallel. First executable phase plans: **Phase 7 (PSK)** on the fldigi side and **W1 (FST4/FST4W)** on the WSJT-X side. Ordering constraints within the fldigi track: Phase 10 (Image + Hell) follows P0; Phase 11 (OFDM core) precedes Phase 16's OFDM data modes; Phase 9's IFK core precedes IFKP (Phase 14) and the IFKP/THOR picture work (Phase 15); Phase 13 (RSID) follows Phase 8 so there is a mode to identify; Phase 15 follows the text modes it adds pictures to. Everything else is independent.

## Self-review

- **Spec coverage:** every non-utility family in the roadmap §2/§2b maps to a phase (PSK→7, MFSK/Contestia→8, DominoEX/THOR→9, Hell→10, MT63→11, NAVTEX/SITOR/WEFAX→12, RSID→13, FSQ/IFKP→14, picture sub-protocols→15, Throb/8PSK/OFDM→16; FST4/FST4W→W1, MSK144→W2, Q65→W3, JT4→W4, JS8→W5). Cross-cutting Image payload → P0 (✅); RSID → Phase 13.
- **Placeholder scan:** P0 is implemented and verified. Mode phases are specified at port-task granularity with real reference files + the uniform T1–T7 template; the literal ported code per mode is produced at execution start from the cited reference (it cannot be authored without reading that reference in depth, and inventing it would violate the porting doctrine — Doctrine §6). This matches how Phases 4–5 were executed (one detailed plan per phase).
- **Type consistency:** `FramePayload::Image { width, gray }` is defined once in P0.1 and referenced by Phases 10, 12, 15. The `ModeConfig` enum + `registry.rs` arms extend the existing pattern from `mode/registry.rs`.

## Per-phase PR (delivery unit)

**Each phase ships as its own PR** (owner requirement). Workflow:

1. One branch per phase: `feature/omnimodem-phaseN-<name>` (e.g. `feature/omnimodem-phase7-psk-family`), branched from the integration base.
2. All phase tasks (T1–T8 for every mode) land on that branch as small commits.
3. Phase exit gate before the PR: full workspace `cargo build` + `cargo test` green, TUI `go test ./...` green, all mode KAT/cross-decode/BER gates met, and **every new mode selectable in the TUI**.
4. Open the PR with `gh pr create`, titled `Phase N — <modes>`, body summarizing modes ported, reference commits used, new building blocks, and the conformance evidence (KAT + BER results). Request review.
5. Merge only after review; the next phase branches from the merged base.

P0 is the first such branch (`feature/mode-parity-p0-harness`) and should be PR'd the same way.

> **Environment note:** pushing/PR-ing is currently **blocked in this sandbox** — the platform repocache rejects pushes ("unable to create temporary object directory") and direct GitHub is walled off by the gitconfig rewrite. Phase branches are prepared and committed locally, ready to PR; opening the PRs needs push access provisioned by the platform. The plan treats "open the PR" as the mandatory closing step of every phase regardless.

## Execution handoff

Per phase, generate the bite-sized executable plan (`docs/plans/YYYY-MM-DD-omnimodem-phaseN-<name>.md`) from the cited references, then implement via subagent-driven-development (fresh subagent per port task, two-stage review) or executing-plans. **Next:** generate the Phase 7 and W1 detailed plans (parallel), then execute — each closing with its own PR.

### Pre-existing TUI gap to true up
The TUI `modes.go` list currently offers only `psk31/rtty/cw/afsk1200/ft8` — the already-shipped `olivia/ft4/jt65/jt9/wspr` are **missing** from the operate screen. This is the exact daemon-vs-TUI drift T8 exists to prevent. Fold a one-time true-up (add those five existing modes to the TUI) into the first fldigi/WSJT-X phase PRs, or a small standalone PR before Phase 7.
