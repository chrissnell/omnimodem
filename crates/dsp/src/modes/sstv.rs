//! SSTV (Slow-Scan Television): analog FM-subcarrier line-scan picture modes.
//!
//! Port of the MMSSTV DSP core (`n5ac/mmsstv`, LGPLv3, upstream commit `8060b5f`).
//! SSTV is a self-contained analog modem: a VIS header identifies the submode, then
//! each picture line is an FM-subcarrier scan where pixel luminance maps linearly to
//! frequency (black 1500 Hz → white 2300 Hz, `ColorToFreq`, ref: ComLib.cpp:3491) with
//! 1200 Hz sync and 1500 Hz porch/separator pulses. RX output is a raster
//! (`FramePayload::Image`), never text.
//!
//! This module is the *foundation layer* (plan `docs/plans/2026-07-06-omnimodem-sstv.md`,
//! task groups F1.2 + per-family T2): submode identity, picture geometry, colour model,
//! and the VIS codec. The modulator (T4), demodulator (T5), sync (F1.3) and colour
//! reconstruction (F1.4) build on these and are gated against the golden vectors under
//! `crates/dsp/tests/vectors/sstv_*` produced by the isolated MMSSTV harness
//! (`scratch/refvectors/build_sstv_{tx,rx}.sh`, which links the *unmodified* reference).
//!
//! Native sample rate is 11025 Hz (ref: Main.cpp:212); all reference timing is `ms`.
//!
//! Bit-exact domains (Doctrine §3): VIS byte codes, picture geometry, and the decoded
//! pixel raster. FP-tolerance domain: the modulated audio (VCO + BPF). The VIS + timing
//! constants here are transcribed verbatim from the reference with `// ref:` cites.

/// Every MMSSTV submode, in the reference's `SSTVModeList[]` order (ref: sstv.cpp:493-503,
/// enum `sm*` sstv.h:450-495). Parametric families share a decode/colour path; the enum is
/// the identity used by the registry, TUI, and table-driven KATs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SstvMode {
    // Robot
    Robot36, Robot72, Robot24, Bw8, Bw12,
    // AVT
    Avt90,
    // Scottie
    Scottie1, Scottie2, ScottieDx,
    // Martin
    Martin1, Martin2,
    // SC2
    Sc2_180, Sc2_120, Sc2_60,
    // PD
    Pd50, Pd90, Pd120, Pd160, Pd180, Pd240, Pd290,
    // Pasokon
    P3, P5, P7,
    // MMSSTV MR / ML (extended VIS)
    Mr73, Mr90, Mr115, Mr140, Mr175,
    Ml180, Ml240, Ml280, Ml320,
    // MMSSTV MP (extended VIS)
    Mp73, Mp115, Mp140, Mp175,
    // Narrow-band MP-N / MC-N (N-VIS FSK)
    Mn73, Mn110, Mn140, Mc110, Mc140, Mc180,
}

/// Colour/line reconstruction family. Determines how the per-line channel samples become
/// RGB pixels (colour math ref: ComLib.cpp `YCtoRGB`:3475 / `GetRY`:3650).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorModel {
    /// Sequential RGB channels per line (Scottie, Martin, SC2, MC-narrow, Pasokon P).
    Rgb,
    /// Robot colour-difference: Y then alternating R-Y / B-Y (R36/R24), or Y,R-Y,B-Y per
    /// line (R72). ref: Main.cpp:3856-3947.
    RobotColor,
    /// PD/MP colour-difference: Y(odd), R-Y, B-Y, Y(even) — two picture rows per scan.
    /// ref: Main.cpp:3948-4011.
    PdColor,
    /// MR/ML colour-difference: Y full-width + horizontally-compressed R-Y/B-Y.
    MrColor,
    /// Monochrome luminance only (B/W 8, B/W 12). ref: sstv.cpp:1015-1035.
    Mono,
    /// AVT 90: colour-difference with no sync pulses (special framing). ref: sstv.cpp:681-690.
    Avt,
}

/// The VIS identifier form for a submode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vis {
    /// Classic 8-bit VIS (LSB-first tone bits, 1=1100 Hz / 0=1300 Hz). ref: Main.cpp:6987-7092.
    Standard(u8),
    /// MMSSTV 16-bit extended VIS; low byte is the `0x23` marker (ref: mode.txt §1, RX
    /// sstv.cpp:1993-2122). Sent as 16 LSB-first tone bits.
    Extended(u16),
    /// Narrow-band N-VIS: 24-bit FSK, 6-bit symbols (1=1900 Hz / 0=2100 Hz). The byte here
    /// is the mode's N-VIS value `D2`; the full frame is `101101 010101 D2 (010101^D2)`.
    /// ref: mode.txt §7, Main.cpp:6946-6969.
    Narrow(u8),
}

/// Picture geometry: on-air pixels per line, displayed picture rows, and the number of
/// transmitted scan lines (`m_L`). For PD/MP families two picture rows are produced per
/// scan, so `rows == 2 * scan_lines` there. ref: sstv.cpp `GetBitmapSize`:607 /
/// `GetPictureSize`:638 and the per-mode `m_L` in `SetSampFreq`:655-1161.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub width: u16,
    pub rows: u16,
    pub scan_lines: u16,
}

impl SstvMode {
    /// The VIS identifier. Table transcribed verbatim from Main.cpp:6987-7092 (standard +
    /// extended) and the N-VIS switch Main.cpp:6946-6969 / mode.txt §4-7 (narrow).
    pub fn vis(self) -> Vis {
        use SstvMode::*;
        use Vis::*;
        match self {
            Robot36 => Standard(0x88),
            Robot72 => Standard(0x0c),
            Robot24 => Standard(0x84),
            Bw8 => Standard(0x82),
            Bw12 => Standard(0x86),
            Avt90 => Standard(0x44),
            Scottie1 => Standard(0x3c),
            Scottie2 => Standard(0xb8),
            ScottieDx => Standard(0xcc),
            Martin1 => Standard(0xac),
            Martin2 => Standard(0x28),
            Sc2_180 => Standard(0xb7),
            Sc2_120 => Standard(0x3f),
            Sc2_60 => Standard(0xbb),
            Pd50 => Standard(0xdd),
            Pd90 => Standard(0x63),
            Pd120 => Standard(0x5f),
            Pd160 => Standard(0xe2),
            Pd180 => Standard(0x60),
            Pd240 => Standard(0xe1),
            Pd290 => Standard(0xde),
            P3 => Standard(0x71),
            P5 => Standard(0x72),
            P7 => Standard(0xf3),
            Mr73 => Extended(0x4523),
            Mr90 => Extended(0x4623),
            Mr115 => Extended(0x4923),
            Mr140 => Extended(0x4a23),
            Mr175 => Extended(0x4c23),
            Ml180 => Extended(0x8523),
            Ml240 => Extended(0x8623),
            Ml280 => Extended(0x8923),
            Ml320 => Extended(0x8a23),
            Mp73 => Extended(0x2523),
            Mp115 => Extended(0x2923),
            Mp140 => Extended(0x2a23),
            Mp175 => Extended(0x2c23),
            Mn73 => Narrow(0x02),
            Mn110 => Narrow(0x04),
            Mn140 => Narrow(0x05),
            Mc110 => Narrow(0x14),
            Mc140 => Narrow(0x15),
            Mc180 => Narrow(0x16),
        }
    }

    /// Colour/line family. ref: the per-mode reconstruction switch Main.cpp:3800-4011.
    pub fn color_model(self) -> ColorModel {
        use ColorModel::*;
        use SstvMode::*;
        match self {
            Scottie1 | Scottie2 | ScottieDx | Martin1 | Martin2
            | Sc2_180 | Sc2_120 | Sc2_60 | P3 | P5 | P7
            | Mc110 | Mc140 | Mc180 => Rgb,
            Robot36 | Robot72 | Robot24 => RobotColor,
            Pd50 | Pd90 | Pd120 | Pd160 | Pd180 | Pd240 | Pd290
            | Mp73 | Mp115 | Mp140 | Mp175
            | Mn73 | Mn110 | Mn140 => PdColor,
            Mr73 | Mr90 | Mr115 | Mr140 | Mr175
            | Ml180 | Ml240 | Ml280 | Ml320 => MrColor,
            Bw8 | Bw12 => Mono,
            Avt90 => Avt,
        }
    }

    /// Picture geometry. ref: sstv.cpp `GetBitmapSize`:607-635, `GetPictureSize`:638-653,
    /// and per-mode `m_L` in `SetSampFreq`.
    pub fn geometry(self) -> Geometry {
        use SstvMode::*;
        let g = |width: u16, rows: u16, scan_lines: u16| Geometry { width, rows, scan_lines };
        match self {
            // Robot colour: picture height hp=240 (GetPictureSize), m_L scan lines.
            Robot36 | Robot72 | Avt90 => g(320, 240, 240),
            Robot24 => g(320, 240, 120),
            Bw8 | Bw12 => g(320, 240, 120),
            // Scottie / Martin / SC2: 320x256, 256 lines.
            Scottie1 | Scottie2 | ScottieDx | Martin1 | Martin2
            | Sc2_180 | Sc2_120 | Sc2_60 => g(320, 256, 256),
            // PD: two picture rows per scan (m_L scans → 2*m_L rows).
            Pd50 => g(320, 256, 128),
            Pd90 => g(320, 256, 128),
            Pd120 => g(640, 496, 248),
            Pd160 => g(512, 400, 200),
            Pd180 => g(640, 496, 248),
            Pd240 => g(640, 496, 248),
            Pd290 => g(800, 616, 308),
            // Pasokon P: 640x496, 496 lines.
            P3 | P5 | P7 => g(640, 496, 496),
            // MMSSTV MR (320x256) / ML (640x496).
            Mr73 | Mr90 | Mr115 | Mr140 | Mr175 => g(320, 256, 256),
            Ml180 | Ml240 | Ml280 | Ml320 => g(640, 496, 496),
            // MMSSTV MP: 320x256, two rows/scan (128 scans).
            Mp73 | Mp115 | Mp140 | Mp175 => g(320, 256, 128),
            // Narrow MP-N (two rows/scan, 128 scans) / MC-N (256 lines).
            Mn73 | Mn110 | Mn140 => g(320, 256, 128),
            Mc110 | Mc140 | Mc180 => g(320, 256, 256),
        }
    }

    /// Whether this is a narrow-band mode (2044–2300 Hz scan, N-VIS). ref: sstv.cpp
    /// `IsNarrowMode`:550-563.
    pub fn is_narrow(self) -> bool {
        matches!(self.vis(), Vis::Narrow(_))
    }

    /// Stable lowercase label used by the registry, TUI and CLI.
    pub fn label(self) -> &'static str {
        use SstvMode::*;
        match self {
            Robot36 => "robot36", Robot72 => "robot72", Robot24 => "robot24",
            Bw8 => "bw8", Bw12 => "bw12", Avt90 => "avt90",
            Scottie1 => "scottie1", Scottie2 => "scottie2", ScottieDx => "scottiedx",
            Martin1 => "martin1", Martin2 => "martin2",
            Sc2_180 => "sc2-180", Sc2_120 => "sc2-120", Sc2_60 => "sc2-60",
            Pd50 => "pd50", Pd90 => "pd90", Pd120 => "pd120", Pd160 => "pd160",
            Pd180 => "pd180", Pd240 => "pd240", Pd290 => "pd290",
            P3 => "p3", P5 => "p5", P7 => "p7",
            Mr73 => "mr73", Mr90 => "mr90", Mr115 => "mr115", Mr140 => "mr140", Mr175 => "mr175",
            Ml180 => "ml180", Ml240 => "ml240", Ml280 => "ml280", Ml320 => "ml320",
            Mp73 => "mp73", Mp115 => "mp115", Mp140 => "mp140", Mp175 => "mp175",
            Mn73 => "mp73-n", Mn110 => "mp110-n", Mn140 => "mp140-n",
            Mc110 => "mc110-n", Mc140 => "mc140-n", Mc180 => "mc180-n",
        }
    }

    pub fn from_label(s: &str) -> Option<SstvMode> {
        SstvMode::all().iter().copied().find(|m| m.label() == s)
    }

    /// Every ported submode, for table-driven tests, the registry and the TUI.
    pub fn all() -> &'static [SstvMode] {
        use SstvMode::*;
        &[
            Robot36, Robot72, Robot24, Bw8, Bw12, Avt90,
            Scottie1, Scottie2, ScottieDx, Martin1, Martin2,
            Sc2_180, Sc2_120, Sc2_60,
            Pd50, Pd90, Pd120, Pd160, Pd180, Pd240, Pd290,
            P3, P5, P7,
            Mr73, Mr90, Mr115, Mr140, Mr175,
            Ml180, Ml240, Ml280, Ml320,
            Mp73, Mp115, Mp140, Mp175,
            Mn73, Mn110, Mn140, Mc110, Mc140, Mc180,
        ]
    }
}

/// The VIS codec (plan F1.2): the tone/timing sequence that frames a submode's identity
/// on the wire, and its inverse. A "symbol" is one `(freq_hz, ms)` write, matching the
/// reference `CSSTVMOD::Write(fq, ms)` domain — this is a **bit-exact** stage.
pub mod vis {
    use super::{SstvMode, Vis};

    /// One transmit symbol: hold `freq_hz` for `ms` milliseconds.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Symbol {
        pub freq_hz: u16,
        pub ms: f64,
    }
    const fn sym(freq_hz: u16, ms: f64) -> Symbol {
        Symbol { freq_hz, ms }
    }

    // Standard/extended VIS tones (ref: Main.cpp:7098-7104).
    const BIT1_HZ: u16 = 1100;
    const BIT0_HZ: u16 = 1300;
    // N-VIS FSK tones (ref: mode.txt §7).
    const NVIS1_HZ: u16 = 1900;
    const NVIS0_HZ: u16 = 2100;

    /// The standard leader + VIS-bit sequence for a submode's VIS header, as the exact
    /// `(freq, ms)` writes the reference emits (ref: Main.cpp:6940-7107). Narrow modes use
    /// the 24-bit N-VIS FSK framing instead (ref: mode.txt §7 / Main.cpp:6936-6970).
    pub fn header(mode: SstvMode) -> Vec<Symbol> {
        let mut out = Vec::new();
        match mode.vis() {
            Vis::Standard(byte) => {
                push_leader(&mut out);
                push_bits(&mut out, byte as u32, 8);
                out.push(sym(1200, 30.0)); // stop
            }
            Vis::Extended(word) => {
                push_leader(&mut out);
                push_bits(&mut out, word as u32, 16);
                out.push(sym(1200, 30.0)); // stop
            }
            Vis::Narrow(d2) => push_nvis(&mut out, d2),
        }
        out
    }

    // 1900/300, 1200/10, 1900/300, 1200/30 (ref: Main.cpp:6975-6978).
    fn push_leader(out: &mut Vec<Symbol>) {
        out.push(sym(1900, 300.0));
        out.push(sym(1200, 10.0));
        out.push(sym(1900, 300.0));
        out.push(sym(1200, 30.0));
    }

    // `n` VIS bits, LSB first, 30 ms each: bit 1 → 1100 Hz, bit 0 → 1300 Hz.
    fn push_bits(out: &mut Vec<Symbol>, mut d: u32, n: u32) {
        for _ in 0..n {
            out.push(sym(if d & 1 != 0 { BIT1_HZ } else { BIT0_HZ }, 30.0));
            d >>= 1;
        }
    }

    // 24-bit N-VIS FSK: preamble 1900/300, 2100/100, start 1900/22, then four 6-bit
    // symbols D0=101101, D1=010101, D2=mode, D3=010101^mode — each bit 22 ms,
    // MSB(D05..D00) first, 1=1900 Hz/0=2100 Hz. ref: mode.txt §7.
    fn push_nvis(out: &mut Vec<Symbol>, d2: u8) {
        out.push(sym(1900, 300.0));
        out.push(sym(2100, 100.0));
        out.push(sym(1900, 22.0)); // start bit
        let d0 = 0b101101u8;
        let d1 = 0b010101u8;
        let d3 = d1 ^ d2;
        for sym6 in [d0, d1, d2, d3] {
            for bit in (0..6).rev() {
                let hi = (sym6 >> bit) & 1 != 0;
                out.push(sym(if hi { NVIS1_HZ } else { NVIS0_HZ }, 22.0));
            }
        }
    }

    /// Decode a run of received VIS bit-tone frequencies (1100/1300 Hz, LSB first) back to
    /// the identifying value, then to a submode. `extended` selects 16-bit framing. This is
    /// the RX counterpart of `push_bits` (ref: sstv.cpp:1979-1990); tone→bit is `d11 > d13`.
    pub fn decode_bits(tones_hz: &[u16], extended: bool) -> Option<SstvMode> {
        let n = if extended { 16 } else { 8 };
        if tones_hz.len() < n {
            return None;
        }
        let mut v = 0u32;
        for (i, &t) in tones_hz.iter().take(n).enumerate() {
            let bit = if nearer(t, BIT1_HZ, BIT0_HZ) { 1 } else { 0 };
            v |= bit << i;
        }
        let target = if extended {
            Vis::Extended(v as u16)
        } else {
            Vis::Standard(v as u8)
        };
        SstvMode::all().iter().copied().find(|m| m.vis() == target)
    }

    fn nearer(t: u16, a: u16, b: u16) -> bool {
        let (t, a, b) = (t as i32, a as i32, b as i32);
        (t - a).abs() <= (t - b).abs()
    }
}

/// TX modulator symbol layer (plan T4): builds the exact `(freq_hz, ms)` write sequence a
/// picture produces — the **bit-exact** symbol domain, before the FP tone renderer. Each
/// family's line layout is transcribed from the reference `TMmsstv::LineXXX` with `// ref:`
/// cites; audio rendering (VCO) is a later, FP-tolerance stage.
pub mod modulator {
    use super::vis::{header, Symbol};
    use super::SstvMode;

    /// A source pixel, channels 0–255. Matches the reference `COLD.b` byte order (r low).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Rgb {
        pub r: u8,
        pub g: u8,
        pub b: u8,
    }

    /// Pixel luminance → scan frequency, integer-exact. ref: ComLib.cpp:3491 `ColorToFreq`:
    /// `d = d*(2300-1500)/256; return d + 1500;` (black 1500 Hz → white 2300 Hz).
    pub fn color_to_freq(v: u8) -> u16 {
        ((v as i32) * (2300 - 1500) / 256 + 1500) as u16
    }

    /// The RGB-sequential family kind (Scottie / Martin / SC2), which fixes the per-line
    /// sync + porch layout and channel order.
    #[derive(Clone, Copy, PartialEq)]
    pub enum RgbFamily {
        Scottie,
        Martin,
        Sc2,
    }

    /// Per-submode RGB-sequential parameters: family, scan window `tw` (ms), and the sync
    /// pulse length `s` (ms). ref: the TX dispatch Main.cpp:6620-6641.
    pub fn rgb_params(mode: SstvMode) -> Option<(RgbFamily, f64, f64)> {
        use RgbFamily::*;
        use SstvMode::*;
        Some(match mode {
            Scottie1 => (Scottie, 138.24, 9.0),
            Scottie2 => (Scottie, 88.064, 9.0),
            ScottieDx => (Scottie, 345.6, 9.0),
            Martin1 => (Martin, 146.432, 4.862),
            Martin2 => (Martin, 73.216, 4.862),
            Sc2_180 => (Sc2, 235.0, 5.5437),
            Sc2_120 => (Sc2, 156.5, 5.52248),
            Sc2_60 => (Sc2, 78.128, 5.5006),
            _ => return None,
        })
    }

    /// Scan window `tw` (ms) for the wired RGB-sequential submodes; `None` otherwise.
    pub fn scottie_tw(mode: SstvMode) -> Option<f64> {
        rgb_params(mode).map(|(_, tw, _)| tw)
    }

    // The channel-tag flag bits the reference ORs into the porch/pixel frequency to select
    // per-channel TX gain in CSSTVMOD::Do. ref: Main.cpp LineSCT/LineMRT/LineSC2180.
    const TAG_R: u16 = 0x1000;
    const TAG_G: u16 = 0x2000;
    const TAG_B: u16 = 0x3000;

    fn pixels(out: &mut Vec<Symbol>, vals: impl Iterator<Item = u8>, tag: u16, dt: f64) {
        for v in vals {
            out.push(Symbol { freq_hz: color_to_freq(v) + tag, ms: dt });
        }
    }

    /// One Scottie scan line. ref: Main.cpp:6173 `LineSCT`:
    /// porch(G)·G·sep(B)·B·sync·sep(R)·R, pixels at `tw/320`.
    pub fn scottie_line(out: &mut Vec<Symbol>, row: &[Rgb; 320], tw: f64) {
        let dt = tw / 320.0;
        out.push(Symbol { freq_hz: 1500 + TAG_G, ms: 1.5 });
        pixels(out, row.iter().map(|p| p.g), TAG_G, dt);
        out.push(Symbol { freq_hz: 1500 + TAG_B, ms: 1.5 });
        pixels(out, row.iter().map(|p| p.b), TAG_B, dt);
        out.push(Symbol { freq_hz: 1200, ms: 9.0 });
        out.push(Symbol { freq_hz: 1500 + TAG_R, ms: 1.5 });
        pixels(out, row.iter().map(|p| p.r), TAG_R, dt);
    }

    /// One Martin scan line. ref: Main.cpp:6195 `LineMRT`:
    /// sync·porch(G)·G·porch(B)·B·porch(R)·R·porch, pixels at `tw/320`.
    pub fn martin_line(out: &mut Vec<Symbol>, row: &[Rgb; 320], tw: f64) {
        let dt = tw / 320.0;
        out.push(Symbol { freq_hz: 1200, ms: 4.862 });
        out.push(Symbol { freq_hz: 1500 + TAG_G, ms: 0.572 });
        pixels(out, row.iter().map(|p| p.g), TAG_G, dt);
        out.push(Symbol { freq_hz: 1500 + TAG_B, ms: 0.572 });
        pixels(out, row.iter().map(|p| p.b), TAG_B, dt);
        out.push(Symbol { freq_hz: 1500 + TAG_R, ms: 0.572 });
        pixels(out, row.iter().map(|p| p.r), TAG_R, dt);
        out.push(Symbol { freq_hz: 1500, ms: 0.572 });
    }

    /// One SC2 scan line. ref: Main.cpp:6218 `LineSC2180`:
    /// sync(S)·porch(R)·R·G·B (no inter-channel porches), pixels at `tw/320`.
    pub fn sc2_line(out: &mut Vec<Symbol>, row: &[Rgb; 320], s: f64, tw: f64) {
        let dt = tw / 320.0;
        out.push(Symbol { freq_hz: 1200, ms: s });
        out.push(Symbol { freq_hz: 1500 + TAG_R, ms: 0.5 });
        pixels(out, row.iter().map(|p| p.r), TAG_R, dt);
        pixels(out, row.iter().map(|p| p.g), TAG_G, dt);
        pixels(out, row.iter().map(|p| p.b), TAG_B, dt);
    }

    /// Full RGB-sequential transmission: VIS header, the Scottie leading 1200/9 sync (ref:
    /// Main.cpp:7124; Scottie only), then one scan line per image row. Bit-exact against the
    /// golden harness symbol digest.
    pub fn rgb_symbols(mode: SstvMode, rows: &[[Rgb; 320]]) -> Option<Vec<Symbol>> {
        let (fam, tw, s) = rgb_params(mode)?;
        let mut out = header(mode);
        if fam == RgbFamily::Scottie {
            out.push(Symbol { freq_hz: 1200, ms: 9.0 });
        }
        for row in rows {
            match fam {
                RgbFamily::Scottie => scottie_line(&mut out, row, tw),
                RgbFamily::Martin => martin_line(&mut out, row, tw),
                RgbFamily::Sc2 => sc2_line(&mut out, row, s, tw),
            }
        }
        Some(out)
    }

    /// FNV-1a over a symbol stream, each symbol serialized as `freq(i32 LE) ++ ms(f64 LE
    /// bits)`. Byte-identical to the harness `symbol_digest` (sstv_tx_dump.cxx) for a
    /// bit-exact TX gate.
    pub fn symbol_digest(syms: &[Symbol]) -> u64 {
        let mut h: u64 = 1469598103934665603;
        let byte = |b: u8, h: &mut u64| {
            *h ^= b as u64;
            *h = h.wrapping_mul(1099511628211);
        };
        for s in syms {
            for b in (s.freq_hz as i32).to_le_bytes() {
                byte(b, &mut h);
            }
            for b in s.ms.to_le_bytes() {
                byte(b, &mut h);
            }
        }
        h
    }
}

/// Fixed working sample rate for every SSTV submode. ref: Main.cpp:212 (`SampFreq=11025`).
pub const SAMPLE_RATE: u32 = 11025;

/// Audio rendering + FM demodulation (plan F1.1 + T4-audio). These are the **FP-tolerance**
/// stages (Doctrine §3): the tone renderer and discriminator are faithful ports of the
/// reference's VCO/PLL behaviour but are never asserted bit-exact against reference audio.
pub mod audio {
    use super::vis::Symbol;
    use super::SAMPLE_RATE;
    use crate::frontend::fir::{design_lowpass, Fir};
    use crate::frontend::nco::DownConverter;
    use crate::types::{Cplx, Sample};

    /// The scan/sync band spans 1200–2300 Hz; centre the discriminator at 1900 Hz (the
    /// reference PLL free band is 1500–2300, ref: sstv.cpp:1436). Only the low 12 bits of a
    /// symbol frequency are the tone — the upper nibble is a TX channel-gain tag masked off
    /// in `CSSTVMOD::Do` (`f & 0x0fff`, ref: sstv.cpp:2869).
    const DISC_CENTER_HZ: f32 = 1900.0;
    const TONE_MASK: u16 = 0x0fff;

    /// Render a symbol stream to PCM at [`SAMPLE_RATE`], accumulating fractional sample
    /// positions exactly as the reference does (`m_dPos += ms*fs/1000`, ref: sstv.cpp:2843)
    /// so line timing does not drift. A pure sine per tone (the FP-domain analogue of the
    /// reference VCO); edge shaping/BPF are spectral refinements that don't move the raster.
    pub fn render(symbols: &[Symbol], amplitude: f32) -> Vec<Sample> {
        let fs = SAMPLE_RATE as f64;
        let mut out = Vec::new();
        let mut d_pos = 0.0f64;
        let mut i_pos = 0i64;
        let mut phase = 0.0f64;
        for s in symbols {
            let f = (s.freq_hz & TONE_MASK) as f64;
            d_pos += s.ms * fs / 1000.0;
            let dphi = std::f64::consts::TAU * f / fs;
            while (i_pos as f64) < d_pos {
                out.push((amplitude as f64 * phase.sin()) as Sample);
                phase += dphi;
                if phase > std::f64::consts::TAU {
                    phase -= std::f64::consts::TAU;
                }
                i_pos += 1;
            }
        }
        out
    }

    /// FM discriminator: down-convert to complex baseband around [`DISC_CENTER_HZ`], lowpass
    /// to kill the sum-frequency image, and read instantaneous frequency from the phase
    /// increment `arg(z[n] · conj(z[n-1]))`. Output is the recovered tone frequency in Hz.
    pub struct Discriminator {
        dc: DownConverter,
        lpf_i: Fir,
        lpf_q: Fir,
        prev: Cplx,
        have_prev: bool,
    }

    impl Discriminator {
        pub fn new() -> Self {
            // Anti-image lowpass on the complex baseband: pass the ±700 Hz difference band,
            // reject the sum-frequency image (≈3000–4600 Hz). 31-tap Blackman sinc @ 1000 Hz.
            let taps = design_lowpass(31, 1000.0, SAMPLE_RATE as f32);
            Discriminator {
                dc: DownConverter::new(DISC_CENTER_HZ, SAMPLE_RATE as f32),
                lpf_i: Fir::new(taps.clone()),
                lpf_q: Fir::new(taps),
                prev: Cplx::new(0.0, 0.0),
                have_prev: false,
            }
        }

        /// Push one real sample, get the instantaneous tone frequency (Hz).
        pub fn push(&mut self, x: Sample) -> f32 {
            let b = self.dc.push(x);
            let z = Cplx::new(self.lpf_i.push(b.re), self.lpf_q.push(b.im));
            let freq = if self.have_prev {
                let d = z * self.prev.conj();
                let dphi = d.im.atan2(d.re);
                DISC_CENTER_HZ + dphi * SAMPLE_RATE as f32 / std::f32::consts::TAU
            } else {
                DISC_CENTER_HZ
            };
            self.prev = z;
            self.have_prev = true;
            freq
        }
    }

    impl Default for Discriminator {
        fn default() -> Self {
            Self::new()
        }
    }
}

/// RGB-sequential RX line reconstruction (plan T5) for the Scottie / Martin / SC2 families.
/// Runs the FM discriminator, locates the 1200 Hz line-sync pulses, and samples the R/G/B
/// channels at their per-family emission offsets from the sync (ref: TX layouts
/// Main.cpp:6173/6195/6218, RX geometry Main.cpp:3800-3855). These families have no
/// colour-difference math — each channel maps directly via the inverse of `ColorToFreq`.
/// Sync-anchored so the discriminator group delay cancels.
pub mod demod {
    use super::audio::Discriminator;
    use super::modulator::{rgb_params, Rgb, RgbFamily};
    use super::SAMPLE_RATE;

    /// Inverse of `ColorToFreq` (ref: ComLib.cpp:3491): scan frequency → 0–255 luminance.
    pub fn freq_to_value(freq_hz: f32) -> u8 {
        let v = ((freq_hz - 1500.0) * 256.0 / 800.0).round();
        v.clamp(0.0, 255.0) as u8
    }

    /// Run the discriminator over the whole PCM buffer.
    pub fn discriminate(pcm: &[f32]) -> Vec<f32> {
        let mut d = Discriminator::new();
        pcm.iter().map(|&x| d.push(x)).collect()
    }

    /// Centre-sample indices of sustained 1200 Hz sync pulses (freq well below the 1500 Hz
    /// black floor for at least `min_ms`). Picks up both VIS and per-line syncs; callers
    /// select the line syncs by position.
    pub fn find_sync_centers(freq: &[f32], min_ms: f64) -> Vec<usize> {
        let min_len = (SAMPLE_RATE as f64 * min_ms / 1000.0) as usize;
        let mut out = Vec::new();
        let mut run_start: Option<usize> = None;
        for (i, &f) in freq.iter().enumerate() {
            if f < 1350.0 {
                run_start.get_or_insert(i);
            } else if let Some(s) = run_start.take() {
                if i - s >= min_len {
                    out.push((s + i) / 2);
                }
            }
        }
        out
    }

    fn ms_to_samp(ms: f64) -> f64 {
        SAMPLE_RATE as f64 * ms / 1000.0
    }

    /// Sample one channel: 320 pixels across `[start_ms, start_ms+tw]` relative to the sync
    /// centre, reading the discriminator at each pixel's centre and mapping to luminance.
    fn sample_channel(freq: &[f32], sync_center: usize, start_ms: f64, tw_ms: f64) -> [u8; 320] {
        let base = sync_center as f64 + ms_to_samp(start_ms);
        let dt = ms_to_samp(tw_ms) / 320.0;
        let mut out = [0u8; 320];
        for (x, px) in out.iter_mut().enumerate() {
            let cf = base + (x as f64 + 0.5) * dt;
            let f = if cf < 0.0 {
                1500.0
            } else {
                freq.get(cf.round() as usize).copied().unwrap_or(1500.0)
            };
            *px = freq_to_value(f);
        }
        out
    }

    /// Total VIS-header duration (ms) for a mode, from its `vis::header` symbols.
    fn header_ms(mode: super::SstvMode) -> f64 {
        super::vis::header(mode).iter().map(|s| s.ms).sum()
    }

    fn nearest(syncs: &[usize], target: i64, win: i64) -> Option<usize> {
        syncs
            .iter()
            .copied()
            .filter(|&s| (s as i64 - target).abs() <= win)
            .min_by_key(|&s| (s as i64 - target).abs())
    }

    /// Per-family RX geometry: channel windows (start offset ms, relative to a line-sync
    /// centre), the line period, the first sync's position, and the minimum sync length to
    /// detect. Derived from the TX layout of each family.
    struct Layout {
        tw: f64,
        min_sync_ms: f64,
        period_ms: f64,
        first_sync_ms: f64,
        r_off: f64,
        g_off: f64,
        b_off: f64,
    }

    fn layout(mode: super::SstvMode) -> Option<Layout> {
        let (fam, tw, s) = rgb_params(mode)?;
        let header = header_ms(mode);
        Some(match fam {
            // Sync mid-line (9 ms); G,B precede it, R follows. Leading 9 ms sync after VIS.
            RgbFamily::Scottie => Layout {
                tw,
                min_sync_ms: 6.0,
                period_ms: 3.0 * tw + 13.5,
                first_sync_ms: header + 9.0 + 2.0 * tw + 7.5,
                g_off: -(2.0 * tw + 6.0),
                b_off: -(tw + 4.5),
                r_off: 6.0,
            },
            // Sync at line start (4.862 ms); porch 0.572; order G,B,R after the sync.
            RgbFamily::Martin => Layout {
                tw,
                min_sync_ms: 3.0,
                period_ms: 3.0 * tw + 7.15,
                first_sync_ms: header + 4.862 / 2.0,
                g_off: 3.003,
                b_off: tw + 3.575,
                r_off: 2.0 * tw + 4.147,
            },
            // Sync at line start (S ms); porch 0.5; order R,G,B with no inter-channel porch.
            RgbFamily::Sc2 => Layout {
                tw,
                min_sync_ms: 3.5,
                period_ms: 3.0 * tw + s + 0.5,
                first_sync_ms: header + s / 2.0,
                r_off: s / 2.0 + 0.5,
                g_off: s / 2.0 + 0.5 + tw,
                b_off: s / 2.0 + 0.5 + 2.0 * tw,
            },
        })
    }

    fn decode_line(freq: &[f32], sync_center: usize, lay: &Layout) -> [Rgb; 320] {
        let r = sample_channel(freq, sync_center, lay.r_off, lay.tw);
        let g = sample_channel(freq, sync_center, lay.g_off, lay.tw);
        let b = sample_channel(freq, sync_center, lay.b_off, lay.tw);
        let mut row = [Rgb { r: 0, g: 0, b: 0 }; 320];
        for (x, px) in row.iter_mut().enumerate() {
            *px = Rgb { r: r[x], g: g[x], b: b[x] };
        }
        row
    }

    // Discriminator FIR group delay in samples ((31-1)/2). The discriminator output lags the
    // audio by this much, so freq[] positions of true features sit this many samples later
    // than their audio-timeline predictions; add it to the prediction fallback.
    const DISC_GROUP_DELAY: i64 = 15;

    /// Decode a full RGB-sequential frame from PCM to a row-major RGB raster (3 bytes/pixel).
    /// Predicts each line sync from the header + line period and snaps to the nearest
    /// detected sync ("predict & refine", the standard robust SSTV alignment). Returns
    /// `(width, rgb)`; rows beyond the captured audio decode to black.
    pub fn decode_frame(pcm: &[f32], mode: super::SstvMode) -> Option<(u16, Vec<u8>)> {
        let lay = layout(mode)?;
        let geom = mode.geometry();
        let freq = discriminate(pcm);
        // Only consider line syncs in the image region: the VIS stop pulse (1200/30 ms) abuts
        // the first line sync for Martin/SC2, merging into one run whose centre is off — so
        // reject any detected sync before the header ends and let line 0 fall back to
        // prediction. ref: the VIS→image transition, Main.cpp:6975-7124.
        let header_end = ms_to_samp(header_ms(mode)) as i64;
        let all: Vec<usize> =
            find_sync_centers(&freq, lay.min_sync_ms).into_iter().filter(|&s| s as i64 >= header_end).collect();
        let period = ms_to_samp(lay.period_ms);
        let first = ms_to_samp(lay.first_sync_ms) as i64 + DISC_GROUP_DELAY;
        let win = (period * 0.3) as i64;
        let mut rgb = Vec::with_capacity(geom.width as usize * geom.scan_lines as usize * 3);
        for k in 0..geom.scan_lines as usize {
            let predicted = first + (k as f64 * period) as i64;
            let sc = nearest(&all, predicted, win).unwrap_or_else(|| predicted.max(0) as usize);
            for px in decode_line(&freq, sc, &lay) {
                rgb.push(px.r);
                rgb.push(px.g);
                rgb.push(px.b);
            }
        }
        Some((geom.width, rgb))
    }
}

/// The RGB-sequential SSTV families (Scottie `smSCT1/2/DX`, Martin `smMRT1/2`, SC2
/// `smSC2_180/120/60`) as omnimodem `Modulator` + `Demodulator`. TX takes a
/// `FramePayload::ImageRgb` picture and renders it; RX buffers the capture and emits the
/// reconstructed `ImageRgb` raster on flush (facsimile finalises at end-of-transmission,
/// like Hell). The colour-difference families (Robot/PD/MP/MR) get their own decode later.
pub mod rgb {
    use super::modulator::{rgb_params, rgb_symbols, Rgb};
    use super::{audio, demod, SstvMode, SAMPLE_RATE};
    use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
    use crate::types::{Frame, FrameMeta, FramePayload, Sample};

    fn caps() -> ModeCaps {
        ModeCaps {
            native_rate: SAMPLE_RATE,
            bandwidth_hz: 2300.0 - 1200.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    /// Resolve a label to an RGB-sequential submode, but only the wired ones
    /// (`scottie1/2/dx`, `martin1/2`, `sc2-180/120/60`) — so the daemon never
    /// exposes an unimplemented SSTV mode.
    pub fn from_label(s: &str) -> Option<SstvMode> {
        let m = SstvMode::from_label(s)?;
        rgb_params(m).map(|_| m)
    }

    /// Transmitter: an RGB picture → SSTV audio.
    pub struct RgbMod {
        mode: SstvMode,
    }
    impl RgbMod {
        pub fn new(mode: SstvMode) -> Self {
            RgbMod { mode }
        }
    }
    impl Modulator for RgbMod {
        fn caps(&self) -> ModeCaps {
            caps()
        }
        fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
            let (width, rgb) = match &frame.payload {
                FramePayload::ImageRgb { width, rgb } => (*width, rgb),
                _ => return Err(ModError::UnsupportedPayload("sstv needs an rgb image")),
            };
            if width != 320 {
                return Err(ModError::Encode(format!("sstv needs width 320, got {width}")));
            }
            let scan = self.mode.geometry().scan_lines as usize;
            let n = (rgb.len() / (320 * 3)).min(scan);
            let mut rows: Vec<[Rgb; 320]> = Vec::with_capacity(n);
            for r in 0..n {
                let mut row = [Rgb { r: 0, g: 0, b: 0 }; 320];
                for (x, px) in row.iter_mut().enumerate() {
                    let i = (r * 320 + x) * 3;
                    *px = Rgb { r: rgb[i], g: rgb[i + 1], b: rgb[i + 2] };
                }
                rows.push(row);
            }
            let syms = rgb_symbols(self.mode, &rows)
                .ok_or(ModError::Encode("not an RGB-sequential submode".into()))?;
            Ok(audio::render(&syms, 0.5))
        }
    }

    /// Receiver: buffers the capture, emits the reconstructed `ImageRgb` on flush.
    pub struct RgbDemod {
        mode: SstvMode,
        buf: Vec<Sample>,
    }
    impl RgbDemod {
        pub fn new(mode: SstvMode) -> Self {
            RgbDemod { mode, buf: Vec::new() }
        }
    }
    impl Demodulator for RgbDemod {
        fn caps(&self) -> ModeCaps {
            caps()
        }
        fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
            self.buf.extend_from_slice(samples);
            Vec::new()
        }
        fn reset(&mut self) {
            self.buf.clear();
        }
        fn flush(&mut self) -> Vec<Frame> {
            let out = demod::decode_frame(&self.buf, self.mode).map(|(width, rgb)| Frame {
                payload: FramePayload::ImageRgb { width, rgb },
                meta: FrameMeta { decoder: Some("sstv".into()), crc_ok: true, ..Default::default() },
            });
            self.buf.clear();
            out.into_iter().collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::audio::{render, Discriminator};
    use super::modulator::{color_to_freq, rgb_symbols, symbol_digest, Rgb};
    use super::vis::{header, Symbol};
    use super::*;

    // The harness test image: 8 vertical colour bars across 320 px (ref: sstv_tx_dump.cxx
    // kBars, packed 0x00BBGGRR). Reproduced here so the Rust modulator hashes the same input.
    fn colorbar_row() -> [Rgb; 320] {
        // kBars as (r,g,b): black,red,green,yellow,blue,magenta,cyan,white.
        const BARS: [Rgb; 8] = [
            Rgb { r: 0x00, g: 0x00, b: 0x00 },
            Rgb { r: 0xFF, g: 0x00, b: 0x00 },
            Rgb { r: 0x00, g: 0xFF, b: 0x00 },
            Rgb { r: 0xFF, g: 0xFF, b: 0x00 },
            Rgb { r: 0x00, g: 0x00, b: 0xFF },
            Rgb { r: 0xFF, g: 0x00, b: 0xFF },
            Rgb { r: 0x00, g: 0xFF, b: 0xFF },
            Rgb { r: 0xFF, g: 0xFF, b: 0xFF },
        ];
        let mut row = [Rgb { r: 0, g: 0, b: 0 }; 320];
        for (x, px) in row.iter_mut().enumerate() {
            *px = BARS[(x * 8) / 320];
        }
        row
    }

    #[test]
    fn all_modes_have_unique_labels_and_roundtrip() {
        let modes = SstvMode::all();
        assert_eq!(modes.len(), 43, "SSTVModeList has 43 submodes (ref: sstv.cpp:493-503, smEND=43)");
        let mut labels = std::collections::HashSet::new();
        for &m in modes {
            assert!(labels.insert(m.label()), "duplicate label {}", m.label());
            assert_eq!(SstvMode::from_label(m.label()), Some(m));
        }
    }

    #[test]
    fn vis_codes_are_unique_per_form() {
        // No two modes share a VIS identifier (would make RX ambiguous).
        let mut seen = std::collections::HashSet::new();
        for &m in SstvMode::all() {
            let key = format!("{:?}", m.vis());
            assert!(seen.insert(key), "duplicate VIS for {}", m.label());
        }
    }

    #[test]
    fn scottie1_vis_matches_reference() {
        // ref: Main.cpp:6997 (smSCT1 → 0x3c).
        assert_eq!(SstvMode::Scottie1.vis(), Vis::Standard(0x3c));
    }

    #[test]
    fn scottie1_geometry_matches_reference() {
        assert_eq!(
            SstvMode::Scottie1.geometry(),
            Geometry { width: 320, rows: 256, scan_lines: 256 }
        );
    }

    /// The header symbol sequence must be byte-for-byte the leader + VIS bits the reference
    /// emits — verified against the golden TX vector `sstv_scottie1_tx.json`, whose first 13
    /// `vis_symbols` are exactly this header (the 14th onward is line data).
    #[test]
    fn scottie1_header_matches_golden_vector() {
        let h = header(SstvMode::Scottie1);
        let want = [
            Symbol { freq_hz: 1900, ms: 300.0 },
            Symbol { freq_hz: 1200, ms: 10.0 },
            Symbol { freq_hz: 1900, ms: 300.0 },
            Symbol { freq_hz: 1200, ms: 30.0 },
            // VIS 0x3c = 0b00111100, LSB-first: 0,0,1,1,1,1,0,0
            Symbol { freq_hz: 1300, ms: 30.0 },
            Symbol { freq_hz: 1300, ms: 30.0 },
            Symbol { freq_hz: 1100, ms: 30.0 },
            Symbol { freq_hz: 1100, ms: 30.0 },
            Symbol { freq_hz: 1100, ms: 30.0 },
            Symbol { freq_hz: 1100, ms: 30.0 },
            Symbol { freq_hz: 1300, ms: 30.0 },
            Symbol { freq_hz: 1300, ms: 30.0 },
            Symbol { freq_hz: 1200, ms: 30.0 }, // stop
        ];
        assert_eq!(h, want);
    }

    #[test]
    fn extended_header_is_16_bit() {
        // MR73 → 0x4523; 16 bits between leader (4) and stop (1) → 21 symbols total.
        let h = header(SstvMode::Mr73);
        assert_eq!(h.len(), 4 + 16 + 1);
        // Low byte 0x23 marks extended VIS; bit 0 (LSB) is 1 → first VIS bit is 1100 Hz.
        assert_eq!(h[4].freq_hz, 1100);
    }

    #[test]
    fn narrow_header_is_nvis_fsk() {
        // MP73-N: N-VIS D2=0x02; frame D0=101101 D1=010101 D2=000010 D3=010111
        // (ref: mode.txt §7 example "101101 010101 000010 010111").
        let h = header(SstvMode::Mn73);
        // preamble(2) + start(1) + 24 bits
        assert_eq!(h.len(), 3 + 24);
        assert_eq!(h[0].freq_hz, 1900);
        assert_eq!(h[1].freq_hz, 2100);
        // D2 = 000010 (MSB first): 0,0,0,0,1,0 → tones 2100,2100,2100,2100,1900,2100
        let d2 = &h[3 + 12..3 + 18];
        let d2_tones: Vec<u16> = d2.iter().map(|s| s.freq_hz).collect();
        assert_eq!(d2_tones, vec![2100, 2100, 2100, 2100, 1900, 2100]);
    }

    #[test]
    fn color_to_freq_matches_reference() {
        // ref: ComLib.cpp:3491. Black→1500, white→2296 (255*800/256+1500), mid→1898.
        assert_eq!(color_to_freq(0), 1500);
        assert_eq!(color_to_freq(255), 2296);
        assert_eq!(color_to_freq(128), 1900);
    }

    /// The decisive bit-exact TX gate: the Scottie 1 modulator's FULL symbol stream (VIS +
    /// leading sync + 4 colour-bar lines) must hash identically to the golden vector produced
    /// by the isolated MMSSTV harness (`sstv_scottie1_tx.json`, `symbol_fnv1a`).
    #[test]
    fn scottie1_full_symbol_stream_matches_golden_digest() {
        let row = colorbar_row();
        let rows = [row; 4]; // harness nlines = 4
        let syms = rgb_symbols(SstvMode::Scottie1, &rows).unwrap();

        // Structure: VIS(13) + leading sync(1) + 4*(964) = 3870 (harness symbol_count).
        assert_eq!(syms.len(), 3870);

        // Bit-exact digest, matching the harness value in sstv_scottie1_tx.json.
        const GOLDEN_SYMBOL_FNV1A: u64 = 0x812e72b7fb4fbac1;
        assert_eq!(symbol_digest(&syms), GOLDEN_SYMBOL_FNV1A);
    }

    #[test]
    fn scottie_line_channel_order_and_tags() {
        // Sanity on the transcribed layout: after the 13-symbol VIS header and the leading
        // 1200/9 sync, the first line's first symbol is the green porch (1500+0x2000), and the
        // 1200/9 sync precedes the red channel.
        let rows = [colorbar_row(); 1];
        let syms = rgb_symbols(SstvMode::Scottie1, &rows).unwrap();
        let line0 = &syms[14..]; // 13-symbol VIS header + 1 leading sync
        assert_eq!(line0[0], Symbol { freq_hz: 1500 + 0x2000, ms: 1.5 });
        // porch + 320 G + sepB + 320 B = index 642 is the 1200/9 sync.
        assert_eq!(line0[1 + 320 + 1 + 320], Symbol { freq_hz: 1200, ms: 9.0 });
    }

    /// FP round-trip of the audio primitives (plan F1.1): render known held tones, run the
    /// FM discriminator, and confirm the recovered frequency settles near each tone. Proves
    /// the symbol→PCM renderer and the discriminator agree end-to-end.
    #[test]
    fn audio_render_then_discriminate_recovers_tones() {
        use super::vis::Symbol;
        // Black (1500), sync (1200), mid-grey (1900), white (2300), each held 20 ms.
        let syms = [
            Symbol { freq_hz: 1500, ms: 20.0 },
            Symbol { freq_hz: 1200, ms: 20.0 },
            Symbol { freq_hz: 1900, ms: 20.0 },
            Symbol { freq_hz: 2300, ms: 20.0 },
        ];
        let pcm = render(&syms, 16000.0);
        let mut disc = Discriminator::new();
        let freqs: Vec<f32> = pcm.iter().map(|&x| disc.push(x)).collect();

        let sps = (SAMPLE_RATE as f64 * 0.020) as usize; // samples per 20 ms tone
                                                         // Sample the steady-state middle of each tone (skip filter/discriminator settling).
        for (i, &want) in [1500.0f32, 1200.0, 1900.0, 2300.0].iter().enumerate() {
            let mid = i * sps + sps / 2;
            let got = freqs[mid];
            assert!(
                (got - want).abs() < 25.0,
                "tone {}: wanted {want} Hz, discriminator gave {got} Hz",
                i
            );
        }
    }

    /// End-to-end loopback (plan T5) for an RGB-sequential family: render `n_rows` colour-bar
    /// lines to audio, recover the full frame through the real RX chain (discriminate → find
    /// sync → sample R/G/B), and check the 8 bar centres on each decoded row. No TX timing is
    /// shared into the decoder — it re-derives line positions from the header + line period.
    fn assert_family_loopback(mode: SstvMode, n_rows: usize) {
        let src = colorbar_row();
        let rows = vec![src; n_rows];
        let syms = rgb_symbols(mode, &rows).unwrap();
        let pcm = render(&syms, 16000.0);

        let (w, out) = super::demod::decode_frame(&pcm, mode).unwrap();
        assert_eq!(w, 320);
        let ok = |g: u8, e: u8| if e >= 128 { g >= 160 } else { g <= 95 };
        for row in 0..n_rows {
            for bar in 0..8 {
                let x = bar * 40 + 20;
                let i = (row * 320 + x) * 3;
                let (gr, gg, gb) = (out[i], out[i + 1], out[i + 2]);
                let (er, eg, eb) = (src[x].r, src[x].g, src[x].b);
                assert!(
                    ok(gr, er) && ok(gg, eg) && ok(gb, eb),
                    "{}: row {row} bar {bar}: got ({gr},{gg},{gb}) want ~({er},{eg},{eb})",
                    mode.label()
                );
            }
        }
    }

    #[test]
    fn scottie_audio_loopback_recovers_colorbars() {
        assert_family_loopback(SstvMode::Scottie1, 8);
        assert_family_loopback(SstvMode::Scottie2, 8);
    }

    #[test]
    fn martin_audio_loopback_recovers_colorbars() {
        assert_family_loopback(SstvMode::Martin1, 8);
        assert_family_loopback(SstvMode::Martin2, 8);
    }

    #[test]
    fn sc2_audio_loopback_recovers_colorbars() {
        assert_family_loopback(SstvMode::Sc2_180, 8);
        assert_family_loopback(SstvMode::Sc2_60, 8);
    }

    /// Full trait round-trip (T5 emission + the new `ImageRgb` payload): build a colour-bar
    /// `ImageRgb`, run it through the Scottie `Modulator` → `Demodulator`, and confirm the
    /// recovered raster is the right shape and its bar centres match. Exercises the colour
    /// payload both ways end-to-end.
    #[test]
    fn scottie_modulator_demodulator_imagergb_roundtrip() {
        use super::rgb::{RgbDemod, RgbMod};
        use crate::mode::{Demodulator, Modulator};
        use crate::types::{Frame, FrameMeta, FramePayload};

        // 8-row colour-bar picture (each row identical; enough to prove multi-row assembly).
        let src = colorbar_row();
        let n_rows = 8usize;
        let mut rgb = Vec::with_capacity(320 * n_rows * 3);
        for _ in 0..n_rows {
            for px in &src {
                rgb.extend_from_slice(&[px.r, px.g, px.b]);
            }
        }
        let frame = Frame {
            payload: FramePayload::ImageRgb { width: 320, rgb },
            meta: FrameMeta::default(),
        };

        let pcm = RgbMod::new(SstvMode::Scottie1).modulate(&frame).unwrap();

        let mut demod = RgbDemod::new(SstvMode::Scottie1);
        assert!(demod.feed(&pcm).is_empty(), "facsimile emits on flush, not feed");
        let frames = demod.flush();
        assert_eq!(frames.len(), 1);

        let (w, out) = match &frames[0].payload {
            FramePayload::ImageRgb { width, rgb } => (*width, rgb),
            other => panic!("expected ImageRgb, got {other:?}"),
        };
        assert_eq!(w, 320);
        // Scottie 1 is 320x256; rows past the 8 captured decode to black but the buffer is full-size.
        assert_eq!(out.len(), 320 * 256 * 3);

        // Verify the 8 real rows: each bar centre matches the transmitted colour.
        let ok = |g: u8, e: u8| if e >= 128 { g >= 160 } else { g <= 95 };
        for row in 0..n_rows {
            for bar in 0..8 {
                let x = bar * 40 + 20;
                let i = (row * 320 + x) * 3;
                let (gr, gg, gb) = (out[i], out[i + 1], out[i + 2]);
                let (er, eg, eb) = (src[x].r, src[x].g, src[x].b);
                assert!(
                    ok(gr, er) && ok(gg, eg) && ok(gb, eb),
                    "row {row} bar {bar}: got ({gr},{gg},{gb}) want ~({er},{eg},{eb})"
                );
            }
        }
    }

    #[test]
    fn rendered_pcm_length_tracks_symbol_durations() {
        use super::vis::Symbol;
        // 100 ms total at 11025 Hz ≈ 1102 samples (fractional accumulation, no drift).
        let pcm = render(&[Symbol { freq_hz: 1900, ms: 100.0 }], 10000.0);
        assert!((pcm.len() as i64 - 1102).abs() <= 1, "got {} samples", pcm.len());
    }

    #[test]
    fn decode_bits_recovers_mode() {
        // Round-trip a standard and an extended VIS through decode_bits.
        for &m in &[SstvMode::Scottie1, SstvMode::Robot36, SstvMode::Pd90] {
            let byte = match m.vis() { Vis::Standard(b) => b, _ => unreachable!() };
            let tones: Vec<u16> = (0..8).map(|i| if (byte >> i) & 1 != 0 { 1100 } else { 1300 }).collect();
            assert_eq!(super::vis::decode_bits(&tones, false), Some(m));
        }
        let word = match SstvMode::Mr73.vis() { Vis::Extended(w) => w, _ => unreachable!() };
        let tones: Vec<u16> = (0..16).map(|i| if (word >> i) & 1 != 0 { 1100 } else { 1300 }).collect();
        assert_eq!(super::vis::decode_bits(&tones, true), Some(SstvMode::Mr73));
    }
}
