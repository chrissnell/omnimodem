//! WEFAX-576 / WEFAX-288: HF weather facsimile — an FM-modulated grayscale
//! raster at Index-Of-Cooperation 576 or 288.
//!
//! Port of fldigi's WEFAX modem (`fldigi/src/wefax/wefax.cxx`, upstream
//! w1hkj/fldigi 4.1.23 @ `61b97f413`). WEFAX is a *facsimile* mode: a scan line
//! is a stream of gray pixels whose luminance is carried by the FM carrier's
//! instantaneous frequency, so the RX output is a raster (`FramePayload::Image`,
//! the Phase-10 shape), never characters.
//!
//! **Geometry** (wefax.cxx:258-265, 1251-1265): image width = `int(IOC·π)`
//! (576 → 1809 px, 288 → 904 px); WEFAX-576 runs at 120 LPM with a 300 Hz APT
//! start tone, WEFAX-288 at 60 LPM with 675 Hz; APT stop is 450 Hz. Samples per
//! line = `rate·60/LPM`.
//!
//! **FM mapping** (wefax.cxx:1959, 1986): TX `freq = carrier + 2·(v−0.5)·dev`,
//! `v = pixel/255` → black(0) = carrier−dev, white(1) = carrier+dev. RX recovers
//! `pixel = clamp(round(255·(0.5 + Δφ·(rate/shift)/2π)))`. (fldigi mixes with
//! `e^{+jθ}`, inverting the baseband sign, so its formula carries a leading `−`;
//! this port uses the standard `e^{−jθ}` down-converter, giving the `+` above —
//! same pixels, and the transmitted audio is identical since the pixel→freq map
//! is unchanged.)
//!
//! **Sync** (wefax.cxx:1650-1732): each phasing line is 2.5% white / 95% black /
//! 2.5% white, so a ~5% white band straddles every line boundary. The RX finds
//! those white-band centers, locks the line grid, assembles the raster, and
//! trims the constant APT/phasing/black margins.
//!
//! The audio is FP (Doctrine §3): the gate is a loopback that recovers the pixel
//! raster within an FM tolerance, plus closed-form geometry/round-trip unit
//! tests; cross-decode vs the fldigi binary is `#[ignore]`-gated. Operational
//! padding (APT/black durations) is shortened from fldigi's 5/5/10 s; every
//! wire-defining parameter (IOC, LPM, carrier/shift, pixel↔freq, phasing-line
//! format) matches the reference.

use crate::frontend::detector::FmDiscriminator;
use crate::frontend::fir::{design_lowpass, Fir};
use crate::frontend::nco::DownConverter;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Cplx, Frame, FrameMeta, FramePayload, Sample};
use std::f32::consts::TAU;

/// Native working rate (fldigi runs WEFAX at 11025 Hz, wefax.cxx:2256).
pub const WEFAX_RATE: u32 = 11_025;
/// Carrier / shift / deviation (wefax.cxx:267-268; `WEFAX_Center`/`WEFAX_Shift`
/// defaults). Standard 800 Hz shift → 400 Hz deviation; black = 1500 Hz, white =
/// 2300 Hz around a 1900 Hz carrier.
pub const CARRIER_HZ: f32 = 1900.0;
pub const SHIFT_HZ: f32 = 800.0;
pub const DEVIATION_HZ: f32 = SHIFT_HZ / 2.0;
/// APT stop tone (wefax.cxx:1237).
pub const APT_STOP_HZ: f32 = 450.0;

/// Index-of-correlation → image width in pixels. fldigi's `ioc_to_width`
/// returns `ioc * M_PI` as an `int`, i.e. C++ truncation toward zero
/// (576 → 1809, 288 → 904). ref: wefax.cxx:262-265.
pub fn ioc_to_width(ioc: u32) -> u16 {
    (ioc as f64 * std::f64::consts::PI) as u16
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WefaxVariant {
    /// IOC 576, 120 LPM, 300 Hz APT start.
    Wefax576,
    /// IOC 288, 60 LPM, 675 Hz APT start.
    Wefax288,
}

impl WefaxVariant {
    pub fn from_label(s: &str) -> Option<Self> {
        match s {
            "wefax576" => Some(WefaxVariant::Wefax576),
            "wefax288" => Some(WefaxVariant::Wefax288),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            WefaxVariant::Wefax576 => "wefax576",
            WefaxVariant::Wefax288 => "wefax288",
        }
    }
    pub fn all() -> &'static [WefaxVariant] {
        &[WefaxVariant::Wefax576, WefaxVariant::Wefax288]
    }
    /// (IOC, LPM, APT-start Hz). ref: wefax.cxx:1251-1263.
    fn params(self) -> (u32, f32, f32) {
        match self {
            WefaxVariant::Wefax576 => (576, 120.0, 300.0),
            WefaxVariant::Wefax288 => (288, 60.0, 675.0),
        }
    }
    pub fn width(self) -> u16 {
        ioc_to_width(self.params().0)
    }
    /// Samples per scan line at `rate` (`rate·60/LPM`, wefax.cxx:460-462).
    fn samples_per_line(self, rate: f32) -> f32 {
        rate * 60.0 / self.params().1
    }
}

// Operational padding (shortened from fldigi's 5/5/10 s — not wire-defining).
const APT_START_SECS: f32 = 2.0;
const APT_STOP_SECS: f32 = 1.0;
const BLACK_SECS: f32 = 0.5;
/// Phasing lines before the image (fldigi's `m_tx_phasing_lin`, wefax.cxx:1239).
const TX_PHASING_LINES: usize = 20;

/// One pixel's TX baseband value `v ∈ [0,1]` → absolute frequency around
/// `carrier` (black = carrier−dev, white = carrier+dev). ref: wefax.cxx:1986.
#[inline]
fn pixel_to_freq(carrier: f32, v: f32) -> f32 {
    carrier + 2.0 * (v - 0.5) * DEVIATION_HZ
}

pub struct WefaxMod {
    variant: WefaxVariant,
    center_hz: f32,
}

impl WefaxMod {
    pub fn new(variant: WefaxVariant, center_hz: f32) -> Self {
        WefaxMod { variant, center_hz }
    }

    fn tone(&self, out: &mut Vec<Sample>, phase: &mut f32, freq: f32, n: usize) {
        let dp = TAU * freq / WEFAX_RATE as f32;
        for _ in 0..n {
            *phase += dp;
            if *phase > TAU {
                *phase -= TAU;
            }
            out.push(0.9 * phase.sin());
        }
    }

    /// Append one scan line: `values[i] ∈ [0,1]` stretched to `spl` samples.
    fn line(&self, out: &mut Vec<Sample>, phase: &mut f32, values: &[f32], spl: f32) {
        let n = spl.round() as usize;
        for s in 0..n {
            let src = ((s as f32 / spl) * values.len() as f32) as usize;
            let v = values[src.min(values.len() - 1)];
            let dp = TAU * pixel_to_freq(self.center_hz, v) / WEFAX_RATE as f32;
            *phase += dp;
            if *phase > TAU {
                *phase -= TAU;
            }
            out.push(0.9 * phase.sin());
        }
    }
}

impl Modulator for WefaxMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: WEFAX_RATE,
            bandwidth_hz: SHIFT_HZ + 200.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let (width, gray) = match &frame.payload {
            FramePayload::Image { width, gray } => (*width as usize, gray),
            _ => return Err(ModError::UnsupportedPayload("wefax needs an image")),
        };
        if width == 0 || gray.is_empty() {
            return Ok(Vec::new());
        }
        let (_, _, apt_start) = self.variant.params();
        let spl = self.variant.samples_per_line(WEFAX_RATE as f32);
        let out_w = self.variant.width() as usize;
        let rows = gray.len() / width;

        let mut out = Vec::new();
        let mut phase = 0.0f32;

        // APT start tone.
        self.tone(&mut out, &mut phase, apt_start, (APT_START_SECS * WEFAX_RATE as f32) as usize);

        // Phasing lines: 2.5% white / 95% black / 2.5% white.
        let mut phasing = vec![0.0f32; out_w];
        for (c, p) in phasing.iter_mut().enumerate() {
            let pos = c as f32 / out_w as f32;
            *p = if !(0.025..0.975).contains(&pos) { 1.0 } else { 0.0 };
        }
        for _ in 0..TX_PHASING_LINES {
            self.line(&mut out, &mut phase, &phasing, spl);
        }
        // One all-white line marks the phasing→image transition.
        self.line(&mut out, &mut phase, &vec![1.0; out_w], spl);

        // Image lines: stretch each source row to the mode width, pixel→freq.
        for r in 0..rows {
            let row: Vec<f32> = (0..out_w)
                .map(|c| {
                    let src = c * width / out_w;
                    gray[r * width + src.min(width - 1)] as f32 / 255.0
                })
                .collect();
            self.line(&mut out, &mut phase, &row, spl);
        }

        // APT stop tone, then black.
        self.tone(&mut out, &mut phase, APT_STOP_HZ, (APT_STOP_SECS * WEFAX_RATE as f32) as usize);
        self.line(
            &mut out,
            &mut phase,
            &[0.0f32],
            BLACK_SECS * WEFAX_RATE as f32,
        );
        Ok(out)
    }
}

/// White-band threshold for phasing detection (wefax.cxx:1681, `x > 188`).
const WHITE_THRESH: u8 = 188;

/// Channel low-pass taps / cutoff: pass the image band (carrier ± deviation)
/// while rejecting the 2·carrier image the real→complex mix leaves behind.
const LPF_TAPS: usize = 63;
const LPF_CUTOFF_HZ: f32 = 700.0;
/// Group delay of the channel filter (samples), skipped when resampling lines.
#[cfg(test)]
const FILTER_DELAY: f32 = (LPF_TAPS as f32 - 1.0) / 2.0;

pub struct WefaxDemod {
    variant: WefaxVariant,
    center_hz: f32,
    nco: DownConverter,
    lpf_i: Fir,
    lpf_q: Fir,
    disc: FmDiscriminator,
    /// `(rate/shift)/2π` — scales phase advance to a pixel fraction.
    deviation_ratio: f32,
    pixels: Vec<u8>,
}

impl WefaxDemod {
    pub fn new(variant: WefaxVariant, center_hz: f32) -> Self {
        let rate = WEFAX_RATE as f32;
        let taps = design_lowpass(LPF_TAPS, LPF_CUTOFF_HZ, rate);
        WefaxDemod {
            variant,
            center_hz,
            nco: DownConverter::new(center_hz, rate),
            lpf_i: Fir::new(taps.clone()),
            lpf_q: Fir::new(taps),
            disc: FmDiscriminator::new(),
            deviation_ratio: (rate / SHIFT_HZ) / TAU,
            pixels: Vec::new(),
        }
    }

    /// FM-demodulate one sample to a 0..255 gray pixel (wefax.cxx:1959): the
    /// down-converted, channel-filtered baseband's phase advance sets the pixel.
    fn demod(&mut self, x: Sample) -> u8 {
        let bb = self.nco.push(x);
        let bb = Cplx::new(self.lpf_i.push(bb.re), self.lpf_q.push(bb.im));
        let dphi = self.disc.push(bb);
        let frac = 0.5 + self.deviation_ratio * dphi;
        (frac * 255.0).round().clamp(0.0, 255.0) as u8
    }

    /// Resample one scan line (`pixels[start..]`, `spl` samples wide) to `width`
    /// column averages (wefax.cxx:1801-1820, per-column mean).
    fn resample_line(pixels: &[u8], start: f32, spl: f32, width: usize) -> Vec<u8> {
        (0..width)
            .map(|c| {
                let a = (start + c as f32 * spl / width as f32).round() as usize;
                let b = (start + (c + 1) as f32 * spl / width as f32).round() as usize;
                let a = a.min(pixels.len());
                let b = b.clamp(a + 1, pixels.len().max(a + 1));
                let slice = &pixels[a..b.min(pixels.len()).max(a)];
                if slice.is_empty() {
                    0
                } else {
                    (slice.iter().map(|&p| p as u32).sum::<u32>() / slice.len() as u32) as u8
                }
            })
            .collect()
    }

    /// Find the line-grid origin from the phasing white bands. Returns the
    /// sample index of a line boundary, or `None` if no phasing lock.
    fn phasing_origin(&self, spl: f32) -> Option<f32> {
        // White-band centers: runs of white pixels spanning ~5% of a line.
        let min_run = (0.015 * spl) as usize;
        let max_run = (0.25 * spl) as usize;
        let mut centers = Vec::new();
        let mut run_start: Option<usize> = None;
        for (i, &p) in self.pixels.iter().enumerate() {
            if p > WHITE_THRESH {
                run_start.get_or_insert(i);
            } else if let Some(s) = run_start.take() {
                let len = i - s;
                if len >= min_run && len <= max_run {
                    centers.push((s + i) / 2);
                }
            }
        }
        // Best line-grid: the center from which the most others land on
        // `center + k·spl` (within tolerance) — the phasing line boundaries.
        let tol = 0.15 * spl;
        let mut best = (0usize, 0f32);
        for &c0 in &centers {
            let hits = centers
                .iter()
                .filter(|&&c| {
                    let k = ((c as f32 - c0 as f32) / spl).round();
                    k >= 0.0 && ((c as f32 - c0 as f32) - k * spl).abs() < tol
                })
                .count();
            if hits > best.0 {
                best = (hits, c0 as f32);
            }
        }
        if best.0 >= 4 {
            Some(best.1)
        } else {
            None
        }
    }

    fn assemble(&self) -> Option<Frame> {
        let width = self.variant.width() as usize;
        let spl = self.variant.samples_per_line(WEFAX_RATE as f32);
        if (self.pixels.len() as f32) < 6.0 * spl {
            return None;
        }
        let origin = self.phasing_origin(spl).unwrap_or(0.0);
        let mut rows: Vec<Vec<u8>> = Vec::new();
        let mut line = origin;
        while (line + spl) as usize <= self.pixels.len() {
            rows.push(Self::resample_line(&self.pixels, line, spl, width));
            line += spl;
        }
        // Trim near-constant margin rows (APT / phasing / black) at both ends.
        let is_flat = |row: &Vec<u8>| {
            let (mn, mx) = row.iter().fold((255u8, 0u8), |(mn, mx), &p| (mn.min(p), mx.max(p)));
            mx.saturating_sub(mn) < 24
        };
        let first = rows.iter().position(|r| !is_flat(r));
        let last = rows.iter().rposition(|r| !is_flat(r));
        let (first, last) = match (first, last) {
            (Some(a), Some(b)) => (a, b),
            _ => return None,
        };
        let gray: Vec<u8> = rows[first..=last].iter().flatten().copied().collect();
        if gray.is_empty() {
            return None;
        }
        Some(Frame {
            payload: FramePayload::Image { width: width as u16, gray },
            meta: FrameMeta {
                crc_ok: true,
                decoder: Some(self.variant.label().into()),
                ..Default::default()
            },
        })
    }
}

impl Demodulator for WefaxDemod {
    fn caps(&self) -> ModeCaps {
        WefaxMod::new(self.variant, self.center_hz).caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        for &x in samples {
            let p = self.demod(x);
            self.pixels.push(p);
        }
        Vec::new() // a fax finalizes at end-of-transmission (flush)
    }

    fn reset(&mut self) {
        *self = WefaxDemod::new(self.variant, self.center_hz);
    }

    fn flush(&mut self) -> Vec<Frame> {
        let out = self.assemble().into_iter().collect();
        self.pixels.clear();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioc_widths_match_reference() {
        // fldigi truncates `ioc * M_PI` to int: 576·π = 1809.55 → 1809,
        // 288·π = 904.78 → 904.
        assert_eq!(ioc_to_width(576), 1809);
        assert_eq!(ioc_to_width(288), 904);
        assert_eq!(WefaxVariant::Wefax576.width(), 1809);
        assert_eq!(WefaxVariant::Wefax288.width(), 904);
    }

    #[test]
    fn samples_per_line_from_lpm() {
        // 11025 Hz, 120 LPM → 5512.5 samples/line; 60 LPM → 11025.
        assert!((WefaxVariant::Wefax576.samples_per_line(11025.0) - 5512.5).abs() < 1e-3);
        assert!((WefaxVariant::Wefax288.samples_per_line(11025.0) - 11025.0).abs() < 1e-3);
    }

    #[test]
    fn caps_are_tx_at_native_rate() {
        let m = WefaxMod::new(WefaxVariant::Wefax576, CARRIER_HZ);
        assert!(m.caps().tx);
        assert_eq!(m.caps().native_rate, WEFAX_RATE);
    }

    #[test]
    fn rejects_non_image_payload() {
        let mut m = WefaxMod::new(WefaxVariant::Wefax576, CARRIER_HZ);
        assert!(matches!(
            m.modulate(&Frame::text("x")),
            Err(ModError::UnsupportedPayload(_))
        ));
    }

    #[test]
    fn variant_labels_round_trip() {
        for &v in WefaxVariant::all() {
            assert_eq!(WefaxVariant::from_label(v.label()), Some(v));
        }
        assert_eq!(WefaxVariant::from_label("bogus"), None);
    }

    /// FM pixel→freq→pixel round-trip through the modulate/demod core (the real
    /// DSP). A horizontal gradient line recovers within FM tolerance.
    #[test]
    fn fm_pixel_roundtrip_within_tolerance() {
        let v = WefaxVariant::Wefax288;
        let width = v.width() as usize;
        let spl = v.samples_per_line(WEFAX_RATE as f32);
        // A single gradient row rendered to audio, then demodulated.
        let row: Vec<u8> = (0..width).map(|c| (c * 255 / (width - 1)) as u8).collect();
        let m = WefaxMod::new(v, CARRIER_HZ);
        let vals: Vec<f32> = row.iter().map(|&p| p as f32 / 255.0).collect();
        let mut audio = Vec::new();
        let mut phase = 0.0;
        m.line(&mut audio, &mut phase, &vals, spl);
        let mut d = WefaxDemod::new(v, CARRIER_HZ);
        let pixels: Vec<u8> = audio.iter().map(|&x| d.demod(x)).collect();
        // Skip the channel-filter group delay, resample the line to width cols.
        let got = WefaxDemod::resample_line(&pixels, FILTER_DELAY, spl - FILTER_DELAY, width);
        // Compare the interior (edges blur under FM); mean abs error small.
        let lo = width / 10;
        let hi = width - width / 10;
        let err: f32 = (lo..hi).map(|c| (got[c] as f32 - row[c] as f32).abs()).sum::<f32>()
            / (hi - lo) as f32;
        assert!(err < 12.0, "mean abs pixel error {err}");
    }

    /// Full TX→RX loopback: a distinctive image recovers as a raster of the
    /// right width whose content reproduces the source rows within tolerance.
    #[test]
    fn loopback_recovers_raster() {
        for &v in WefaxVariant::all() {
            let width = v.width() as usize;
            // Small source image: 6 rows, a diagonal ramp so rows differ.
            let src_w = 64usize;
            let rows = 6usize;
            let mut gray = vec![0u8; src_w * rows];
            for r in 0..rows {
                for c in 0..src_w {
                    gray[r * src_w + c] = (((c + r * 8) * 255 / (src_w - 1)).min(255)) as u8;
                }
            }
            let img = Frame {
                payload: FramePayload::Image { width: src_w as u16, gray: gray.clone() },
                meta: FrameMeta::default(),
            };
            let mut tx = WefaxMod::new(v, CARRIER_HZ);
            let audio = tx.modulate(&img).unwrap();
            let mut rx = WefaxDemod::new(v, CARRIER_HZ);
            rx.feed(&audio);
            let frames = rx.flush();
            let frame = frames.first().expect("a raster");
            let (w, g) = match &frame.payload {
                FramePayload::Image { width, gray } => (*width as usize, gray),
                _ => panic!("expected image"),
            };
            assert_eq!(w, width, "{}: recovered width", v.label());
            let got_rows = g.len() / w;
            assert!(got_rows >= rows, "{}: got {got_rows} rows", v.label());
            // Some contiguous block of recovered rows must reproduce the source
            // (each source column stretched to the mode width) within tolerance.
            let expect_row = |r: usize| -> Vec<u8> {
                (0..w).map(|c| gray[r * src_w + (c * src_w / w).min(src_w - 1)]).collect()
            };
            let row_err = |got: &[u8], want: &[u8]| -> f32 {
                let lo = w / 8;
                let hi = w - w / 8;
                (lo..hi).map(|c| (got[c] as f32 - want[c] as f32).abs()).sum::<f32>()
                    / (hi - lo) as f32
            };
            let mut best = f32::INFINITY;
            for start in 0..=(got_rows - rows) {
                let e: f32 = (0..rows)
                    .map(|r| {
                        let got = &g[(start + r) * w..(start + r + 1) * w];
                        row_err(got, &expect_row(r))
                    })
                    .sum::<f32>()
                    / rows as f32;
                best = best.min(e);
            }
            assert!(best < 20.0, "{}: best block mean err {best}", v.label());
        }
    }
}
