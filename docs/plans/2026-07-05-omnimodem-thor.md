# Phase 9 (cont.) — THOR (fldigi) port

**Goal:** port fldigi's THOR family into omnimodem, completing Phase 9 (DominoEX
landed in #65). THOR is DominoEX's 18-tone IFK+ core with a rate-1/2
convolutional FEC, a size-4 diagonal interleaver, soft-decision Viterbi decode
and the THOR varicode layered on top.

**Reference:** `fldigi/src/thor/{thor.cxx,thorvaricode.cxx}`,
`fldigi/src/filters/viterbi.cxx` (encoder/Viterbi), `fldigi/src/mfsk/{interleave.cxx,mfskvaricode.cxx}`
— upstream 4.1.23 @ 61b97f413.

**Submodes (parametric):** Micro, 4, 5, 8, 11, 16, 22, 25x4, 50x1, 50x2, 100.
Picture sub-protocol (`thor-pic.cxx`) is out of scope (Phase 15).

## The wire (thor.cxx sendchar/sendsymbol, reverse=false)

Per character: `thorvarienc(c, secondary)` → varicode `'0'/'1'` bits. Primary set
is the **IZ8BLY MFSK varicode** (`varienc`, already ported as
`framing::varicode::MFSK`); the secondary set (0x100+) is the THOR 12-bit
varicode (`thorvaricode.cxx`). Each varicode bit is fed to a **streaming**
rate-1/2 convolutional encoder (state carried across the whole message, no
per-char flush): K=7 `0x6d`/`0x4f` for most modes, K=15 `044735`/`063057` for
the high-speed 25x4/50x1/50x2/100. The 2 output bits (poly1 then poly2) are
shifted MSB-first into a 4-bit nibble; each full nibble goes through a **size-4
diagonal interleaver** (`interleave(4, idepth, FWD)`, idepth per submode) and is
sent as an **IFK+** tone: `tone = (prevtone + 2 + nibble) % 18`, prevtone starts 0.

RX inverts: IFK+ tone difference → nibble → 4 soft bits → reverse interleave →
soft Viterbi → data bits → varicode reframe (`datashreg`, decode on `&7==1`).

## Per-submode params (thor.cxx:217-297)

| submode | symlen | doublespaced | rate | K  | idepth |
|---------|--------|--------------|------|----|--------|
| Micro   | 4000   | 1            | 8000 | 7  | 4      |
| 4       | 2048   | 2            | 8000 | 7  | 10     |
| 5       | 2048   | 2            | 11025| 7  | 10     |
| 8       | 1024   | 2            | 8000 | 7  | 10     |
| 11      | 1024   | 1            | 11025| 7  | 10     |
| 16      | 512    | 1            | 8000 | 7  | 10     |
| 22      | 512    | 1            | 11025| 7  | 10     |
| 25x4    | 320    | 4            | 8000 | 15 | 50     |
| 50x1    | 160    | 1            | 8000 | 15 | 50     |
| 50x2    | 160    | 2            | 8000 | 15 | 50     |
| 100     | 80     | 1            | 8000 | 15 | 50     |

`tonespacing = samplerate * doublespaced / symlen`; `bandwidth = 18 * tonespacing`.

## Tasks

- **T1 — Golden vectors.** `scratch/refvectors/thor_dump.cxx` + `build_thor.sh`
  wire the unmodified fldigi encoder/interleave/varicode leaf files and dump, for
  `CQ DE K1ABC`, every stage (varicode bits, code pairs, nibbles, post-interleave
  nibbles, IFK+ tones) for THOR16 (K=7) and THOR100 (K=15), plus the secondary
  varicode table. → `crates/dsp/tests/vectors/thor_varicode.json`. ✅
- **T2 — THOR varicode.** `framing/thor_varicode.rs`: primary via
  `framing::varicode::MFSK`; secondary 12-bit table transcribed verbatim from
  `thorvaricode.cxx`. Bit-exact KAT vs the `secondary[]` vector.
- **T3 — FEC + interleave.** Add a streaming `ConvEncoder` + THOR K=7/K=15
  `ConvCode` constructors to `fec::conv`; add a general square
  `MfskInterleaver` (size-4) to `fec::interleave` porting `interleave.cxx`.
  Bit-exact KATs: encoder code pairs, post-interleave nibbles vs the vector.
- **T4 — Modulator.** `modes/thor.rs` `text_symbols` reproduces the IFK+ tone
  sequence bit-exact vs the vector; `ThorMod` renders IFK+ audio via
  `frontend::modulate::MFsk` (FP, loopback-gated only).
- **T5 — Demodulator.** IFK+ inverse → reverse interleave → streaming Viterbi →
  varicode reframe. Loopback recovers text (framed with idle preamble/flush to
  drain the interleaver + Viterbi latency).
- **T6 — Register in daemon.** `modes/mod.rs`, `ModeConfig::Thor`, `registry.rs`,
  `ThorParams` proto message, `service.rs` params arm. Registry + parse tests.
- **T7 — Reference KAT.** `tests/kat.rs` bit-exact assertion vs the vector
  (plain lib unit tests also present per doctrine §6, since CI omits `testutil`).
- **T8 — TUI.** `modeInfo` rows for the THOR family (`chat` shape, submode +
  center params), `modeParamsFor` arm, proto regen, Go tests.
- **T9 — PR.** Workspace `cargo test` + TUI `go test ./...` green; open the phase PR.

## Equivalence classes (doctrine §3)

Bit-exact (asserted byte-for-byte vs the vector): secondary varicode, conv code
pairs, interleaver output, IFK+ tone indices. FP-tolerance / loopback-only:
modulated IFK+ audio (never asserted bit-exact). Soft-decode CWI/doppler
refinements and RSID/AFC/preamble-detect are RX-quality features deferred like
the DominoEX sync path — the loopback is the gate, fldigi cross-decode the
`#[ignore]` nightly gate.
