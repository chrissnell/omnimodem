# Phase 14 — FSQ + IFKP (text), fldigi parity

> Instantiates the per-mode port template (T1–T9) from
> `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md` for the
> **FSQ (FSQCALL)** and **IFKP** families. Reference:
> `fldigi/src/fsq/{fsq.cxx,fsq_varicode.cxx}`, `fldigi/src/ifkp/{ifkp.cxx,
> ifkp_varicode.cxx}`, `fldigi/src/include/{fsq.h,ifkp.h,crc8.h}` (checked out at
> `../fldigi`).

**Goal:** port fldigi's FSQ and IFKP **text** modes into omnimodem as bit-exact-
compatible, cross-decoding modes, verified against golden vectors extracted from
the unmodified fldigi tables. The **picture sub-protocols** (`ifkp-pic.cxx`,
`fsq-pic.cxx`) are Phase 15 and are out of scope here (the P0 `FramePayload::Image`
plumbing already exists for them). FSQ's asynchronous station services (sounder,
aging heard-list eviction, delayed/relayed store-and-forward transmit) are
runtime/scheduler concerns outside the `Demodulator`/`Modulator` trait model and
are carried as a documented follow-up (see "Deferred"); the **wire-determining**
directed-protocol layer (CRC8 header, callsign addressing, trigger parse, heard
list, reply-string synthesis) is ported and tested here.

## What these modes are

Both are **IFK (incremental frequency keying)** MFSK over **33 tones**, sharing
the same self-framing varicode family (a `[256][2]` symbol table + a
`prev*32+curr` decode table). Each character maps to one symbol (`sym1`) or two
(`sym1`, then `sym2` when `sym2 > 28`). The transmitter advances the tone by
`sym + OFFSET` each symbol; the receiver differences successive tones back to the
symbol and streams them through a two-symbol state machine
(`process_symbol`). This is the same IFK plumbing DominoEX/THOR (Phase 9) use,
retargeted to 33 tones with `OFFSET = 1`.

### IFKP (`ifkp.cxx`, `ifkp_varicode.cxx`, `ifkp.h`)

- `IFKP_SR = 16000`, `IFKP_SYMLEN = 4096`, `IFKP_SPACING = 3` bins, `IFKP_OFFSET
  = 1`, 33 tones. Tone spacing = `SPACING · SR / SYMLEN = 11.72 Hz`; bandwidth =
  `33 · spacing`.
- TX: `send_symbol(sym): tone = (prevtone + sym + 1) % 33` (`ifkp.cxx:706-710`);
  `send_char`: emit `sym1 = varicode[ch][0]`, then `sym2 = varicode[ch][1]` iff
  `sym2 > 28` (`ifkp.cxx:717-728`).
- Three speeds via `progdefaults.ifkp_baud` (`ifkp.cxx:694-696`): slow = 2× symlen,
  normal = 1×, fast = 0.5× — i.e. 1.95 / 3.91 / 7.81 baud. Tone spacing is
  unchanged; only symbol duration changes.
- RX: `process_symbol` (`ifkp.cxx:423-480`) — difference to a symbol via the
  `nibbles[]` table, then the `prev_nibble` machine: `prev<29 && curr<29` →
  `varidecode[prev]` (single-symbol char, decoded one symbol late); `prev<29 &&
  28<curr<32` → `varidecode[prev*32+curr]` (two-symbol char).

### FSQ / FSQCALL (`fsq.cxx`, `fsq_varicode.cxx`, `fsq.h`, `crc8.h`)

- `SR = 12000`, `FSQ_SYMLEN = 4096` (the fixed tone-spacing reference), `spacing =
  3` bins, `basetone = 333`, 33 tones. Tone spacing = `3 · 12000 / 4096 = 8.79
  Hz`. `send_symbol` is the same IFK as IFKP with `+1` (`fsq.cxx:1352-1356`).
- Five speeds (`fsq.cxx:312-327`) fix only the emitted symbol length: 1.5→8192,
  2→6144, 3→4096, 4.5→3072, 6→2048 samples/symbol at 12 kHz.
- `send_idle` = `send_symbol(28); send_symbol(30)` (`fsq.cxx:1359-1363`);
  `send_char` = same `sym1`/`sym2>28` rule (`fsq.cxx:1367-1382`).
- **Directed framing** (`tx_process`, `fsq.cxx:1482-1521`): a transmission is
  `" " + FSQBOL + mycall + ":" + crc8(mycall) + body + FSQEOT` where `FSQBOL = "
  \n"`, `FSQEOT = "  \b  "` (directed) or `FSQEOL = "\n "` (non-directed). `crc8`
  is CRC-8/CCITT (poly `0x07`, init `0x00`), 2-char lowercase hex (`crc8.h`).
- **Directed parse** (`parse_rx_text`, `fsq.cxx:436-623`): locate the `<call>:<crc>`
  header (verify CRC over the callsign), register the caller in the heard list,
  then walk addressed callsigns/triggers. `valid_callsign` classifies mycall /
  `allcall` / `cqcqcq` / other. When addressed (directed or allcall), the leading
  trigger character selects a responder (`' '` plain text, `?` snr, `*` info, `#`
  send-message, `%` acknowledge, …). Trigger set `fsq.cxx:405`.

## Method (doctrine)

- **Golden vectors from the unmodified reference.** Two standalone extractors link
  the untouched varicode `.cxx` and transcribe the short send/decode/CRC glue with
  `// ref:` cites (that glue lives in the modem class, which cannot link without
  the FLTK/modem runtime):
  - `scratch/refvectors/ifkp_varicode_dump.cxx` (+ `build_ifkp_varicode.sh`) →
    `crates/dsp/tests/vectors/ifkp_varicode.json`: the raw `[sym1,sym2]` table,
    the `varidecode` table, and per-message `syms` / IFK `tones` / streaming
    `decode`.
  - `scratch/refvectors/fsq_varicode_dump.cxx` (+ `build_fsq_varicode.sh`) →
    `crates/dsp/tests/vectors/fsq_varicode.json`: the `[sym1,sym2]` table, the
    `wsq_varidecode` table, CRC8 callsign checksums, and full `text`/`directed`
    frames (`onair` string, `syms`, `tones`, `decode`).
- **Two equivalence classes.** Bit-exact on the integer domain (varicode symbols,
  `varidecode` output, IFK tone indices, CRC8) — asserted byte-for-byte. FP
  tolerance on audio — gated on a loopback + AWGN decode only, never bit-exact
  (Doctrine §3).
- **Verbatim tables.** Both `varicode[256][2]` encode tables and both
  `varidecode[29*32]` decode tables are transcribed verbatim with `// ref:` cites
  and asserted against the golden dump. IFKP and FSQ tables differ in a handful of
  entries (space, `<LF>`, a few punctuation rows) — they are **not** shared.
- **CI mirror.** `tests/kat.rs` is gated behind `testutil` (empty in CI), so the
  bit-exact reference KATs are **also** plain `#[cfg(test)]` lib unit tests in the
  framing/mode modules (Doctrine §5).

## Building blocks

- `crates/dsp/src/framing/ifkp_varicode.rs` — `IFKP_VARICODE[256][2]`,
  `IFKP_VARIDECODE`, `encode_char`, streaming `Framer` (ports `process_symbol`).
- `crates/dsp/src/framing/fsq_varicode.rs` — `FSQ_VARICODE`, `WSQ_VARIDECODE`,
  `encode_char`, `Framer`, and `crc8` (CRC-8/CCITT).
- `crates/dsp/src/modes/ifk33.rs` — the shared 33-tone IFK modulator/demodulator
  helpers (`tone advance`, symbol→audio via `frontend::modulate::MFsk`, Goertzel
  symbol detect + differential inverse), parameterised by symlen/spacing/rate so
  IFKP and FSQ both build on it (mirrors how DominoEX/THOR share the IFK core).
- `crates/dsp/src/modes/ifkp.rs` — IFKP mode: submode speeds, `Modulator`,
  `Demodulator`.
- `crates/dsp/src/modes/fsq.rs` — FSQ mode: speeds, BOT/EOT framing, `Modulator`,
  `Demodulator`, plus `fsqcall` directed-protocol submodule (CRC header build /
  verify, `parse_directed`, heard list, reply synthesis).

## Tasks (T1–T9)

- [x] **T1** — extractors + golden vectors committed (`ifkp_varicode.json`,
  `fsq_varicode.json`).
- [x] **T2** — ported `ifkp_varicode` + `fsq_varicode` (verbatim tables + framer
  + CRC8); bit-exact unit tests vs the golden tables.
- [x] **T3** — n/a: neither mode has FEC or interleave (text path). The
  "codeword" stage is the varicode symbol stream, covered by T2.
- [x] **T4** — 33-tone IFK modulator (`modes/ifk33.rs` + `ifkp`/`fsq`):
  symbol→tone index bit-exact vs golden `tones`; audio via `MFsk`, loopback
  tolerance only.
- [x] **T5** — 33-tone IFK demodulator: Goertzel symbol detect + differential
  inverse + framer; loopback round-trip + AWGN sweep. FSQ adds the BOT/EOT
  segmentation + directed parse.
- [x] **T6** — registered `ifkp` (+ speed) and `fsq` (+ speed, directed, mycall)
  in `modes/mod.rs`, `mode/{mod.rs,registry.rs}`, proto params.
- [x] **T7** — conformance: golden-vector KAT + loopback/AWGN grid + `#[ignore]`
  cross-decode gate in `kat.rs`; `ber.rs` AWGN decode-rate sweeps with committed
  thresholds.
- [x] **T8** — TUI: `ifkp` and `fsq` rows in `internal/app/modes.go` with their
  params; `IfkpParams`/`FsqParams` proto (Rust + Go), `effective_mode` encoding,
  and the FSQ callsign injected from the station identity.
- [ ] **T9** — phase PR.

## Deferred (documented, not stubbed)

- **Picture sub-protocols** (`ifkp-pic.cxx`, `fsq-pic.cxx`) → Phase 15.
- **FSQ station services** — sounder beacon, heard-list aging/eviction timers,
  delayed/relayed store-and-forward transmit (`fsq.cxx` `sounder`/`aging`/
  `try_transmit` threads). These are scheduler/timer behaviours with no home in
  the stateless demod/mod trait; the demod **surfaces** parsed directed messages
  (caller, addressee-class, trigger, payload) and the reply-string synthesiser is
  a pure, tested function, but auto-transmit scheduling is a daemon concern for a
  later slice. No reachable code path is stubbed — unsupported async triggers
  simply do not synthesise a reply.
- **IFKP heard-list / `de` callsign scraping** is decoding-time UI in fldigi; the
  decoded text stream carries the same information and the daemon can scrape it,
  so it is not reproduced in the DSP layer.
