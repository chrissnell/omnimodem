# Phase W1 — FST4 / FST4W Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port WSJT-X's **FST4** (QSO) and **FST4W** (beacon) modes into omnimodem, bit-exact-compatible on the air with the reference, by transcribing the two LDPC codes they use — **(240,101)/CRC24** and **(240,74)/CRC24** — plus the CRC-24, and assembling a 4-GFSK windowed modem that reuses the FT8 windowed decode path (STFT → sync → soft-LLR → LDPC BP+OSD → CRC → unpack). Every T/R period (15/30/60/120/300/900/1800 s) is a parametric instance of one ported family. The whole phase ships as one PR (T9).

**Architecture:** A new FEC building block `crates/dsp/src/fec/ldpc_fst4.rs` transcribes both parity/generator tables **verbatim** from `wsjtx/lib/fst4/ldpc_240_101_*.f90` / `ldpc_240_74_*.f90` and the CRC-24 from `get_crc24.f90`; it reuses the existing BP decoder (`fec::ldpc::Ldpc`) and OSD (`fec::osd::osd_decode`) unchanged, only supplying new tables. It is **KAT-gated in isolation against `ldpcsim240_101` output before any mode touches it** (Task 0). The mode assembly `crates/dsp/src/modes/fst4.rs` is one file carrying the shared 4-GFSK waveform, the `s8 d30 s8 d30 s8 d30 s8 d30 s8` sync/data frame, and a parametric `Fst4Params` over the T/R grid; FST4 and FST4W differ only in message packing (77-bit vs 50-bit WSPR-format) and the code selected ((240,101) vs (240,74)). Both surface as `BlockDemodulator` + `Modulator` and wire into `mode::registry` exactly like the Phase-5 windowed modes, plus the TUI `modes.go` (T8).

**Tech Stack:** Rust (edition 2021, workspace). Reuses `omnimodem-dsp` blocks: `fec::ldpc::Ldpc` (BP), `fec::osd::osd_decode`, `framing::message77::{pack77,unpack77}` and `message77::legacy::{pack50,unpack50}`, `frontend::modulate::Gfsk`, the FT8-style Goertzel spectrogram + tone-energy soft demap in `modes::ft8`, and the conformance harness (`crates/dsp/tests/{kat.rs,ber.rs,loopback.rs}`, `testutil` AWGN/Watterson). Reference vectors come from the Fortran programs already in-tree: `wsjtx/lib/fst4/ldpcsim240_101.f90`, `ldpcsim240_74.f90`, and `fst4sim.f90` (which prints message bits, channel symbols, and writes a `.wav`).

---

## File structure

**Created:**

| File | Responsibility |
|---|---|
| `crates/dsp/src/fec/ldpc_fst4.rs` | Both FST4 LDPC codes as `Ldpc` instances: `Ldpc::fst4_240_101()` and `Ldpc::fst4_240_74()` built from transcribed generator + parity (`Nm`) tables, plus `get_crc24()` (poly `0x100065b`). Transcription mechanism + KAT hook live here. |
| `crates/dsp/src/fec/fst4_tables.rs` | The verbatim `const` tables: `FST4_101_GEN` / `FST4_74_GEN` (hex-string generator rows) and `FST4_101_NM` / `FST4_101_NRW` / `FST4_74_NM` / `FST4_74_NRW` (parity Tanner graph), each with a `// ref:` cite. Data only, no logic. |
| `crates/dsp/src/modes/fst4.rs` | `Fst4Mod` + `Fst4Demod` (FST4, 77-bit) and `Fst4wMod` + `Fst4wDemod` (FST4W, 50-bit beacon), parametric over `Fst4Params` (T/R grid). 4-GFSK waveform, sync frame, Gray map, soft demap. |
| `crates/dsp/tests/vectors/fst4_reference.json` | Golden vectors from `ldpcsim240_101`/`ldpcsim240_74`/`fst4sim` (provenance header per `vectors/README.md`): `msg`, `msgbits`, `crc24`, `codeword`, `tones`, per T/R period. |
| `crates/dsp/tests/vectors/fst4_reference.provenance.txt` | Upstream commit + exact generating commands. |
| `scratch/refvectors/fst4/` | Notes + patched build steps for running the Fortran drivers (scratch per CLAUDE.md; not shipped). |

**Modified:**

| File | Responsibility |
|---|---|
| `crates/dsp/src/fec/mod.rs` | `pub mod ldpc_fst4; pub mod fst4_tables;`. |
| `crates/dsp/src/modes/mod.rs` | `pub mod fst4;`. |
| `crates/omnimodemd/src/mode/mod.rs` | `ModeConfig::Fst4 { tr_s }` + `Fst4w { tr_s }` variants; `parse`/`to_mode_string`/`label` arms. |
| `crates/omnimodemd/src/mode/registry.rs` | `demod_kind`/`build_modulator`/`native_rate`/`tx_slot_s` arms (windowed, parametric period). |
| `clients/omnimodem-tui/internal/app/modes.go` | `fst4` (sequencer, parametric slot) + `fst4w` (beacon) rows + `modeParamsFor` arms. |
| `crates/dsp/tests/kat.rs` | FST4 KAT (codeword/tones bit-exact; audio FP-tolerance) + `#[ignore]` cross-decode gate. |
| `crates/dsp/tests/ber.rs` | FST4/FST4W decode-rate sweep (AWGN + Watterson) with committed floors. |
| `crates/dsp/tests/loopback.rs` | FST4/FST4W round-trip over the T/R grid. |

**Proto:** FST4/FST4W's only tunable param is the T/R period (an integer choice from the fixed grid) plus optional audio sub-carrier. This mirrors how ft8/ft4/jt65/jt9/wspr carry **no** proto params today (registry test `only_ft8_has_a_tx_slot` and `modeParamsFor`'s `default: nil`). We therefore add a small `Fst4Params { tr_seconds }` proto message so the TUI can pick the period; if the reviewer prefers to keep the period as a bare-label suffix (`fst4:tr=60`) like `cw:wpm=25`, the proto message is dropped and T8 uses the label-tail path. **Decision recorded in Task 6.**

---

## Reference facts (extracted — do not re-derive)

Pinned from `wsjtx/lib/fst4/` at the workspace commit (record the exact hash in the vector provenance at T1):

- **Frame** (`fst4_params.f90`, `genfst4.f90:11-12,99-107`): `NN=160` symbols = `NS=40` sync + `ND=120` data, laid out `s8 d30 s8 d30 s8 d30 s8 d30 s8`. 4-GFSK, `hmod=1`, GFSK pulse `gfsk_pulse(2.0, …)` (BT=2). Sync words `isyncword1={0,1,3,2,1,0,2,3}`, `isyncword2={2,3,1,0,3,2,0,1}` at symbol groups (0-based) 0, 38, 76, 114, 152.
- **Gray map** (`genfst4.f90:85-97`): codeword bit-pair `is = cw[2i] + 2·cw[2i-1]` → tone `{0→0, 1→1, 2→3, 3→2}` (i.e. tones `{0,1,3,2}` indexed by `is`).
- **FST4 (iwspr=0)** (`genfst4.f90:61-67`): 77 message bits from `pack77`, XORed with a fixed 77-bit `rvec` scramble, then CRC-24 appended → `msgbits(101)`; encoded by `encode240_101` → 240-bit codeword (101 systematic + 139 parity).
- **FST4W (iwspr=1)** (`genfst4.f90:47-59,79-83`): `pack77(msg, i3=0, n3=6)` → 50 bits (WSPR-format `CALL GRID dBm`), **no rvec**, CRC-24 over the 74-bit block → `msgbits(74)`; encoded by `encode240_74` → 240-bit codeword (74 systematic + 166 parity).
- **CRC-24** (`get_crc24.f90`): poly `p = 0x100065b` (25-bit `1,0,…,1,1,0,0,1,0,1,1,0,1,1`), computed over `mc(1:len)` with the trailing 24 bits zeroed for generation.
- **(240,101) parity** (`ldpc_240_101_parity.f90`): `Mn(3,240)` (3 checks/bit, `ncw=3`), `Nm(6,139)` (≤6 bits/check, zero-padded), `nrw(139)` valid counts, `M=139`. Generator (`ldpc_240_101_generator.f90`): `g(139)` of 26-hex-char strings; row `i` bit `col=(j-1)*4+jj` set if hex nibble `j` has bit `4-jj` (last nibble `j=26` contributes 1 bit → 101 columns).
- **(240,74) parity** (`ldpc_240_74_parity.f90`): `Nm(6,166)`, `nrw(166)`, `M=166`. Generator: `g(166)` of 19-hex strings (last nibble `j=19` contributes 2 bits → 74 columns).
- **T/R grid** (`fst4sim.f90:52-58`): samples/symbol `nsps` at `fs=12000`: 15 s→720, 30 s→1680, 60 s→3888, 120 s→8200, 300 s→21504, 900 s→66560, 1800 s→134400. Baud `=12000/nsps`; tone spacing `= baud·hmod`; transmission length `= nsps·NN` samples.

---

## Task 0 — LDPC(240,101) + (240,74) + CRC-24 building block (KAT-gated first)

**This is the foundational building block; it lands and its gate is green BEFORE any mode uses it (Doctrine §6).** The existing `Ldpc` type already carries a general BP decoder and a systematic-encode path (`fec/ldpc.rs`); we only add two new table-backed constructors and the CRC-24. The existing `osd_decode` works against any `Ldpc`.

**Files:** `crates/dsp/src/fec/fst4_tables.rs`, `crates/dsp/src/fec/ldpc_fst4.rs`, `crates/dsp/src/fec/mod.rs`, `crates/dsp/tests/vectors/fst4_reference.json` (Task-0 records: codeword + crc), `crates/dsp/tests/kat.rs`.

### T0.1 — Extract the LDPC golden vectors

- [ ] **Step 1:** In `scratch/refvectors/fst4/`, build and run the reference encoders for a fixed message. `ldpcsim240_101` prints `message`, `message with crc24`, and `codeword` for `"K9AN K1JT FN20"` (its default); run `ldpcsim240_74` similarly for a WSPR-format message (e.g. `"K1ABC FN42 30"`). Record the exact commands.

```bash
# scratch/refvectors/fst4/run.sh  (documented, not shipped)
cd wsjtx/lib/fst4
# ldpcsim programs are standalone; compile against packjt77 + the fst4 units.
# Capture stdout: message(77|50) bits, crc24 bits, and the 240-bit codeword.
gfortran -O2 -I<build/include> ldpcsim240_101.f90 encode240_101.f90 get_crc24.f90 \
   decode240_101.f90 osd240_101.f90 <packjt77 objs> -o ldpcsim240_101
./ldpcsim240_101 30 3 1 -1 101 "K9AN K1JT FN20" | tee 101.txt
```

- [ ] **Step 2:** Author `crates/dsp/tests/vectors/fst4_reference.json` with a `_meta` provenance record (upstream commit, files `ldpc_240_101_*.f90`/`ldpc_240_74_*.f90`/`get_crc24.f90`, the commands above) and one record per code: `{ "code": "240_101", "msg": "K9AN K1JT FN20", "msgbits": "…101 bits…", "crc24": "…24 bits…", "codeword": "…240 bits…" }`. Add the sibling `.provenance.txt`. Commit.

### T0.2 — Transcribe the parity + generator tables verbatim

- [ ] **Step 1: Write the failing test** (`crates/dsp/tests/kat.rs`)

```rust
#[test]
fn fst4_240_101_encode_matches_reference() {
    let v = load_json("fst4_reference.json"); // existing helper pattern
    let rec = v.iter().find(|r| r["code"] == "240_101").unwrap();
    let msgbits = bits_of(&rec["msgbits"]); // 101 u8
    let want_cw = bits_of(&rec["codeword"]); // 240 u8
    let code = Ldpc::fst4_240_101();
    assert_eq!(code.encode(&msgbits), want_cw, "codeword must match ldpcsim240_101");
}
```

- [ ] **Step 2: Run it, verify it fails.** `cargo test -p omnimodem-dsp --test kat fst4_240_101_encode_matches_reference` → **FAIL** (`Ldpc::fst4_240_101` undefined).

- [ ] **Step 3: Transcribe the tables** into `crates/dsp/src/fec/fst4_tables.rs`. Copy the hex generator strings and the `Nm`/`nrw` integer tables **byte-for-byte** from the Fortran; do not re-derive. Show the mechanism (generator rows stay as hex strings, unpacked at construction; parity stays as 1-based indices, rebased to 0 at construction):

```rust
// ref: wsjtx/lib/fst4/ldpc_240_101_generator.f90:3-142 (g(139), 26 hex chars each)
pub const FST4_101_GEN: [&str; 139] = [
    "e28df133efbc554bcd30eb1828",
    "b1adf97787f81b4ac02e0caff8",
    // … all 139 rows, verbatim …
    "d559e31b34d21f48e1f501af30",
];

// ref: wsjtx/lib/fst4/ldpc_240_101_parity.f90:243-391 (Nm(6,139), 1-based, 0=pad; nrw(139))
pub const FST4_101_NM: [[u16; 6]; 139] = [
    [3, 52, 95, 102, 140, 182],
    [4, 53, 96, 103, 108, 210],
    // … all 139 checks, verbatim …
];
pub const FST4_101_NRW: [u8; 139] = [
    6,6,6,6,6,6,6,5,5,5,5,5,5,5,5,5,5,5,6,5,
    5,5,5,5,6,5,5,5,6,5,6,5,5,5,6,5,5,5,5,5,
    // … all 139 counts, verbatim from the nrw block …
];
// (240,74) analogues: FST4_74_GEN[166] (19 hex), FST4_74_NM[[u16;6];166], FST4_74_NRW[166].
```

- [ ] **Step 4: Implement the constructors** (`crates/dsp/src/fec/ldpc_fst4.rs`). Reuse the existing `Ldpc` struct via a private raw constructor built from `gen` rows + `check_vars` (mirror `Ldpc::ft8`'s field assembly). Worked encode-side example — unpack the hex generator (Fortran `encode240_101.f90:16-30`) into systematic rows, and build the Tanner graph from `Nm` (rebased 1→0):

```rust
use super::fst4_tables::*;
use super::ldpc::Ldpc;

/// Unpack one generator row's hex string into `ncols` parity-contribution bits.
/// ref: wsjtx/lib/fst4/encode240_101.f90:18-28 — nibble j, bit (4-jj), MSB-first.
fn unpack_gen_row(hex: &str, ncols: usize) -> Vec<u8> {
    let mut bits = Vec::with_capacity(ncols);
    for ch in hex.chars() {
        let nib = ch.to_digit(16).expect("hex") as u8;
        for jj in 1..=4 {
            if bits.len() == ncols { break; }
            bits.push((nib >> (4 - jj)) & 1); // btest(istr, 4-jj)
        }
    }
    debug_assert_eq!(bits.len(), ncols);
    bits
}

impl Ldpc {
    /// FST4/FST4W standard (240,101) code: 77 msg + 24 CRC systematic, 139 parity.
    pub fn fst4_240_101() -> Self {
        const N: usize = 240;
        const K: usize = 101;
        const M: usize = 139;
        // Generator: gen[j] = e_j followed by parity bit j of each of the M rows.
        // Fortran gen(M,K): row i is the parity check, column j the message bit.
        let rows: Vec<Vec<u8>> = FST4_101_GEN.iter().map(|h| unpack_gen_row(h, K)).collect();
        let mut gen = vec![vec![0u8; N]; K];
        for (j, grow) in gen.iter_mut().enumerate() {
            grow[j] = 1;
            for (c, prow) in rows.iter().enumerate() {
                grow[K + c] = prow[j] & 1;
            }
        }
        // Parity Tanner graph from Nm (1-based, 0-padded) → 0-based var lists.
        let mut check_vars = vec![Vec::new(); M];
        for (c, vars) in check_vars.iter_mut().enumerate() {
            for &v in FST4_101_NM[c].iter().take(FST4_101_NRW[c] as usize) {
                vars.push(v as usize - 1);
            }
        }
        Ldpc::from_raw(N, K, gen, check_vars) // small pub(crate) ctor mirroring ft8()
    }

    /// FST4W low-Keff (240,74) code: 50 msg + 24 CRC systematic, 166 parity.
    pub fn fst4_240_74() -> Self { /* identical shape, K=74, M=166, FST4_74_* tables */ }
}
```

Add the `pub(crate) fn from_raw(n,k,gen,check_vars)` helper to `fec/ldpc.rs` (the `ft8()` path currently inlines the same field assembly — refactor it to call `from_raw` so there is one construction point).

- [ ] **Step 5: Run, verify pass.** `cargo test -p omnimodem-dsp --test kat fst4_240_101_encode_matches_reference` → **PASS**. Add the twin `fst4_240_74_encode_matches_reference` (from the `ldpcsim240_74` record) and a **self-consistency** KAT `fst4_generator_satisfies_parity` (`G·Hᵀ=0`: every systematic generator row is a codeword of the `Nm` matrix), mirroring `ft8_ldpc_matches_reference`.

### T0.3 — CRC-24

- [ ] **Step 1: Write the failing test** (`kat.rs`): assert `get_crc24(&msgbits[..77_or_50_zero_padded])` equals the golden `crc24` bits for both records, and that appending the CRC then re-running `get_crc24` over the full block returns 0 (the checker property from `get_crc24.f90:1-5`).

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement `get_crc24`** in `ldpc_fst4.rs`, porting the shift-register division verbatim (`get_crc24.f90:12-23`), poly `0x100065b`:

```rust
// ref: wsjtx/lib/fst4/get_crc24.f90 — 24-bit CRC, poly 0x100065b, over mc(1:len).
// P (25 bits): 1,0…0,1,1,0,0,1,0,1,1,0,1,1
const CRC24_P: [u8; 25] =
    [1,0,0,0,0,0,0,0,0,0,0,0,0,0,1,1,0,0,1,0,1,1,0,1,1];

/// `mc` holds `len` bits; for generation the trailing 24 are zero. Returns the
/// 24-bit remainder as a u32 (bits 23..0).
pub fn get_crc24(mc: &[u8], len: usize) -> u32 {
    let mut r = [0u8; 25];
    r[..25].copy_from_slice(&mc[..25]);
    for i in 0..=(len - 25) {
        r[24] = mc[i + 24]; // Fortran r(25)=mc(i+25), 1-based
        let lead = r[0];
        for k in 0..25 {
            r[k] = (r[k] + lead * CRC24_P[k]) & 1; // mod 2
        }
        r.rotate_left(1); // cshift(r,1)
    }
    (0..24).fold(0u32, |acc, b| (acc << 1) | r[b] as u32)
}
```

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit** Task 0.

```bash
git add crates/dsp/src/fec/{fst4_tables.rs,ldpc_fst4.rs,mod.rs,ldpc.rs} \
        crates/dsp/tests/vectors/fst4_reference.* crates/dsp/tests/kat.rs
git commit -m "feat(dsp): FST4 LDPC(240,101)/(240,74) codes + CRC-24, KAT vs ldpcsim"
```

**Task 0 exit gate:** `fst4_240_101_encode_matches_reference`, `fst4_240_74_encode_matches_reference`, `fst4_generator_satisfies_parity`, and the CRC-24 KATs are green. No mode may reference these constructors until this gate is green.

---

## Task 1 — Extract the FST4/FST4W mode golden vectors (T1)

**Files:** `crates/dsp/tests/vectors/fst4_reference.json`, `scratch/refvectors/fst4/`.

- [ ] **Step 1:** Build `fst4sim` in `scratch/refvectors/fst4/` and run it for a fixed FST4 message and a fixed FST4W message at one representative period (60 s), capturing the printed `Message:`, `Message bits:`/`50-bit message`, and `Channel symbols:` blocks plus the generated `.wav`:

```bash
./fst4sim "K1JT K9AN EN50" 60 1500 0.0 0.0 0.0 1 99 F   # FST4  (W=F)
./fst4sim "K1ABC FN42 30"  60 1500 0.0 0.0 0.0 1 99 T   # FST4W (W=T → iwspr=1)
```

- [ ] **Step 2:** Add per-mode records to `fst4_reference.json`: `{ "mode": "fst4", "tr_s": 60, "msg": …, "msgbits": …(101), "crc24": …, "codeword": …(240), "tones": …(160 values 0..3), "wav": "fst4_60_ref.wav" }` and the FST4W twin (`msgbits` 74, code 240_74). Extend the provenance block with the two `fst4sim` commands + commit hash. Also record the **symbol counts** so the frame-layout test is data-driven: `NN=160`, sync at groups {0,38,76,114,152}.

- [ ] **Step 3: Commit.**

```bash
git add crates/dsp/tests/vectors/fst4_reference.* scratch/refvectors/fst4/
git commit -m "test(fst4): golden tone/codeword/wav vectors from fst4sim + ldpcsim"
```

---

## Task 2 — Source encode: 77-bit (FST4) and 50-bit (FST4W) packing (T2)

**Files:** `crates/dsp/src/modes/fst4.rs`.

- [ ] **Step 1: Write the failing test** (`fst4.rs` inline tests): `fst4_msgbits_match_reference` asserts `fst4_msgbits("K1JT K9AN EN50")` (77 msg XOR rvec, + CRC24 → 101 bits) equals the golden `msgbits`, and `fst4w_msgbits("K1ABC FN42 30")` (50 bits + CRC24 → 74 bits) equals the FST4W golden `msgbits`.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** the two packers. FST4 reuses `message77::pack77` (same source encoding as FT8) then applies the fixed `rvec` scramble and CRC-24; FST4W reuses `message77::legacy::pack50` for the WSPR-format `CALL GRID dBm` then CRC-24 (no rvec):

```rust
use crate::fec::ldpc_fst4::get_crc24;
use crate::framing::message77::{pack77, unpack77};
use crate::framing::message77::legacy::pack50;

// ref: genfst4.f90:29-31 — fixed 77-bit scramble vector.
const RVEC: [u8; 77] = [
    0,1,0,0,1,0,1,0,0,1,0,1,1,1,1,0,1,0,0,0,1,0,0,1,1,0,1,1,0,
    1,0,0,1,0,1,1,0,0,0,0,1,0,0,0,1,0,1,0,0,1,1,1,1,0,0,1,0,1,
    0,1,0,1,0,1,1,0,1,1,1,1,1,0,0,0,1,0,1,
];

/// 101 message bits for FST4: pack77 → 77 bits ⊕ rvec, then CRC-24. ref: genfst4.f90:61-67
pub fn fst4_msgbits(message: &str) -> [u8; 101] {
    let payload = pack77(message);               // [u8;10], 77 bits MSB-first + 3 pad
    let mut bits = [0u8; 101];
    for i in 0..77 {
        let b = (payload[i / 8] >> (7 - (i % 8))) & 1;
        bits[i] = (b ^ RVEC[i]) & 1;             // mod(msgbits+rvec,2)
    }
    let crc = get_crc24(&bits, 101);             // trailing 24 bits are 0 here
    for i in 0..24 { bits[77 + i] = ((crc >> (23 - i)) & 1) as u8; }
    bits
}

/// 74 message bits for FST4W: pack50 (WSPR CALL GRID dBm), no rvec, then CRC-24.
/// ref: genfst4.f90:47-59 (i3=0,n3=6 → 50 bits) + 55-59.
pub fn fst4w_msgbits(message: &str) -> Option<[u8; 74]> {
    let (call, grid, dbm) = parse_wspr_message(message)?; // as in wspr.rs
    let p50 = pack50(&call, &grid, dbm)?;                  // [u8;50]
    let mut bits = [0u8; 74];
    bits[..50].copy_from_slice(&p50);
    let crc = get_crc24(&bits, 74);
    for i in 0..24 { bits[50 + i] = ((crc >> (23 - i)) & 1) as u8; }
    Some(bits)
}
```

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit.** `feat(fst4): 77-bit (FST4) + 50-bit (FST4W) message packing`.

---

## Task 3 — FEC encode → codeword (T3, bit-exact)

**Files:** `crates/dsp/src/modes/fst4.rs`.

- [ ] **Step 1: Write the failing test** `fst4_codeword_matches_reference` / `fst4w_codeword_matches_reference`: `Ldpc::fst4_240_101().encode(&fst4_msgbits(msg))` equals the golden 240-bit `codeword`; twin for (240,74).

- [ ] **Step 2: Run → FAIL** (until wired; the codes already passed Task 0's own KAT, so this confirms the full pack→encode chain).

- [ ] **Step 3: Implement** the chain call inside the modulator (no new code beyond Task 0 + Task 2 composed). This step exists to prove the **composed** stage is bit-exact against `fst4sim`'s `codeword`, per Doctrine §4 (stage boundary, not just end-to-end).

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit.** `test(fst4): codeword bit-exact vs fst4sim (both codes)`.

---

## Task 4 — Modulator: symbols (bit-exact) + 4-GFSK waveform (FP tolerance) (T4)

**Files:** `crates/dsp/src/modes/fst4.rs`.

- [ ] **Step 1: Write the failing tests.**
  - `fst4_tones_match_reference`: `fst4_symbols(msg)` (160 tone indices) equals the golden `tones`, **bit-exact**. Twin for FST4W.
  - `fst4_waveform_len_and_tolerance`: modulated audio length `== NN*nsps` for the chosen period, and (when the reference `.wav` is present) max-abs sample error vs `fst4_60_ref.wav` is within the committed tolerance — **never asserted bit-exact** (Doctrine §3).

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** the parametric params, the frame builder, and the waveform. The Gray map and sync insertion are transcribed from `genfst4.f90:85-107`; the T/R grid from `fst4sim.f90:52-58`:

```rust
// ref: genfst4.f90:27-28 — sync symbol words.
const ISYNC1: [u8; 8] = [0, 1, 3, 2, 1, 0, 2, 3];
const ISYNC2: [u8; 8] = [2, 3, 1, 0, 3, 2, 0, 1];
/// Gray map: is = cw[2i]+2·cw[2i-1] → tone. ref: genfst4.f90:92-97 → tones {0,1,3,2}.
const GRAY: [u8; 4] = [0, 1, 3, 2];
pub const NN: usize = 160;   // sync+data symbols
pub const ND: usize = 120;   // data symbols

#[derive(Clone, Copy)]
pub struct Fst4Params { pub tr_s: u16, pub nsps: usize }
impl Fst4Params {
    /// ref: fst4sim.f90:52-58 (fs=12000).
    pub fn for_tr(tr_s: u16) -> Option<Self> {
        let nsps = match tr_s {
            15 => 720, 30 => 1680, 60 => 3888, 120 => 8200,
            300 => 21504, 900 => 66560, 1800 => 134400, _ => return None,
        };
        Some(Fst4Params { tr_s, nsps })
    }
    pub fn baud(&self) -> f32 { 12000.0 / self.nsps as f32 } // = tone spacing (hmod=1)
    pub fn tone_spacing(&self) -> f32 { self.baud() }        // hmod=1
    pub fn samples(&self) -> usize { NN * self.nsps }
}

/// Build the 160 channel tones (0..3). `codeword` is the 240-bit LDPC output.
/// ref: genfst4.f90:92-108.
fn fst4_tones_from_codeword(cw: &[u8]) -> [u8; NN] {
    let mut itmp = [0u8; ND];
    for i in 0..ND {
        let is = (cw[2 * i + 1] + 2 * cw[2 * i]) as usize; // 0-based: cw[2i], cw[2i+1]
        itmp[i] = GRAY[is];
    }
    let mut t = [0u8; NN];
    t[0..8].copy_from_slice(&ISYNC1);
    t[8..38].copy_from_slice(&itmp[0..30]);
    t[38..46].copy_from_slice(&ISYNC2);
    t[46..76].copy_from_slice(&itmp[30..60]);
    t[76..84].copy_from_slice(&ISYNC1);
    t[84..114].copy_from_slice(&itmp[60..90]);
    t[114..122].copy_from_slice(&ISYNC2);
    t[122..152].copy_from_slice(&itmp[90..120]);
    t[152..160].copy_from_slice(&ISYNC1);
    t
}

pub fn fst4_symbols(message: &str) -> [u8; NN] {
    let cw = Ldpc::fst4_240_101().encode(&fst4_msgbits(message));
    fst4_tones_from_codeword(&cw)
}
```

The waveform reuses `frontend::modulate::Gfsk` (BT=2.0, tone-0 at the sub-carrier). `gen_fst4wave.f90` shapes a Gaussian frequency-deviation pulse over `3*nsps` and cosine-ramps the first/last `nsps/4`; the existing `Gfsk` already produces the equivalent shaped 4-GFSK — the FP tolerance test gates that the omnimodem waveform tracks `fst4sim`'s `.wav` within the committed max-abs error. If the tolerance is not met, port the exact pulse/ramp from `gen_fst4wave.f90:29-90` into a small `Fst4Wave` helper (Gray map is unaffected — bit-domain already passed).

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit.** `feat(fst4): 4-GFSK modulator; tones bit-exact, wave within tolerance`.

---

## Task 5 — Demodulator: sync → soft demap → BP+OSD → CRC → unpack (T5)

**Files:** `crates/dsp/src/modes/fst4.rs`.

- [ ] **Step 1: Write the failing test** `fst4_loopback_decodes` (and `fst4w_loopback_decodes`): modulate → zero-pad to a T/R window → `decode_window` recovers the message at high SNR, over the 60 s period.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** the block demod, reusing the FT8 windowed machinery. FST4 is 4-tone (not 8) with a 5×8 sync arrangement instead of Costas, so:
  - **Spectrogram:** Goertzel tone-energy at 4 tones per symbol on the `baud`-spaced grid (reuse the `modes::ft8` Goertzel; parametric `nsps`/tone count), one row per candidate start slot.
  - **Sync:** slide over base-frequency bins and start offsets, scoring the five 8-symbol sync groups against `ISYNC1`/`ISYNC2` at symbol positions {0,38,76,114,152} — the hard-sync count `is1+…+is5` from `get_fst4_bitmetrics.f90:67-79` (threshold ≥16). This replaces the Costas correlator; the rest of the candidate loop mirrors `ft8.rs`.
  - **Soft demap:** for each of the 120 data symbols, LLRs for its two coded bits from the 4 tone energies (max-of-set difference, the single-symbol `bitmetrics(:,1)` path of `get_fst4_bitmetrics.f90:96-109`; the 2/4/8-symbol correlations are a decode-quality refinement, deferred like FT8's SIC). Undo the Gray map to place the two LLRs in codeword-bit order.
  - **Decode:** `Ldpc::fst4_240_101().decode_minsum(&llrs, 30)`; on parity failure `osd_decode(&code, &llrs, 2)` (both already exist). FST4W selects `fst4_240_74()` and 74 systematic bits.
  - **CRC + unpack:** recompute `get_crc24` over the recovered 101 (or 74) bits and require 0 (`decode240_101.f90:74-88`); for FST4 undo `rvec` on the 77 message bits before `unpack77`; for FST4W `unpack50`. Emit `FramePayload::Text` with `freq_offset_hz`/`time_offset_s` meta like `ft8.rs`.

```rust
impl BlockDemodulator for Fst4Demod {
    fn caps(&self) -> ModeCaps {
        let p = self.params;
        ModeCaps {
            native_rate: 12_000,
            bandwidth_hz: 4.0 * p.baud(),
            tx: false,
            duplex: Duplex::Half,
            shape: DemodShape::Windowed { window_s: p.tr_s as f32, period_s: p.tr_s as f32 },
        }
    }
    fn decode_window(&mut self, window: &[Sample], _t0: u64) -> Vec<Frame> { /* as above */ }
}
```

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit.** `feat(fst4): windowed FST4/FST4W demodulator (sync+soft LLR+BP/OSD)`.

---

## Task 6 — Daemon registry wiring, parametric over the T/R grid (T6)

**Files:** `crates/dsp/src/modes/mod.rs`, `crates/omnimodemd/src/mode/mod.rs`, `crates/omnimodemd/src/mode/registry.rs`, (optionally `proto/*.proto`).

- [ ] **Step 1: Decide the param surface.** FST4/FST4W's only real param is the T/R period. **Decision: carry it as a bare-label tail** (`fst4:tr=60`, `fst4w:tr=120`) exactly like `cw:wpm=25` — no new proto message, matching the existing zero-param windowed modes and keeping the TUI simple (T8 uses `modeParamsFor`'s label-tail defaulting). Record this in the mode file's doc comment. (If a later review wants structured params, add `Fst4Params { tr_seconds }` to the proto and regen — noted, not done.)

- [ ] **Step 2: Write the failing tests** (`mode/mod.rs`): `parse("fst4")` → `Fst4 { tr_s: 60 }` (default), `parse("fst4:tr=120")` → `Fst4 { tr_s: 120 }`, `parse("fst4:tr=7")` → falls back to default 60 (7 not on grid), `parse("fst4w")` → `Fst4w { tr_s: 120 }`; `to_mode_string` round-trips; registry `demod_kind(&Fst4{tr_s:300})` is `Windowed(_, 300.0)` and `tx_slot_s` is `Some(300.0)`.

- [ ] **Step 3: Run → FAIL.**

- [ ] **Step 4: Implement.**
  - `ModeConfig`: add `Fst4 { tr_s: u16 }`, `Fst4w { tr_s: u16 }`; `parse` arms use the `u("tr", 60)` / `u("tr", 120)` helper, validated against the grid (`Fst4Params::for_tr(..).map(|_| tr).unwrap_or(default)`); `to_mode_string` → `format!("fst4:tr={tr_s}")`; `label` → `"fst4"`/`"fst4w"`.
  - `registry.rs`: `demod_kind` → `windowed(Box::new(Fst4Demod::new(*tr_s)))` / `Fst4wDemod::new(*tr_s)`; `build_modulator` → `Fst4Mod::new(*tr_s)` / `Fst4wMod::new(*tr_s)`; `native_rate`/`tx_slot_s` fall out of caps automatically.

- [ ] **Step 5: Run → PASS.** Extend the existing `labels_are_distinct_and_non_empty` and `modulators_build_for_every_mode` lists with the two new variants.

- [ ] **Step 6: Commit.** `feat(omnimodemd): register FST4/FST4W (parametric T/R period)`.

---

## Task 7 — Conformance gates: cross-decode + BER/decode-rate sweep (T7)

**Files:** `crates/dsp/tests/kat.rs`, `crates/dsp/tests/ber.rs`, `crates/dsp/tests/loopback.rs`.

- [ ] **Step 1: Table-test the T/R grid** in `loopback.rs`: a data-driven test enumerates all seven periods `[15,30,60,120,300,900,1800]`, modulates + decodes each for FST4 and FST4W, asserting the message round-trips (the "port the family once, table-test the grid" rule). Keep the long periods behind a `--ignored`/feature guard if the 1800 s buffer is too large for default CI, but include them.

- [ ] **Step 2: BER/decode-rate sweep** in `ber.rs`: `fst4_decode_rate` and `fst4w_decode_rate` over AWGN and Watterson CCIR (reuse `testutil::{add_awgn, WattersonChannel, decode_rate}`) at the 60 s period, with a committed CI floor set just under the observed rate (mirroring the FT8 sweep). Record the curve in the commit message.

- [ ] **Step 3: `#[ignore]` bidirectional cross-decode gate** in `kat.rs`: our TX `.wav` decodes in the reference `jt9`/FST4 decoder AND `fst4sim`-generated `.wav` decodes in `Fst4Demod`, gated behind the reference-binary env var already documented in the `kat.rs` header (extend that header to name the FST4 decoder). This is the decisive interop gate (Doctrine §5).

- [ ] **Step 4: Run** the non-ignored gates → **PASS**; run the sweep and record numbers.

- [ ] **Step 5: Commit.** `test(fst4): T/R-grid loopback, AWGN+Watterson BER, cross-decode gate`.

---

## Task 8 — TUI wiring (T8, mandatory) (T8)

**Files:** `clients/omnimodem-tui/internal/app/modes.go` (+ `modes`/`view_operate` Go tests).

- [ ] **Step 1: Write/extend the failing Go test** (`internal/app/modes_test.go`): `modeByLabel("fst4")` returns a `sequencer` row and `modeByLabel("fst4w")` returns a `beacon` row; `modeParamsFor` handles the `tr` param (or returns nil under the label-tail decision) without panicking. `go test ./...` currently lacks these rows → **FAIL**.

- [ ] **Step 2: Add the rows** to `modes.go`. FST4 is a QSO/sequencer-shaped mode with a selectable T/R period; FST4W is a beacon-shaped mode (like WSPR). `slotSecs` defaults to the mode's default period; the period is an editable param:

```go
{"fst4", "sequencer", 60, []modeParam{{"tr", 60}}},
{"fst4w", "beacon", 120, []modeParam{{"tr", 120}}},
```

Because the T6 decision keeps the period as a bare-label tail, `modeParamsFor` needs no new proto arm — `fst4`/`fst4w` fall through to `default: nil`, and the TUI encodes `fst4:tr=<v>` into the mode string via the existing label-tail path (confirm the operate screen sends `label:k=v` for modes with params; if it only sends structured `ModeParams`, add the `tr` proto message per the T6 fallback and regen with `clients/omnimodem-tui/gen.sh`).

- [ ] **Step 3: Run → PASS.** `cd clients/omnimodem-tui && go test ./...` green. No new view shape is needed — FST4 reuses the existing `sequencer` view, FST4W the `beacon` view.

- [ ] **Step 4: Commit.** `feat(tui): FST4 (sequencer) + FST4W (beacon) in the operate screen`.

---

## Task 9 — Close the phase with a PR (T9)

- [ ] **Step 1:** Ensure the whole workspace builds and all gates are green: `cargo build`, `cargo test -p omnimodem-dsp`, `cargo test -p omnimodem-dsp --features testutil` (BER), `cargo test` (workspace), and `cd clients/omnimodem-tui && go test ./...`.

- [ ] **Step 2:** Confirm every W1 gate: Task-0 LDPC/CRC KATs, FST4/FST4W tones+codeword bit-exact, waveform within tolerance, T/R-grid loopback, AWGN+Watterson BER floors, and both modes selectable in the TUI.

- [ ] **Step 3:** Branch `feature/omnimodem-w1-fst4` (from the integration base), commit history from Tasks 0–8, then push + open the PR (per the master plan's push mechanics + commit identity `chrissnell`):

```bash
git push "https://x-access-token:$(gh auth token)@github.com/chrissnell/omnimodem.git" feature/omnimodem-w1-fst4
gh pr create --repo chrissnell/omnimodem --title "Phase W1 — FST4 / FST4W" \
  --body "Ports WSJT-X FST4 (QSO) + FST4W (beacon). New block fec/ldpc_fst4 (LDPC 240,101 + 240,74 + CRC-24, KAT vs ldpcsim). 4-GFSK windowed modem reusing the FT8 path, parametric over the 15/30/60/120/300/900/1800 s T/R grid. Conformance: tones/codeword bit-exact vs fst4sim, waveform within committed tolerance, AWGN+Watterson decode-rate floors, #[ignore] cross-decode gate. Selectable in the TUI (fst4 sequencer, fst4w beacon)."
```

- [ ] **Step 4:** Request review. Do not merge until reviewed.

---

## Self-review

- **Spec coverage:** Task 0 delivers the W1 building block (`fec/ldpc_fst4.rs`) with **both** LDPC codes — (240,101) [77-bit + CRC-24] and (240,74) [low-Keff, 50-bit WSPR-format + CRC-24] — transcribed verbatim + CRC-24, **KAT-gated against `ldpcsim240_101`/`ldpcsim240_74` before any mode uses it** (Doctrine §6). Tasks 1–9 instantiate the uniform T1–T9 template on the FST4 family: golden vectors from `fst4sim` (T1), 77-bit/50-bit packing (T2), FEC (T3, bit-exact), 4-GFSK modulator (T4, tones bit-exact / audio FP-tolerance), demod (T5), registry (T6, parametric period), conformance incl. cross-decode + AWGN/Watterson BER (T7), TUI (T8), PR (T9). The family is ported once and the T/R grid is table-tested (T7.1), per the parametric-submode rule.
- **Placeholder scan:** every code step shows real Rust — the generator-unpack + Tanner-graph construction, the CRC-24 shift register, the rvec-scrambled 77-bit / CRC-appended 50-bit packers, the Gray-mapped sync-inserted tone builder, and the demod outline — each with `// ref:` cites to exact Fortran lines. The large tables are shown as `const` arrays transcribed verbatim from the named parity/generator files with a self-consistency `G·Hᵀ=0` KAT and an encode-equality KAT against `ldpcsim`, never "port the table" with no mechanism. No `todo!()`/stub closes any gate.
- **Type consistency:** reuses `Ldpc` (extended with a `pub(crate) from_raw` ctor shared with `ft8()`), `osd_decode`, `Gfsk`, `pack77`/`unpack77`, `legacy::pack50`/`unpack50`, `FramePayload::Text`, `DemodShape::Windowed { window_s, period_s }`. `ModeConfig::Fst4 { tr_s }` / `Fst4w { tr_s }` extend the existing parametric-enum + `parse`/`registry` pattern; `Fst4Params::for_tr` centralizes the T/R→nsps grid so period is parametric everywhere (caps, TX slot, TUI slotSecs).

## Execution handoff

Execute with superpowers:subagent-driven-development (fresh subagent per task, two-stage review) or superpowers:executing-plans. **Task 0 must land and go green first** — it is the shared FEC block every FST4/FST4W stage depends on, and the Doctrine forbids building the mode on an unverified code. Then Tasks 1–8 in order (each closing on its own passing gate, never a stub), and Task 9 opens the single Phase-W1 PR. Record the exact upstream commit hash in `fst4_reference.provenance.txt` at T1 so the vectors are reproducible. The `#[ignore]` cross-decode gate (T7.3) is the decisive interop proof; loopback + BER are necessary but not sufficient.
