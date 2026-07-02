# JT65 / JT9 WSJT-X On-Air Interop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the self-consistent placeholder JT65 and JT9 assemblies with a faithful port of WSJT-X's encode/decode pipeline so omnimodem is bit-exact on the air and cross-decodes with the WSJT-X reference binaries in both directions.

**Architecture:** Follows the house **Porting Doctrine** in `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md` (port from the reference, gate each stage on golden vectors, close on a bidirectional cross-decode). JT65 and JT9 share one message source-encoder (`framing::message77::legacy`, a port of `wsjtx/lib/packjt.f90`); they diverge only in FEC + modulation. This plan brings the *already-shipped-but-non-interoperable* JT65/JT9 up to that doctrine. WSPR is a sibling (different FEC + a beacon TX path that does not yet exist) and is out of scope here — tracked separately.

**Tech Stack:** Rust (workspace, edition 2021). Reuses `omnimodem-dsp` blocks (`fec`, `framing`, `frontend`, `modes::fsk_util`) and the conformance harness in `crates/dsp/tests/` (`kat.rs`, `ber.rs`, `loopback.rs`, `roundtrip.rs`, `vectors/`). Reference source is checked out in-tree at `wsjtx/lib/`.

---

## Why this plan exists (root cause)

A user calling CQ in JT65/JT9 hears nothing. Traced to the encoder: `framing::message77::legacy::pack72` only accepts a standard `CALL CALL GRID` exchange, so `"CQ MYCALL GRID"` fails to pack, `modulate()` returns `Err`, and `core/tx_worker.rs` silently emits start/complete **without keying PTT**. Beyond CQ, the placeholder is not WSJT-X-compatible in several confirmed ways:

| Stage | omnimodem today | WSJT-X reference | Ref |
|---|---|---|---|
| Message pack | `pack72`: `CALL CALL GRID` only; no CQ, reports, RRR/RR73/73, free-text, prefix/suffix, shorthands | `packmsg` + `packcall`/`packgrid`/`packtext`/`packpfx`/`pack50` | `wsjtx/lib/packjt.f90` |
| RS(63,12) GF(64) | `RsGf64::jt65()` uses **fcr = 1** | `init_rs_int(6, 0x43, 3, 1, 51, 0)` → **fcr = 3** | `wsjtx/lib/wrapkarn.c:20` |
| Interleave | simple `[sync,data]` alternation | `interleave63` = 7×9 transpose | `wsjtx/lib/interleave63.f90` |
| Gray code | **none** | `graycode65` (`igray`) applied to 63 symbols | `wsjtx/lib/graycode65.f90`, `igray.c` |
| Sync placement | alternating sync every other symbol | fixed 126-entry pseudorandom `nprc` vector; data tone = `gray(rs)+2`, sync = tone 0 | `wsjtx/lib/gen65.f90` |

The RS `fcr` mismatch, the missing Gray code, and the wrong sync vector each independently break cross-decode; all must be corrected together. The module headers already flag this: "exact WSJT-X sync-vector interleave is the `#[ignore]` cross-decode gate."

## Acceptance gates (Porting Doctrine)

Two oracles, both from the reference already in-tree:

1. **Golden symbol vectors** — `wsjtx/lib/jt65code.f90` and `jt9code.f90` print, for a message, the packed 12×6-bit `dgen`, the 63-symbol RS codeword `sent`, and the final tone array (`itone(126)` for JT65, `i4tone` for JT9). `jt65code -t` enumerates the canonical WSJT-X test-message set (incl. `CQ`, reports, `RRR`, `RR73`, `73`, prefix/suffix calls, free text). These are the KAT fixtures.
2. **Bidirectional cross-decode** — omnimodem-generated audio decodes in the WSJT-X `jt65`/`jt9` binary, and reference-generated audio (`jt65sim`/`jt9sim`) decodes in omnimodem. `#[ignore]`-gated in `kat.rs` behind the reference-binary env var, per `Task P0.3` of the master plan.

A stage stays **open** until its golden-vector gate passes. No stubs merged behind the gate.

## Building the reference oracle (do first, once)

The Fortran tools must be compiled to emit vectors. They live in `wsjtx/lib/` and build with `gfortran` + the small C shims (`init_rs.c`, `encode_rs.c`, `igray.c`, `wrapkarn.c`).

- [ ] **Step 1:** From a scratch dir, compile the JT65 packer + encoder into a vector-dumping harness. Minimum objects: `packjt.f90`, `wrapkarn.c`, `init_rs.c`, `encode_rs.c`, `igray.c`, `graycode65.f90`, `interleave63.f90`, `gen65.f90`, `jt65code.f90` (+ `fmtmsg.f90`, `chkmsg.f90`, `grid2deg.f90` and other `jt65code` deps surfaced by the linker). Put the recipe in `scratch/jt65code/Makefile`.

Run: `gfortran -o scratch/jt65code/jt65code <objs> && scratch/jt65code/jt65code -t`
Expected: a table of test messages with `Decoded`, `Type`, `Expected` columns, all `Err?` blank.

- [ ] **Step 2:** Add a `--dump` path (or a tiny wrapper `jt65vec.f90`) that additionally prints `dgen(12)`, `sent(63)`, and `itone(126)` as CSV for each `-t` message. Capture stdout into `crates/dsp/tests/vectors/jt65/*.csv` (one file per message, filename = slugified message). Commit the fixtures.

Run: `scratch/jt65code/jt65code --dump-t > /tmp/jt65vec.txt` then split into fixtures.
Expected: N CSV fixtures, each with `dgen`, `sent`, `itone` rows.

- [ ] **Step 3:** Repeat Steps 1–2 for JT9 using `jt9code.f90` + `gen9`/`genjt9`, `jt9fano.f90`. Fixtures in `crates/dsp/tests/vectors/jt9/*.csv` (packed bits, symbol tones `i4tone`).

- [ ] **Step 4:** Commit the fixtures and the `scratch/jt65code`, `scratch/jt9code` build recipes.

```bash
git add crates/dsp/tests/vectors/jt65 crates/dsp/tests/vectors/jt9 scratch/jt65code scratch/jt9code
git commit -m "test(jt65/jt9): golden symbol vectors from WSJT-X reference tools"
```

> If a Fortran toolchain is unavailable in CI, the fixtures are still committed as static files; the `scratch/*/Makefile` documents exactly how they were produced so they can be regenerated. The `#[ignore]` cross-decode gate (which needs the live binary) is separate from the KAT gate (which needs only the committed fixtures).

---

## Task 1: Port `packmsg`/`unpackmsg` (message source encoder)

The single biggest piece and the immediate CQ blocker. Shared by JT65 and JT9. Port `wsjtx/lib/packjt.f90` verbatim in behavior, idiomatic Rust.

**Files:**
- Modify: `crates/dsp/src/framing/message77.rs` (the `legacy` module: replace `pack72`/`unpack72`, add `packcall`/`unpackcall`, `packgrid`/`unpackgrid` special values, `packtext`/`unpacktext`, `packpfx`/`getpfx1`/`getpfx2`, `pack50` already present — verify)
- Test: `crates/dsp/tests/vectors/jt65/*.csv` (dgen rows) + inline unit tests

Reference map (`wsjtx/lib/packjt.f90`): `packcall:54`, `unpackcall:138`, `packgrid:279`, `unpackgrid:356`, `packmsg:401`, `unpackmsg:537`, `packtext:663`, `getpfx1:747`, `getpfx2:847`, `pack50:947`, `packpfx:973`. Transcribe the constant tables (`nchar`, alphabet orderings, grid special-value ranges, the `NGBASE`/report encodings) **verbatim** with a `// ref: packjt.f90:<lines>` cite. Re-express the control flow the Rust way (no COMMON blocks, no 1-based array translit).

- [ ] **Step 1: Write the failing test** — round-trip + golden `dgen` for the canonical set.

```rust
// in message77.rs legacy_tests
#[test]
fn jt65_packmsg_matches_wsjtx_golden() {
    for (msg, want_dgen) in load_jt65_dgen_fixtures() { // reads tests/vectors/jt65/*.csv
        let dgen = legacy::packmsg(msg).expect(msg);       // [u8; 12], 6-bit symbols
        assert_eq!(dgen, want_dgen, "dgen mismatch for {msg:?}");
        assert_eq!(legacy::unpackmsg(&dgen), msg_as_received(msg));
    }
}
#[test]
fn jt65_cq_now_packs() {
    assert!(legacy::packmsg("CQ K1ABC FN42").is_some());
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p omnimodem-dsp jt65_packmsg_matches_wsjtx_golden` → FAIL (`packmsg` undefined / `CQ` returns None).

- [ ] **Step 3: Implement `packmsg`/`unpackmsg`** porting `packjt.f90` (all message types: standard call/call/grid, call/call/report, RRR/RR73/73, CQ/QRZ/DE prefixes, `CQ nnn`, type-6 free text via `packtext`, compound-call prefix/suffix via `packpfx`). Keep `pack50` (WSPR) intact.

- [ ] **Step 4: Run to verify it passes** — both tests PASS; CQ, reports, RR73, 73, free text all round-trip and match golden `dgen`.

- [ ] **Step 5: Commit** — `git commit -m "feat(jt65/jt9): WSJT-X packmsg/unpackmsg port (full message set incl. CQ)"`

## Task 2: Fix RS(63,12) to WSJT-X parameters (fcr = 3)

**Files:**
- Modify: `crates/dsp/src/fec/rs_gf64.rs:68-69` (`RsGf64::jt65()`)
- Test: `crates/dsp/src/fec/rs_gf64.rs` (inline) + `sent` rows from JT65 fixtures

- [ ] **Step 1: Write the failing test** — encode golden `dgen` → must equal golden `sent(63)`.

```rust
#[test]
fn jt65_rs_encode_matches_wsjtx_sent() {
    for (dgen, want_sent) in load_jt65_sent_fixtures() {
        assert_eq!(RsGf64::jt65().encode(&dgen), want_sent);
    }
}
```

- [ ] **Step 2: Run to verify it fails** — FAIL (current `fcr = 1` produces different parity than reference `fcr = 3`).

- [ ] **Step 3: Fix the parameter** — change `RsGf64::jt65()` to `Self::new(51, 3)` and update the doc comment; confirm the primitive poly stays `0x43` and `build_genpoly` uses `(fcr + i) % PERIOD` (already correct). Verify existing loopback decode still passes with the new genpoly.

- [ ] **Step 4: Run to verify it passes** — the golden `sent` test PASSES and existing `rs_gf64` decode round-trip tests stay green.

- [ ] **Step 5: Commit** — `git commit -m "fix(fec): JT65 RS uses WSJT-X fcr=3 (was fcr=1), breaking on-air compat"`

## Task 3: Port the JT65 symbol pipeline (interleave + Gray + `nprc` sync)

**Files:**
- Modify: `crates/dsp/src/modes/jt65.rs` (`modulate` and `decode_window`)
- New helpers (private in `jt65.rs`): `interleave63`, `graycode65`, the `NPRC: [u8; 126]` table
- Test: `itone` rows from JT65 fixtures

Reference: `gen65.f90` (the `nprc` data table + `itone(j)=sent(k)+2` / sync `=0` placement), `interleave63.f90` (7×9 transpose), `graycode65.f90` + `igray.c` (`igray(v, idir)`).

- [ ] **Step 1: Write the failing test** — full message → `itone(126)` equals golden.

```rust
#[test]
fn jt65_itone_matches_wsjtx_golden() {
    for (msg, want_itone) in load_jt65_itone_fixtures() {
        assert_eq!(Jt65Mod::new().symbol_tones(msg).unwrap(), want_itone); // 126 tones
    }
}
```

- [ ] **Step 2: Run to verify it fails** — FAIL (alternating sync, no Gray code).

- [ ] **Step 3: Implement** — replace the `[sync, s+1]` loop with: `sent = rs.encode(dgen)`; `interleave63(&mut sent)`; `graycode65(&mut sent)`; then place into `itone[126]` per `NPRC` (`sent(k)+2` where `nprc==0`, else `0`). Transcribe `NPRC` verbatim from `gen65.f90` with a `// ref:` cite. Mirror the inverse in `decode_window` (de-sync, un-Gray via `igray(-1)`, de-interleave, RS decode).

- [ ] **Step 4: Run to verify it passes** — `itone` golden test PASSES; the existing `loopback_decodes_message` still PASSES through the new pipeline.

- [ ] **Step 5: Commit** — `git commit -m "feat(jt65): WSJT-X symbol pipeline — interleave63 + Gray + nprc sync vector"`

## Task 4: JT65 tone→audio frequency mapping + cross-decode gate

**Files:**
- Modify: `crates/dsp/src/modes/jt65.rs` (`JT65_BASE_HZ`, tone→freq in `MFsk`), verify sub-mode A spacing = `11025/4096` Hz
- Test: `crates/dsp/tests/kat.rs` (`#[ignore]` bidirectional cross-decode)

- [ ] **Step 1:** Confirm tone spacing is exactly `JT65_RATE/JT65_SPS` (2.6917 Hz, sub-mode A) and that `itone` index 0 = sync maps to the operator's chosen sync audio frequency, data tones at `+2..+65` above it — matching `jt65_mod.f90`'s tone-to-frequency convention. Adjust `JT65_BASE_HZ`/offset only if the reference wave disagrees.

- [ ] **Step 2:** Add the `#[ignore]` cross-decode test in `kat.rs`: write omnimodem JT65 audio for `"CQ K1ABC FN42"` to a wav, decode with the reference `jt65` binary (env `WSJTX_BIN`), assert the message; and decode a `jt65sim`-generated wav in omnimodem's `Jt65Demod`.

- [ ] **Step 3:** Run the ignored gate locally with the reference binary present.

Run: `WSJTX_BIN=... cargo test -p omnimodem-dsp --test kat jt65_crossdecode -- --ignored`
Expected: PASS both directions.

- [ ] **Step 4: Commit** — `git commit -m "test(jt65): bidirectional WSJT-X cross-decode gate + audio tone map"`

## Task 5: JT9 — reuse packmsg, port FEC + modulation

JT9 shares Task 1's `packmsg`. It diverges: rate-1/2 K=32 convolutional (Fano) FEC and 9-FSK with a 1-symbol sync, per `wsjtx/lib/gen9`/`jt9fano.f90`/`jt9code.f90`. `fec::fano` (K=32) already exists in-tree.

**Files:**
- Modify: `crates/dsp/src/modes/jt9.rs` (`modulate`/`decode_window`), reuse `fec::fano`
- Test: `crates/dsp/tests/vectors/jt9/*.csv` + `#[ignore]` cross-decode in `kat.rs`

- [ ] **Step 1: Write the failing test** — message → `i4tone` equals JT9 golden.
- [ ] **Step 2: Run to verify it fails.**
- [ ] **Step 3: Implement** — `packmsg` → 72 bits → append the JT9 CRC/tail per `gen9` → Fano K=32 encode → interleave → 9-FSK tone map with the JT9 sync symbol. Transcribe JT9 sync/interleave tables verbatim (`// ref: gen9`/`jt9sync.f90`).
- [ ] **Step 4: Run to verify it passes** — JT9 `i4tone` golden PASSES; loopback PASSES.
- [ ] **Step 5: Commit** — `git commit -m "feat(jt9): WSJT-X gen9 port — Fano FEC + 9-FSK sync"`
- [ ] **Step 6:** Add JT9 `#[ignore]` bidirectional cross-decode gate; commit.

## Task 6: True-up the sequencer + close the loop

**Files:**
- Modify: `clients/omnimodem-tui/internal/app/ft8.go` (ladder message forms already match WSJT-X grammar — verify CQ/report/RR73/73 strings pack post-Task 1)
- Modify: `crates/omnimodemd/src/core/tx_worker.rs:133-147` — on a genuine `modulate` `Err`, surface a telemetry **error** event (not a silent start/complete) so a mis-encoded message is visible instead of a phantom no-op transmit

- [ ] **Step 1:** Add a daemon test: enqueuing a JT65 `"CQ …"` frame now keys PTT (MockPtt records a key-up) rather than completing silently.
- [ ] **Step 2:** Run to verify it fails against the current silent path; implement the error-event surfacing; verify it passes.
- [ ] **Step 3:** Manual/integration: JT65 CQ transmits on the minute boundary and decodes in WSJT-X. Update `docs/wiki` mode page noting JT65/JT9 are now WSJT-X-interoperable and how to regenerate golden vectors.
- [ ] **Step 4: Commit.**

---

## Self-Review

- **Spec coverage:** CQ blocker → Task 1. WSJT-X bit-compat → Tasks 1–5 (pack, RS fcr, interleave/Gray/sync, tone map, JT9). Cross-decode requirement → Tasks 4 & 5 gates. Silent-no-key symptom → Task 6. WSPR → explicitly out of scope (sibling, separate FEC + missing beacon TX path).
- **Ordering:** Task 1 (shared pack) before 2–5. Task 2 before 3 (RS output feeds the symbol pipeline). Reference-oracle section before all (fixtures gate every task).
- **Doctrine alignment:** matches `2026-07-02-omnimodem-full-mode-parity-implementation.md` — port not reimplement, golden vectors + bidirectional cross-decode, no stub merged behind the gate.
- **Known-risk:** building the Fortran oracle in this environment (needs `gfortran`); mitigated by committing the produced fixtures + build recipe, and keeping the live-binary cross-decode as a separate `#[ignore]` gate.
