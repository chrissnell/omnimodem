# Phase 16 — Long tail: Throb, 8PSK, OFDM data modes

> **Master plan:** `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md`
> (read the Porting Doctrine + the T1–T9 task template first).
> **Roadmap line:** `docs/plans/2026-07-02-omnimodem-fldigi-mode-parity.md` → Phase 16.
> Lowest-priority fldigi long-tail. Executed via subagent-driven-development, one
> family at a time; each family closes on green KATs + loopback and is table-tested
> across its full submode grid.

**Goal:** bring the last three fldigi mode families to omnimodem parity —

1. **Throb** — Throb1/2/4 + ThrobX1/2/4 (dual-tone MFSK).
2. **8PSK** — 8PSK125/250/500/1000 (uncoded) + 8PSK125F/250F/125FL/250FL/500F/1000F/1200F (FEC).
3. **OFDM data** — OFDM-500F/750F/2000F/2000/3500 (multi-carrier xPSK/8PSK).

**References (checked out at `../fldigi`, upstream 4.1.23 @ `61b97f413`):**
- Throb: `fldigi/src/throb/throb.cxx`, `fldigi/src/include/throb.h`.
- 8PSK + OFDM data: `fldigi/src/psk/psk.cxx` (+ `psk.h`, `pskvaricode.cxx`,
  `mfskvaricode.cxx`, `viterbi.cxx`, `interleave.cxx`).

---

## Reference correction (the reference wins — Doctrine §1)

The master plan says the OFDM data modes "reuse the Phase-11 MT63 OFDM core." **The
fldigi reference does not.** In fldigi the `OFDM-500F/750F/2000F/2000/3500` modes are
implemented **inside `psk.cxx`** as ordinary **multi-carrier xPSK/8PSK** (3/4/7/8
parallel PSK carriers at 16 kHz), not as MT63's 64-carrier overlapping-Walsh OFDM.
"OFDM" here is a marketing label for parallel-carrier PSK. Per Doctrine §1 (interop
is the goal, the reference wins), Phase 16 ports them as **multi-carrier PSK/8PSK
extending `crates/dsp/src/modes/psk.rs`** — the same front end 8PSK uses — **not**
`frontend::ofdm` / `modes::mt63`. The MT63 core stays MT63-only.

Evidence (`psk.cxx:456-528`): each OFDM case sets `_xpsk`/`_8psk`, `numcarriers`
∈ {3,4,7,8}, `separation=2.0`, `samplerate=16000`, and a per-mode `symbollen`.

---

## Family 1 — Throb (✅ implemented in this phase's first PR)

Dual-tone MFSK at 8 kHz. Each printable character is a **pair of tones** sounded
together under a pulse envelope; there is no FEC, varicode, or interleave. The whole
mode is table-driven, which makes the bit-domain golden vector trivially exact.

**Reference:** `throb.cxx` (tables `ThrobTonePairs[45][2]`, `ThrobXTonePairs[55][2]`,
`ThrobCharSet`, `ThrobXCharSet`, freq tables `throb.cxx:729-944`; TX framing
`throb.cxx:582-721`; RX `throb.cxx:297-536`). Constants `throb.h:34-46`.

**Parameters** (`throb.cxx:141-220`):

| submode  | symlen (samp @8 kHz) | baud   | tones | chars | freq table          | pulse       |
|----------|----------------------|--------|-------|-------|---------------------|-------------|
| Throb1   | 8192                 | 0.977  | 9     | 45    | ThrobToneFreqsNar   | semi (1/5)  |
| Throb2   | 4096                 | 1.953  | 9     | 45    | ThrobToneFreqsNar   | semi (1/5)  |
| Throb4   | 2048                 | 3.906  | 9     | 45    | ThrobToneFreqsWid   | full (Hann) |
| ThrobX1  | 8192                 | 0.977  | 11    | 55    | ThrobXToneFreqsNar  | semi (1/5)  |
| ThrobX2  | 4096                 | 1.953  | 11    | 55    | ThrobXToneFreqsNar  | semi (1/5)  |
| ThrobX4  | 2048                 | 3.906  | 11    | 55    | ThrobXToneFreqsWid  | full (Hann) |

**Framing.** A character maps to a symbol index (linear search over the char set), the
symbol index maps to a tone pair. Regular Throb has a `SHIFT` symbol (index 5, pair
`{4,6}`) that prefixes `? @ = \n`; `\r` is dropped; unknown chars fold to space
(index 44). ThrobX has no shift — instead its `idle`/`space` symbols swap indices 0↔1
each time a space or idle throb is sent (`flip_syms`, `throb.cxx:96-114`), so a stream
of idles alternates two symbols. TX opens with a 4-symbol idle preamble
(`throb.cxx:47-52,625-630`).

**fldigi quirk (documented, replicated):** in regular Throb, TX for `-` sends `SHIFT`
+ symbol 9, which the receiver decodes as `=`, not `-` (`throb.cxx:666-669` vs
`357-393`). Likewise `?`/`@`/`\n` round-trip; `-` becomes `=`. The port replicates TX
byte-for-byte (KAT) and the loopback test uses a message inside the clean
round-tripping set.

**Gates.**
- **T1 golden vector** — `scratch/refvectors/throb_dump.cxx` + `build_throb.sh` emit
  `crates/dsp/tests/vectors/throb.json`: both tone-pair tables (0-based), both char
  sets, and for a fixed message per submode the exact emitted `(tone1,tone2)` sequence
  (pre-reverse) fldigi's `tx_process`/`send` produce. Provenance header = commit +
  command. The tables are transcribed **verbatim** from `throb.cxx` (cited) — they are
  the reference *data*; `throb.cxx`'s `tx_process` is entangled with the FLTK modem
  base and cannot be linked standalone, so the driver re-expresses only its
  char→symbol *selection* logic, each branch cited to `throb.cxx`.
- **T2/T3** — n/a (no source codec / FEC).
- **T4 modulator** — `throb.rs` `ThrobMod`: char→symbol→tone-pair **bit-exact** vs the
  vector; audio = `pulse[i]·(sin w1·i + sin w2·i)/2` within FP tolerance (never
  asserted bit-exact — Doctrine §3).
- **T5 demodulator** — `ThrobDemod`: per-symbol Goertzel over the tone set → top-two
  tones (single-tone special case for regular Throb, `throb.cxx:324-329`) → tone-pair
  reverse lookup → char. Loopback recovers text across the whole grid.
- **T6 registry** — `ModeConfig::Throb`, `parse`/`to_mode_string`/`label`, `registry.rs`
  demod+mod arms, `ThrobParams` proto message + `service.rs` arm.
- **T7 conformance** — bit-exact KAT in `kat.rs` (also mirrored as plain lib unit
  tests, since CI runs without `testutil`); loopback across all six submodes; the
  `#[ignore]` fldigi cross-decode note. No AWGN BER curve is committed for Throb — at
  <4 baud a Watterson sweep is not meaningful; the loopback + bit-exact TX gate stands.
- **T8 TUI** — six `chat`-shape rows + `modeParamsFor` arm (`submode`+`center`), Go
  proto regen, Go test.

**RSID:** Throb has no fldigi RSID assignment → no `rsid_key`.

---

## Family 2 — 8PSK (planned)

8-ary differential PSK at 16 kHz, Gray-mapped, extending `modes::psk.rs`. Not yet
implemented; this section is the execution spec for the next Phase-16 PR.

**Reference:** `psk.cxx` — constellation `graymapped_8psk_pos[]` + soft table
`graymapped_8psk_softbits[8][3]` (`psk.cxx:100-124`); mode table `psk.cxx:532-655`;
`symbits=3` (`psk.cxx:905`); TX 8PSK `psk.cxx:2235-2410`; RX soft decode
`psk.cxx:1708-1815`; FEC setup `psk.cxx:990-1030`.

**Submode grid** (`psk.cxx:532-655`, all `samplerate=16000`, `_8psk`):

| submode    | symbollen | baud | FEC                    | dcdbits |
|------------|-----------|------|------------------------|---------|
| 8PSK125    | 128       | 125  | none (`_disablefec`)   | 128     |
| 8PSK250    | 64        | 250  | none                   | 256     |
| 8PSK500    | 32        | 500  | none                   | 512     |
| 8PSK1000   | 16        | 1000 | none                   | 1024    |
| 8PSK125FL  | 128       | 125  | K=13 rate-1/2          | 128     |
| 8PSK250FL  | 64        | 250  | K=13 rate-1/2          | 256     |
| 8PSK125F   | 128       | 125  | K=16 rate-1/2          | 128     |
| 8PSK250F   | 64        | 250  | K=16 rate-1/2          | 256     |
| 8PSK500F   | 32        | 500  | K=13 rate-2/3 punct.   | 512     |
| 8PSK1000F  | 16        | 1000 | K=7 rate-2/3 punct.    | 1024    |
| 8PSK1200F  | 13        | 1200 | K=7 rate-2/3 punct.    | 2048    |

**New building blocks needed on top of Phase-7 PSK infra:**
- `graymapped_8psk` constellation (8 points) + the a-priori `softbits[8][3]` table,
  transcribed verbatim. TX maps a 3-bit symbol via `prevsymbol · pos[sym]` (differential),
  `psk.cxx:2235`; RX takes the received phase index and emits the 3 precomputed soft
  bits, which auto-Gray-decode (`psk.cxx:1716-1810`).
- Convolutional codes not yet in `fec::conv`: **K=13** (`016461`,`012767` octal =
  7473,5623; `psk.cxx:83-85`), **K=16** (`0152711`,`0126723` octal; `psk.cxx:92-94`);
  K=7 (`0x6d`,`0x4f`) already present. Verify against
  `viterbi.cxx` and a `psk_8psk_fec` golden vector.
- **Puncturing** (rate-2/3): the `_puncturing` path drops/inserts a bit per symbol
  (`psk.cxx:2367-2410` TX, `1812` RX depuncture). Add a puncture map to `fec::conv`.
- **Multi-carrier is not needed for plain 8PSK** (numcarriers=1); it *is* reused by the
  OFDM modes below.
- Vestigial-carrier SFFT AFC (`psk.cxx` `vestigial`) — RX-only aid; the loopback gate
  does not require it, defer like DominoEX deferred sync/AFC.

**Gates:** T1 golden vector from the fldigi `encoder` + `graymapped_8psk_softbits`
(bit-exact FEC symbol stream + constellation indices); T4 bit-exact symbol indices; T5
loopback across the grid; T7 AWGN BER sweep for the FEC submodes. Full T1–T9.

---

## Family 3 — OFDM data (planned)

Multi-carrier xPSK/8PSK at 16 kHz (see "Reference correction" above), extending the
same PSK front end + the Phase-7 multi-carrier machinery (`Psk*c*` variants already
run N parallel carriers).

**Submode grid** (`psk.cxx:456-528`, all `samplerate=16000`, `separation=2.0`):

| submode    | symbollen | baud | constellation | carriers | FEC              |
|------------|-----------|------|---------------|----------|------------------|
| OFDM-500F  | 256       | 62.5 | xPSK (2-bit)  | 4        | K? rate-1/2      |
| OFDM-750F  | 128       | 125  | 8PSK          | 3        | rate-1/2         |
| OFDM-2000F | 128       | 125  | 8PSK          | 8        | rate-2/3 punct.  |
| OFDM-2000  | 64        | 250  | 8PSK          | 4        | none             |
| OFDM-3500  | 64        | 250  | 8PSK          | 7        | none             |

**Depends on Family 2** (8PSK constellation + soft decode + the K=13/K=16 codes +
puncturing) and the existing multi-carrier front end. `xPSK` (`graymapped_xpsk_pos`,
`psk.cxx:129-145`) is a Gray-mapped QPSK variant — small addition alongside 8PSK.
Deep interleave depth (`idepth` up to 4800) is carried by the existing `DiagInterleaver`
with a larger depth parameter. Full T1–T9; committed AWGN BER sweep per submode.

---

## Sequencing

1. **PR 16a — Throb** (this PR): self-contained, no PSK dependency, all six submodes.
2. **PR 16b — 8PSK**: the constellation + soft table + K=13/K=16 codes + puncturing on
   top of `psk.rs`; the eleven 8PSK submodes.
3. **PR 16c — OFDM data**: multi-carrier xPSK/8PSK reusing 16b + multi-carrier front
   end; the five OFDM submodes.

Splitting Phase 16 across three PRs (rather than one) is deliberate: 8PSK is a PSK-front-end
extension of a size comparable to Phase 7, and shipping it half-verified would violate
Doctrine §6 (no stubs, gate-green only). Each PR lands a complete, table-tested family.

## Self-review
- **Spec coverage:** every Phase-16 mode in the roadmap maps to a family above
  (Throb×6, 8PSK×11, OFDM×5). The MT63-core assumption is corrected to the reference.
- **Reuse:** Throb is standalone (dual-tone MFSK, `fsk_util` Goertzel + `MFsk`-style
  render). 8PSK/OFDM reuse `fec::conv`, `fec::interleave`, `mfskvaricode`, differential
  PSK, and the multi-carrier front end from Phase 7.
- **Doctrine:** bit-exact on tone-pair / FEC-symbol / constellation-index domains;
  FP-tolerance + loopback on audio; no stubs — each family lands only on green gates.
