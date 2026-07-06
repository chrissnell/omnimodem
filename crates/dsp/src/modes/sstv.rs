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

    /// Per-line total scan window `tw` (ms) for the Scottie submodes. ref: Main.cpp:6620-6626
    /// (`LineSCT(mp, 138.24 / 88.064 / 345.6)`).
    pub fn scottie_tw(mode: SstvMode) -> Option<f64> {
        use SstvMode::*;
        match mode {
            Scottie1 => Some(138.24),
            Scottie2 => Some(88.064),
            ScottieDx => Some(345.6),
            _ => None,
        }
    }

    // The channel-tag flag bits the reference ORs into the porch/pixel frequency to select
    // per-channel TX gain in CSSTVMOD::Do. ref: Main.cpp:6173 LineSCT.
    const TAG_R: u16 = 0x1000;
    const TAG_G: u16 = 0x2000;
    const TAG_B: u16 = 0x3000;

    /// One Scottie scan line for a 320-pixel row. ref: Main.cpp:6173 `TMmsstv::LineSCT`:
    /// porch(G) · G · sep(B) · B · sync · sep(R) · R, channels tagged, pixels at `tw/320`.
    pub fn scottie_line(out: &mut Vec<Symbol>, row: &[Rgb; 320], tw: f64) {
        let dt = tw / 320.0;
        out.push(Symbol { freq_hz: 1500 + TAG_G, ms: 1.5 });
        for p in row.iter() {
            out.push(Symbol { freq_hz: color_to_freq(p.g) + TAG_G, ms: dt });
        }
        out.push(Symbol { freq_hz: 1500 + TAG_B, ms: 1.5 });
        for p in row.iter() {
            out.push(Symbol { freq_hz: color_to_freq(p.b) + TAG_B, ms: dt });
        }
        out.push(Symbol { freq_hz: 1200, ms: 9.0 });
        out.push(Symbol { freq_hz: 1500 + TAG_R, ms: 1.5 });
        for p in row.iter() {
            out.push(Symbol { freq_hz: color_to_freq(p.r) + TAG_R, ms: dt });
        }
    }

    /// Full Scottie transmission: VIS header, then one `scottie_line` per image row, then the
    /// Scottie leading 1200 Hz/9 ms sync (ref: Main.cpp:7124). `rows` supplies each scan
    /// line's 320 pixels. Bit-exact against the golden harness symbol digest.
    pub fn scottie_symbols(mode: SstvMode, rows: &[[Rgb; 320]]) -> Option<Vec<Symbol>> {
        let tw = scottie_tw(mode)?;
        let mut out = header(mode);
        for row in rows {
            scottie_line(&mut out, row, tw);
        }
        out.push(Symbol { freq_hz: 1200, ms: 9.0 });
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

/// Scottie RX line reconstruction (plan T5, RGB-sequential family). Runs the FM
/// discriminator, locates the 1200 Hz line-sync pulses, and samples the R/G/B channels at
/// their emission offsets from the sync (ref: RX geometry Main.cpp:3800-3855; TX layout
/// Main.cpp:6173). Scottie has no colour-difference math — each channel maps directly via
/// the inverse of `ColorToFreq`. Sync-anchored so the discriminator group delay cancels.
pub mod demod_scottie {
    use super::audio::Discriminator;
    use super::modulator::Rgb;
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
            let c = (base + (x as f64 + 0.5) * dt).round() as usize;
            let f = freq.get(c).copied().unwrap_or(1500.0);
            *px = freq_to_value(f);
        }
        out
    }

    /// Reconstruct one Scottie image row anchored on a line-sync centre. Channel windows
    /// (relative to sync end at +4.5 ms): R at +1.5 ms, then next porch+G, then porch+B —
    /// each `tw` ms wide (the emission order of `LineSCT`).
    pub fn decode_line(freq: &[f32], sync_center: usize, tw_ms: f64) -> [Rgb; 320] {
        let sync_half = 4.5; // half the 9 ms sync
        let r = sample_channel(freq, sync_center, sync_half + 1.5, tw_ms);
        let g = sample_channel(freq, sync_center, sync_half + 1.5 + tw_ms + 1.5, tw_ms);
        let b = sample_channel(freq, sync_center, sync_half + 1.5 + 2.0 * tw_ms + 3.0, tw_ms);
        let mut row = [Rgb { r: 0, g: 0, b: 0 }; 320];
        for (x, px) in row.iter_mut().enumerate() {
            *px = Rgb { r: r[x], g: g[x], b: b[x] };
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::audio::{render, Discriminator};
    use super::modulator::{color_to_freq, scottie_symbols, symbol_digest, Rgb};
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
    /// 4 colour-bar lines + trailing sync) must hash identically to the golden vector
    /// produced by the isolated MMSSTV harness (`sstv_scottie1_tx.json`, `symbol_fnv1a`).
    #[test]
    fn scottie1_full_symbol_stream_matches_golden_digest() {
        let row = colorbar_row();
        let rows = [row; 4]; // harness nlines = 4
        let syms = scottie_symbols(SstvMode::Scottie1, &rows).unwrap();

        // Structure: VIS(13) + 4*(964) + trailing sync(1) = 3870 (harness symbol_count).
        assert_eq!(syms.len(), 3870);

        // Bit-exact digest, matching the harness value in sstv_scottie1_tx.json.
        const GOLDEN_SYMBOL_FNV1A: u64 = 0x2f8bedaff9db0041;
        assert_eq!(symbol_digest(&syms), GOLDEN_SYMBOL_FNV1A);
    }

    #[test]
    fn scottie_line_channel_order_and_tags() {
        // Sanity on the transcribed layout: first line symbol is the green porch (1500+0x2000),
        // and the sync pulse (1200/9) precedes the red channel.
        let rows = [colorbar_row(); 1];
        let syms = scottie_symbols(SstvMode::Scottie1, &rows).unwrap();
        let line0 = &syms[13..]; // after the 13-symbol VIS header
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

    /// End-to-end loopback (plan T5): render a Scottie 1 colour-bar image to audio, then
    /// recover it through the real RX chain (discriminate → find sync → sample R/G/B). The
    /// reconstructed bar centres must match the transmitted colours. This exercises the
    /// audio + sync + pixel-mapping path with no TX timing shared into the decoder.
    #[test]
    fn scottie1_audio_loopback_recovers_colorbars() {
        use super::demod_scottie::{decode_line, discriminate, find_sync_centers};
        let src = colorbar_row();
        let rows = [src; 6];
        let syms = scottie_symbols(SstvMode::Scottie1, &rows).unwrap();
        let pcm = render(&syms, 16000.0);

        let freq = discriminate(&pcm);
        // Per-line 1200 Hz sync is 9 ms; require ≥6 ms to reject VIS's short breaks.
        let syncs = find_sync_centers(&freq, 6.0);
        assert!(syncs.len() >= 5, "expected per-line syncs, got {}", syncs.len());

        // Decode a line from a mid-stream sync (well past the VIS header).
        let sc = syncs[syncs.len() / 2];
        let got = decode_line(&freq, sc, 138.24);

        // Check the 8 bar centres (avoid edge smear from the discriminator's group delay).
        // Colour bars are identical on every line, so any line reconstructs the same row.
        for bar in 0..8 {
            let x = bar * 40 + 20;
            let (gr, gg, gb) = (got[x].r, got[x].g, got[x].b);
            let (er, eg, eb) = (src[x].r, src[x].g, src[x].b);
            let ok = |g: u8, e: u8| if e >= 128 { g >= 160 } else { g <= 95 };
            assert!(
                ok(gr, er) && ok(gg, eg) && ok(gb, eb),
                "bar {bar} (x={x}): got ({gr},{gg},{gb}) want ~({er},{eg},{eb})"
            );
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
