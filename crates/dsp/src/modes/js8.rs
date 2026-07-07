//! JS8 (JS8Call) — 8-GFSK windowed mode built on the early-FT8 core.
//!
//! Port of js8call (upstream `js8call/js8call` @ a7ff1be). JS8 shares FT8's
//! 79-symbol, 8-tone frame but uses the LDPC(174,87)/CRC-12 channel code
//! ([`crate::fec::ldpc_js8`]) and its own Costas arrays. The four on-air
//! submodes **Normal / Fast / Turbo / Slow** (plus the disabled **Ultra**) are
//! one parametric family differing only in samples-per-symbol, tone spacing,
//! T-R period, and which Costas variant they use.
//!
//! This module currently provides the submode parameter table, the Costas
//! arrays, and the **TX channel-symbol assembly** (`js8_symbols`, bit-exact vs
//! the reference `genjs8`). The `Modulator`/`BlockDemodulator` waveform + decode
//! and daemon/TUI registration are assembled on top of this foundation.
//!
//! ref: js8call/lib/js8/genjs8.f90 (tone assembly + Costas), JS8Submode.cpp +
//! commons.h + lib/js8/js8{a,b,c,e,i}_params.f90 (submode grid).

use crate::fec::ldpc_js8::encode174;

/// Sample rate (all submodes). ref: commons.h `JS8_RX_SAMPLE_RATE`.
pub const JS8_RATE: u32 = 12_000;
/// Total channel symbols: 21 sync (3×7 Costas) + 58 data. ref: genjs8.f90 `NN`.
pub const JS8_NSYM: usize = 79;
/// Data symbols (each 3 LDPC codeword bits). ref: genjs8.f90 `ND`.
pub const JS8_ND: usize = 58;
/// Costas group start positions within the 79-symbol frame: 0–6, 36–42, 72–78.
pub const JS8_COSTAS_STARTS: [usize; 3] = [0, 36, 72];

/// Original 7×7 Costas array (JS8 Normal, `NCOSTAS=1` — same array in all three
/// groups). ref: genjs8.f90:23-25.
pub const JS8_COSTAS_ORIG: [u8; 7] = [4, 2, 5, 6, 1, 3, 0];
/// Symmetrical Costas arrays A/B/C (JS8 Fast/Turbo/Slow/Ultra, `NCOSTAS=2` —
/// distinct arrays per group). ref: genjs8.f90:27-31.
pub const JS8_COSTAS_SYM_A: [u8; 7] = [0, 6, 2, 3, 5, 4, 1];
pub const JS8_COSTAS_SYM_B: [u8; 7] = [1, 5, 0, 2, 3, 6, 4];
pub const JS8_COSTAS_SYM_C: [u8; 7] = [2, 5, 0, 6, 4, 1, 3];

/// JS8 submodes. Discriminants match `Varicode::SubmodeType`. ref: varicode.h:26.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Js8Submode {
    Normal = 0,
    Fast = 1,
    Turbo = 2,
    Slow = 4,
    /// Defined in the reference but disabled in its calling code; ported for
    /// completeness, not registered as a selectable mode.
    Ultra = 8,
}

/// Per-submode constant parameters. ref: JS8Submode.cpp + `js8{a,b,c,e,i}_params.f90`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Js8Params {
    pub name: &'static str,
    /// Samples per symbol at 12 kHz (`NSPS`).
    pub nsps: usize,
    /// Nominal T-R period / transmission length, seconds (`NTXDUR`).
    pub tx_seconds: u32,
    /// TX start delay, ms (`ASTART`).
    pub start_delay_ms: u32,
    /// Tone spacing = 12000 / nsps, Hz.
    pub tone_spacing: f64,
    /// Costas variant: `false` → original (Normal), `true` → symmetrical.
    pub symmetrical_costas: bool,
}

impl Js8Submode {
    pub fn params(self) -> Js8Params {
        match self {
            // name, nsps, tx_seconds, start_delay_ms, tone_spacing, symmetrical
            Js8Submode::Normal => Js8Params { name: "NORMAL", nsps: 1920, tx_seconds: 15, start_delay_ms: 500, tone_spacing: 12000.0 / 1920.0, symmetrical_costas: false },
            Js8Submode::Fast => Js8Params { name: "FAST", nsps: 1200, tx_seconds: 10, start_delay_ms: 200, tone_spacing: 12000.0 / 1200.0, symmetrical_costas: true },
            Js8Submode::Turbo => Js8Params { name: "TURBO", nsps: 600, tx_seconds: 6, start_delay_ms: 100, tone_spacing: 12000.0 / 600.0, symmetrical_costas: true },
            Js8Submode::Slow => Js8Params { name: "SLOW", nsps: 3840, tx_seconds: 30, start_delay_ms: 500, tone_spacing: 12000.0 / 3840.0, symmetrical_costas: true },
            Js8Submode::Ultra => Js8Params { name: "ULTRA", nsps: 384, tx_seconds: 4, start_delay_ms: 100, tone_spacing: 12000.0 / 384.0, symmetrical_costas: true },
        }
    }

    /// The three 7-symbol Costas arrays for this submode's frame groups.
    pub fn costas(self) -> [[u8; 7]; 3] {
        if self.params().symmetrical_costas {
            [JS8_COSTAS_SYM_A, JS8_COSTAS_SYM_B, JS8_COSTAS_SYM_C]
        } else {
            [JS8_COSTAS_ORIG; 3]
        }
    }
}

/// Assemble the 79 channel-symbol tones (0–7) from an 87-bit message, bit-exact
/// with `genjs8`: LDPC-encode → three Costas groups at 0/36/72 → 58 data tones
/// via the **plain-binary** 3-bit map `cw[3j]·4 + cw[3j+1]·2 + cw[3j+2]`. Frame
/// layout `S7 D29 S7 D29 S7`. ref: genjs8.f90:44-56.
pub fn js8_symbols(msgbits: &[u8; 87], submode: Js8Submode) -> [u8; JS8_NSYM] {
    let cw = encode174(msgbits);
    let costas = submode.costas();
    let mut itone = [0u8; JS8_NSYM];
    for (g, &start) in JS8_COSTAS_STARTS.iter().enumerate() {
        itone[start..start + 7].copy_from_slice(&costas[g]);
    }
    for j in 0..JS8_ND {
        // First 29 data symbols occupy 7..36, next 29 occupy 43..72.
        let pos = if j < 29 { 7 + j } else { 43 + (j - 29) };
        let b = 3 * j;
        itone[pos] = cw[b] * 4 + cw[b + 1] * 2 + cw[b + 2];
    }
    itone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_vec(field: &str, raw: &str) -> Vec<u8> {
        let i = raw.find(field).unwrap() + field.len();
        let s = &raw[i..raw[i..].find('"').unwrap() + i];
        s.split_whitespace().map(|t| t.parse().unwrap()).collect()
    }
    fn parse_bits(field: &str, raw: &str) -> Vec<u8> {
        let i = raw.find(field).unwrap() + field.len();
        raw[i..raw[i..].find('"').unwrap() + i].bytes().map(|c| c - b'0').collect()
    }

    /// Bit-exact: `js8_symbols` reproduces the reference `genjs8` tone sequence
    /// for both Costas variants. Provenance: `tests/vectors/js8_symbols.json`
    /// (js8call @ a7ff1be, driver `scratch/refvectors/js8/build_ldpc.sh`).
    #[test]
    fn js8_symbols_match_reference() {
        let raw = include_str!("../../tests/vectors/js8_symbols.json");
        let msgbits = parse_bits("\"msgbits\": \"", raw);
        let mut m = [0u8; 87];
        m.copy_from_slice(&msgbits);
        let orig = parse_vec("\"itone_orig\": \"", raw);
        let sym = parse_vec("\"itone_sym\": \"", raw);

        assert_eq!(js8_symbols(&m, Js8Submode::Normal).to_vec(), orig, "Normal (original Costas) itone mismatch");
        // Fast/Turbo/Slow/Ultra all share the symmetrical Costas + the same data map.
        for sm in [Js8Submode::Fast, Js8Submode::Turbo, Js8Submode::Slow, Js8Submode::Ultra] {
            assert_eq!(js8_symbols(&m, sm).to_vec(), sym, "{:?} (symmetrical Costas) itone mismatch", sm);
        }
    }

    /// Costas groups sit at 0/36/72 and only they differ between variants; the
    /// 58 data tones are identical across all submodes.
    #[test]
    fn data_tones_shared_costas_differ() {
        let m = [1u8; 87];
        let normal = js8_symbols(&m, Js8Submode::Normal);
        let fast = js8_symbols(&m, Js8Submode::Fast);
        for j in 0..JS8_NSYM {
            let in_costas = JS8_COSTAS_STARTS.iter().any(|&s| (s..s + 7).contains(&j));
            if in_costas {
                continue; // may differ
            }
            assert_eq!(normal[j], fast[j], "data tone {j} differs between submodes");
        }
        // Costas groups are exactly the declared arrays.
        assert_eq!(&normal[0..7], &JS8_COSTAS_ORIG);
        assert_eq!(&fast[0..7], &JS8_COSTAS_SYM_A);
        assert_eq!(&fast[36..43], &JS8_COSTAS_SYM_B);
        assert_eq!(&fast[72..79], &JS8_COSTAS_SYM_C);
    }

    /// Submode grid matches the reference constants (`JS8Submode.cpp`).
    #[test]
    fn submode_grid() {
        assert_eq!(Js8Submode::Normal.params().nsps, 1920);
        assert_eq!(Js8Submode::Fast.params().nsps, 1200);
        assert_eq!(Js8Submode::Turbo.params().nsps, 600);
        assert_eq!(Js8Submode::Slow.params().nsps, 3840);
        assert_eq!(Js8Submode::Ultra.params().nsps, 384);
        assert!((Js8Submode::Normal.params().tone_spacing - 6.25).abs() < 1e-9);
        assert!(!Js8Submode::Normal.params().symmetrical_costas);
        assert!(Js8Submode::Fast.params().symmetrical_costas);
    }
}
