# Omnimodem Phase 12 — NAVTEX / SITOR-B + WEFAX (fldigi port)

> **Master plan:** `docs/plans/2026-07-02-omnimodem-full-mode-parity-implementation.md` (the Porting Doctrine + the T1–T9 per-mode task template). **Roadmap line:** `docs/plans/2026-07-02-omnimodem-fldigi-mode-parity.md` §"NAVTEX / SITOR-B" + §"WEFAX". Read those first.

**Goal.** Port fldigi's maritime broadcast text modes **NAVTEX** and **SITOR-B** (CCIR-476 FEC-B, 4-of-7 constant-weight, time-diversity) and the HF weather-fax modes **WEFAX-576** and **WEFAX-288** into omnimodem, bit-exact on the CCIR-476 bit domain and raster-exact on WEFAX loopback, wired end-to-end through the daemon and the TUI.

**Reference (checked out at `../fldigi`, upstream w1hkj/fldigi @ `61b97f413`, tag 4.1.23):**
- `fldigi/src/navtex/navtex.cxx` — CCIR-476 tables + FEC-B decode + 100-baud FSK + TX.
- `fldigi/src/wefax/{wefax.cxx,wefax-pic.cxx}` — FM demod, IOC 576/288 phasing, APT start/stop.

**New building blocks:**
- `crates/dsp/src/fec/ccir476.rs` — CCIR-476 / SITOR FEC-B: 7-bit 4-of-7 code tables (LTRS/FIGS), `char↔code`, the `create_fec` time-diversity interleave, and the DX/RX diversity **decoder**. KAT in isolation first.
- `crates/dsp/src/modes/navtex.rs` — 100-baud 2-FSK (±85 Hz) front end (reuses `frontend::nco::DownConverter` + `frontend::fir::Fir` + `frontend::detector::FmDiscriminator`, as RTTY does), a synchronous 7-bit bit-clock, then `ccir476` FEC-B → `FramePayload::Text`. NAVTEX and SITOR-B share the **identical** on-air CCIR-476 FEC-B / FSK codec: in fldigi they are one modem class whose `m_only_sitor_b` flag changes only the message-*list* segmentation (grouping the RX char stream into distinct ZCZC…NNNN entries), not the decoded characters (navtex.cxx:1000-1044, `process_messages` vs `filter_print`). The port therefore emits the same character stream for both variants (the wire-defining behavior); the ZCZC/NNNN *segmentation* is a display-layer nicety deferred to when the daemon surfaces message boundaries.
- `crates/dsp/src/modes/wefax.rs` — FM modulate/demod (phase-difference discriminator), IOC 576/288 geometry, APT start/phasing/stop, image-line pixel extraction → `FramePayload::Image` (the Phase-10 raster shape).

## Two equivalence classes (Doctrine §3)
- **Bit-exact (asserted byte-for-byte):** CCIR-476 code table, `char_to_code` output for a message, the `create_fec` diversity stream (code sequence + LSB-first bit stream). Golden vector from the reference tables (`navtex_ccir476.json`). Also plain lib unit tests (CI has no `testutil`).
- **FP / loopback (never bit-exact audio):** the 2-FSK audio (NAVTEX) and the FM audio + recovered raster (WEFAX). Gate: our TX → our RX recovers the message text (NAVTEX) / the pixel raster (WEFAX). Cross-decode against the fldigi binary is `#[ignore]`-gated.

---

## NAVTEX / SITOR-B — reference `navtex.cxx`

**CCIR-476 (navtex.cxx:465-592).** 7-bit codewords, exactly 4 bits set (`check_bits`, :570-578). `code_to_ltrs` / `code_to_figs` (:465-487) map a 0..127 code to a letter/figure char (`'_'` = invalid/unused). Reverse maps built by scanning valid codes (:507-521). Control codes: `LTRS 0x5A`, `FIGS 0x36`, `ALPHA 0x0F`, `BETA 0x33`, `char32(space) 0x6A`, `REP 0x66` (:489-494). Bit order on the wire is **LSB first**: `bytes_to_code` reads `bit i` from `pos[i]` (:554-561); `send_string` shifts the code right, sending `c&1` first (:1798-1802).

**TX (navtex.cxx:1735-1804).** `encode` (:1735) = per-char `char_to_code` with running LTRS/FIGS shift (:524-543, minimal shifts). `create_fec` (:1711-1732) = time diversity, `offset = 2` chars: 2×`[REP,ALPHA]` preamble, then per char `[str[i], str[i-2]]` (its own repeat trails by 5 codes = 35 bits), then a 2-char `[char32, str[sz-2+i]]` flush. Then each 7-bit code is FSK'd LSB-first at 100 baud, mark = center+85, space = center−85.

**RX (navtex.cxx:1046-1330).** Synchronous 100-baud bit clock fills a bit buffer; `fec_offset(p)=p−35` locates a char's repeat. `process_bytes` (:1200-1289): try the direct (alpha) code; if `check_bits` fails, try the rep 35 bits back; else soft-combine (`avg[i]=a+r`) and single-bit-flip fallbacks. Character sync (`find_alpha_characters`, :1095-1153) scans 7-bit phases for the offset with the most valid, rep-matching chars. Output via `code_to_char` with LTRS/FIGS state (:1294-1322); NAVTEX also frames on `ZCZC …NNNN` (:1011-1044), SITOR-B streams raw.

**Port shape.** `NavtexVariant { Navtex, SitorB }` (retained for daemon/TUI selection + decoder label; the wire codec is identical, per above). Native rate **8000 Hz** (100 baud → 80 samples/bit, integer). Center default 1000 Hz. TX: `ccir476::encode` + `create_fec` → LSB-first bits → `Fsk2`. RX: `DownConverter`→`Fir`→`FmDiscriminator`, a `GardnerTed` symbol clock at 80 sps producing a soft mark/space per bit, `ccir476::FecBDecoder` fed bit-by-bit → text.

### Tasks
- **T1** Golden vectors: `scratch/refvectors/navtex_ccir476_dump.cxx` + `build_navtex_ccir476.sh` — the CCIR-476 tables/class + `encode`/`create_fec` transcribed **verbatim with cites** from `navtex.cxx` (the modem class can't link standalone: FLTK/fftfilt runtime — same convention as `dominoex_varicode_dump.cxx`). Emits `crates/dsp/tests/vectors/navtex_ccir476.json`: full 128-entry code→char table + per-message `codes`, `fec` stream (hex) and `bits` (LSB-first) for fixed messages. Commit.
- **T2/T3** `fec/ccir476.rs`: tables + `char_to_code`/`code_to_char` + `create_fec` + FEC-B decoder. KAT: `encode`+`create_fec` == golden vector, **bit-exact**; plain unit tests for the table, `check_bits`, shift handling, decode round-trip. Commit.
- **T4/T5** `modes/navtex.rs`: `NavtexMod` (FSK TX) + `NavtexDemod` (FSK RX → FEC-B → text). Loopback recovers text at high SNR for both submodes; noise stays squelched. Commit.
- **T6** Register: `modes/mod.rs`, `ModeConfig::Navtex{submode,center_hz}` + parse + `registry.rs` arms. Unit test. Commit.
- **T7** `#[ignore]` cross-decode note + loopback/BER gate in tests. Commit.
- **T8** TUI: `modes.go` rows (`chat` shape), `modeParamsFor` arm, `NavtexParams` proto + Go regen, Go test. Commit.

---

## WEFAX-576 / WEFAX-288 — reference `wefax.cxx`

**Geometry (wefax.cxx:258-265, 1251-1265).** `IOC_576=576`, `IOC_288=288`; image width = `round(IOC·π)` (576 → 1809 px, 288 → 905 px). WEFAX-576: APT start 300 Hz, **120 LPM**; WEFAX-288: APT start 675 Hz, **60 LPM**. APT stop 450 Hz (:1237). Samples/line = `rate·60/LPM`.

**FM mapping (wefax.cxx:1295, 1939-1996).** Carrier `WEFAX_Center` (1900 Hz), shift `WEFAX_Shift` (800 Hz) → deviation 400 Hz. TX: `freq = carrier + 2·(v−0.5)·dev`, `v = pixel/256` → black(0)=carrier−dev=1500 Hz, white(255)≈carrier+dev=2300 Hz. RX: `pixel = round(255·(0.5 − deviation_ratio·arg(conj(prev)·cur)))`, `deviation_ratio=(rate/shift)/2π`, clamped 0..255.

**Phasing/sync (wefax.cxx:1650-1732, 2004-2154).** TX sequence: APT-start tone (5 s), 20 phasing lines (2.5% white / 95% black / 2.5% white per line), one white line, image lines (each pixel stretched to samples/line and FM'd), APT-stop (5 s), 10 s black. RX: detect APT-start freq, count valid phasing lines (white/black bands), lock line length → LPM and the line-start column, then clock pixels per column into the raster until APT-stop.

**Port shape.** `WefaxVariant { Wefax576, Wefax288 }`. Native rate **11025 Hz** (matches fldigi). `WefaxMod` renders APT/phasing/image/stop; `WefaxDemod` FM-demods, syncs on the phasing lines, assembles the raster, and emits `FramePayload::Image{width, gray}` on flush. Loopback recovers the raster (bit-exact-domain pixels within a small tolerance band, since FM demod is FP — assert structural raster recovery, not audio).

### Tasks
- **T1** Geometry/FM constants captured with cites; a plain unit test for `ioc_to_width`, samples/line, and the pixel↔freq round-trip (no reference binary needed — these are closed-form; documented in the module header against the cited lines).
- **T4/T5** `modes/wefax.rs`: `WefaxMod` + `WefaxDemod`. Loopback: TX a small synthetic gradient image → RX recovers a raster whose columns reproduce the gradient (within FM tolerance). Commit.
- **T6** Register: `ModeConfig::Wefax{submode,center_hz}` + parse + registry. Unit test. Commit.
- **T8** TUI: `modes.go` rows (`image` shape, reuses `raster.go`), `modeParamsFor` arm, `WefaxParams` proto + Go regen, Go test. Commit.

---

## T9 — Phase PR
Full `cargo build` + `cargo test` (lib, no `testutil`, and with `testutil` for the KAT) green; TUI `go test ./...` green; every new mode selectable in the TUI. Branch `feature/omnimodem-phase12-navtex-wefax`, PR titled `Phase 12 — NAVTEX/SITOR-B + WEFAX`, body listing modes, reference commit, new blocks, and KAT/loopback evidence. Commit as `chrissnell` only.

Build/test env: `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0`. Never run `cargo fmt`.
