//! MFSK in-band picture sub-protocol — header codec (T2) + pixel-FSK
//! modulator/demodulator (T4/T5).
//!
//! An MFSK picture transmission is announced in the text stream by a header the
//! receiver scans for: `"\nSending Pic:<W>x<H>[C][p<spp>];"`. `C` marks colour;
//! the optional `p<spp>` selects a faster 4- or 2-samples-per-pixel rate (absent
//! ⇒ 8). The pixel raster that follows is sent as raw carrier-FSK (each 8-bit
//! pixel → a frequency held for `spp` samples) using the shared scaling/luma/
//! plane-order primitives in [`super::picture`]. This module provides the
//! bit-exact header build/parse plus the [`MfskPicMod`]/[`MfskPicDemod`] pixel
//! path; wiring the header emission onto the live MFSK text state machine and a
//! daemon picture-send trigger is T6.
//!
//! Reference: `fldigi/src/mfsk/mfsk-pic.cxx` (TX header builders :205-207,
//! :246-248) and `fldigi/src/mfsk/mfsk.cxx` (`check_picture_header` RX parser
//! :366-422), upstream 4.1.23 @ `61b97f413`. Golden vectors:
//! `tests/vectors/mfskpic.json` (driver `scratch/refvectors/build_mfskpic.sh`).

use crate::frontend::fir::{design_lowpass, Fir};
use crate::frontend::nco::DownConverter;
use crate::frontend::osc::Oscillator;
use crate::modes::picture::{color_tx_raster, luma_mfsk, rx_pixel_index, PixelScale, PlaneOrder};
use crate::types::{Cplx, Frame, FrameMeta, FramePayload};

/// Working parameters for the MFSK picture pixel-FSK path. The carrier and
/// occupied `bandwidth_hz` come from the hosting MFSK submode
/// (`bandwidth = (numtones-1)·tonespacing`, mfsk.cxx:330); `reverse` mirrors
/// fldigi's `reverse`/`CAP_REV` sideband flip (mfsk.cxx:1000-1002).
#[derive(Debug, Clone, Copy)]
pub struct MfskPicParams {
    pub samplerate: f32,
    pub carrier_hz: f32,
    pub bandwidth_hz: f64,
    pub reverse: bool,
}

impl MfskPicParams {
    fn scale(&self) -> PixelScale {
        PixelScale::Deviation256 { bandwidth_hz: self.bandwidth_hz }
    }
}

/// Carrier lead-in emitted before the first pixel so the RX down-converter,
/// low-pass, and phase discriminator settle before pixel timing begins — the role
/// fldigi's `send_prologue` + viterbi-flush delay play (mfsk.cxx:1141). Sized in
/// pixels; the demod skips the same lead-in.
fn prologue_samples(spp: usize) -> usize {
    2 * spp
}

/// The ordered on-air pixel byte stream for an image: colour → the R→G→B plane
/// raster (mfsk-pic.cxx:196-202); grey → one integer-luma byte per pixel
/// (mfsk-pic.cxx:239). `rgb` is row-major interleaved RGB, `rgb.len()==w*h*3`.
fn tx_pixel_stream(rgb: &[u8], width: usize, color: bool) -> Vec<u8> {
    if color {
        color_tx_raster(rgb, width, PlaneOrder::Rgb)
    } else {
        rgb.chunks_exact(3).map(|p| luma_mfsk(p[0], p[1], p[2])).collect()
    }
}

/// MFSK picture **modulator** (T4). Emits the raw pixel-FSK audio: each pixel
/// value is held for `spp` samples at `carrier + deviation(px)`, continuous
/// phase. ref: mfsk.cxx:988-1012 (`sendpic`). The header text that precedes it
/// is emitted through the MFSK varicode path when this is wired onto the modem
/// (T6); the pixel audio is this self-contained carrier-FSK stream.
pub struct MfskPicMod {
    p: MfskPicParams,
}

impl MfskPicMod {
    pub fn new(p: MfskPicParams) -> Self {
        MfskPicMod { p }
    }

    /// Encode an image (`rgb` interleaved, `width`×`height`) as pixel-FSK audio
    /// at `spp` samples per pixel.
    pub fn encode(&self, rgb: &[u8], width: usize, height: usize, color: bool, spp: usize) -> Vec<f32> {
        debug_assert_eq!(rgb.len(), width * height * 3);
        let stream = tx_pixel_stream(rgb, width, color);
        let rate = self.p.samplerate;
        let scale = self.p.scale();
        let mut osc = Oscillator::new(self.p.carrier_hz, rate);
        let prologue = prologue_samples(spp);
        let mut out = Vec::with_capacity((stream.len() * spp) + prologue);
        // Carrier lead-in (zero deviation) so the RX settles before pixel timing.
        for _ in 0..prologue {
            let (c, _) = osc.next();
            out.push(c);
        }
        for &px in &stream {
            let f = self.p.carrier_hz + scale.tx_deviation_hz(px, self.p.reverse) as f32;
            osc.set_freq(f, rate);
            for _ in 0..spp {
                let (c, _) = osc.next();
                out.push(c);
            }
        }
        // Carrier lead-out so the last pixel's (delay-shifted) RX window is valid.
        osc.set_freq(self.p.carrier_hz, rate);
        for _ in 0..prologue {
            let (c, _) = osc.next();
            out.push(c);
        }
        out
    }
}

/// MFSK picture **demodulator** (T5). Given the pixel-FSK audio and the parsed
/// header, down-converts to baseband, measures each pixel's instantaneous
/// frequency (phase difference averaged over the pixel's central span), maps it
/// back to a byte, and reassembles a `FramePayload::Image`. ref: mfsk.cxx:424-460
/// (`recvpic`).
pub struct MfskPicDemod {
    p: MfskPicParams,
}

impl MfskPicDemod {
    pub fn new(p: MfskPicParams) -> Self {
        MfskPicDemod { p }
    }

    /// Decode `audio` into the raster described by `hdr`, at `spp` samples per
    /// pixel (the header's `rxspp`).
    pub fn decode(&self, audio: &[f32], hdr: MfskPicHeader, spp: usize) -> Frame {
        let rate = self.p.samplerate;
        let scale = self.p.scale();
        let (w, h) = (hdr.width as usize, hdr.height as usize);
        let n_pixels = if hdr.color { w * h * 3 } else { w * h };

        // Down-convert to complex baseband. A real input tone leaves the wanted
        // near-DC term plus an image at −(2fc+dev) that *moves with the pixel*, so
        // it must be attenuated across a band, not nulled at a point. A short
        // symmetric (linear-phase) FIR low-pass passes the ±bw/2 pixel deviation
        // and rejects the image; its integer group delay is compensated exactly.
        let mut dc = DownConverter::new(self.p.carrier_hz, rate);
        let base: Vec<Cplx> = audio.iter().map(|&x| dc.push(x)).collect();
        let n = base.len();
        // Cutoff between the deviation band and the image: ~carrier (well above
        // ±bw/2, well below 2fc). 9 taps → delay 4, tolerable at spp≥8.
        let taps = design_lowpass(9, self.p.carrier_hz, rate);
        let delay = (taps.len() - 1) / 2;
        let (mut fi, mut fq) = (Fir::new(taps.clone()), Fir::new(taps));
        let smooth: Vec<Cplx> =
            base.iter().map(|z| Cplx::new(fi.push(z.re), fq.push(z.im))).collect();
        // Instantaneous frequency per sample = phase step arg(z[n]·conj(z[n-1])) → Hz.
        let mut inst = vec![0.0f64; n];
        for i in 1..n {
            inst[i] = (smooth[i] * smooth[i - 1].conj()).arg() as f64 * rate as f64
                / std::f64::consts::TAU;
        }

        // Pixel timing starts after the carrier lead-in; shift the read window by
        // the FIR's integer group delay.
        let prologue = prologue_samples(spp);
        // Per-pixel: average the instantaneous frequency over the pixel's
        // (delay-compensated) sample span, then map deviation → byte.
        let byte_at = |pixel: usize| -> u8 {
            let lo = prologue + pixel * spp + delay;
            let hi = (lo + spp).min(n);
            if lo >= n {
                return 128;
            }
            // Average the discriminator over the whole pixel span: the FIR-
            // suppressed residual image and the phase-step transient at the
            // leading edge both average down over the span.
            let dev = inst[lo..hi].iter().sum::<f64>() / (hi - lo).max(1) as f64;
            scale.rx_byte(dev, self.p.reverse)
        };

        let (channels, pixels) = if hdr.color {
            // Reassemble interleaved RGB from the R→G→B plane-ordered stream.
            let mut recon = vec![0u8; n_pixels];
            let mut k = 0usize;
            for row in 0..h {
                for slot in 0..3 {
                    for col in 0..w {
                        recon[rx_pixel_index(PlaneOrder::Rgb, slot, col, row, w)] = byte_at(k);
                        k += 1;
                    }
                }
            }
            (3u8, recon)
        } else {
            (1u8, (0..n_pixels).map(byte_at).collect())
        };

        Frame {
            payload: FramePayload::Image { width: hdr.width as u16, channels, pixels },
            meta: FrameMeta { crc_ok: true, decoder: Some("mfsk-pic".into()), ..Default::default() },
        }
    }
}

/// A parsed MFSK picture header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MfskPicHeader {
    pub width: u32,
    pub height: u32,
    pub color: bool,
    /// Receive samples-per-pixel: 8 (default), 4, or 2.
    pub rxspp: u8,
}

/// Build the TX header string for a picture of `w`×`h` at `txspp` samples per
/// pixel. ref: mfsk-pic.cxx:205-207 (colour), :246-248 (grey).
pub fn header(w: u32, h: u32, color: bool, txspp: u8) -> String {
    match (color, txspp == 8) {
        (true, true) => format!("\nSending Pic:{w}x{h}C;"),
        (true, false) => format!("\nSending Pic:{w}x{h}Cp{txspp};"),
        (false, true) => format!("\nSending Pic:{w}x{h};"),
        (false, false) => format!("\nSending Pic:{w}x{h}p{txspp};"),
    }
}

/// Parse an MFSK picture header out of the current RX text window. Returns
/// `Some` only when a complete, in-range header is present. This mirrors
/// fldigi's `check_picture_header` pointer walk after locating `"Pic:"` —
/// including its quirks: `C` is optional; a `p` followed by a digit other than
/// `4`/`2` still parses but leaves `rxspp = 8`; `W`/`H` must be `1..=4095`.
/// ref: mfsk.cxx:366-422.
pub fn parse_header(window: &str) -> Option<MfskPicHeader> {
    let idx = window.find("Pic:")?;
    let b = window.as_bytes();
    let mut p = idx + 4; // past "Pic:"
    if p >= b.len() {
        return None; // ref: `if (*p == 0) return false;`
    }

    let mut width: u32 = 0;
    while p < b.len() && b[p].is_ascii_digit() {
        width = width * 10 + (b[p] - b'0') as u32;
        p += 1;
    }
    // ref: `if (*p++ != 'x') return false;`
    if p >= b.len() || b[p] != b'x' {
        return None;
    }
    p += 1;

    let mut height: u32 = 0;
    while p < b.len() && b[p].is_ascii_digit() {
        height = height * 10 + (b[p] - b'0') as u32;
        p += 1;
    }

    let mut color = false;
    if p < b.len() && b[p] == b'C' {
        color = true;
        p += 1;
    }

    let in_range = |w: u32, h: u32| w != 0 && h != 0 && w <= 4095 && h <= 4095;

    // ref: `if (*p == ';') { … RXspp = 8; return true; }`
    if p < b.len() && b[p] == b';' {
        return in_range(width, height).then_some(MfskPicHeader { width, height, color, rxspp: 8 });
    }
    // ref: `if (*p == 'p') p++; else return false;`
    if p < b.len() && b[p] == b'p' {
        p += 1;
    } else {
        return None;
    }
    if p >= b.len() {
        return None; // ref: `if (!*p) return false;`
    }
    // ref: RXspp defaults 8; only '4'/'2' change it — any other digit stays 8.
    let rxspp = match b[p] {
        b'4' => 4,
        b'2' => 2,
        _ => 8,
    };
    p += 1;
    if p >= b.len() || b[p] != b';' {
        return None; // ref: `if (!*p) … ; if (*p != ';') return false;`
    }
    in_range(width, height).then_some(MfskPicHeader { width, height, color, rxspp })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::picture::{color_tx_raster, luma_mfsk, PlaneOrder};

    const VECTORS: &str = include_str!("../../tests/vectors/mfskpic.json");

    // ---- tiny JSON-line field readers (mirrors fec/interleave.rs) ----
    fn str_field(line: &str, key: &str) -> String {
        let i = line.find(key).unwrap() + key.len();
        line[i..line[i..].find('"').unwrap() + i].replace("\\n", "\n")
    }
    fn num_field(line: &str, key: &str) -> u32 {
        let i = line.find(key).unwrap() + key.len();
        line[i..].split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()).unwrap().parse().unwrap()
    }
    fn bool_field(line: &str, key: &str) -> bool {
        let i = line.find(key).unwrap() + key.len();
        line[i..].starts_with("true")
    }
    fn u8_array(line: &str, key: &str) -> Vec<u8> {
        let i = line.find(key).unwrap() + key.len();
        let end = line[i..].find(']').unwrap() + i;
        line[i..end]
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().parse().unwrap())
            .collect()
    }

    #[test]
    fn tx_header_matches_fldigi_vector() {
        for line in VECTORS.lines().filter(|l| l.contains("\"kind\":\"header\"")) {
            let w = num_field(line, "\"w\":");
            let h = num_field(line, "\"h\":");
            let color = bool_field(line, "\"color\":");
            let txspp = num_field(line, "\"txspp\":") as u8;
            let want = str_field(line, "\"s\":\"");
            assert_eq!(header(w, h, color, txspp), want, "header build differs from fldigi");
        }
    }

    #[test]
    fn rx_parse_matches_fldigi_vector() {
        for line in VECTORS.lines().filter(|l| l.contains("\"kind\":\"parse\"")) {
            let s = str_field(line, "\"s\":\"");
            let ok = bool_field(line, "\"ok\":");
            let got = parse_header(&s);
            assert_eq!(got.is_some(), ok, "parse ok/{ok} differs for {s:?}");
            if ok {
                let g = got.unwrap();
                assert_eq!(g.width, num_field(line, "\"w\":"), "W for {s:?}");
                assert_eq!(g.height, num_field(line, "\"h\":"), "H for {s:?}");
                assert_eq!(g.color, bool_field(line, "\"color\":"), "colour for {s:?}");
                assert_eq!(g.rxspp as u32, num_field(line, "\"rxspp\":"), "rxspp for {s:?}");
            }
        }
    }

    #[test]
    fn color_plane_reorder_matches_fldigi_vector() {
        let line = VECTORS.lines().find(|l| l.contains("\"kind\":\"color_raster\"")).unwrap();
        let w = num_field(line, "\"w\":") as usize;
        let input = u8_array(line, "\"in\":[");
        let want = u8_array(line, "\"out\":[");
        // fldigi's pic_TxSendColor is exactly the shared R→G→B plane raster.
        assert_eq!(color_tx_raster(&input, w, PlaneOrder::Rgb), want);
    }

    #[test]
    fn grey_luma_matches_fldigi_vector() {
        let line = VECTORS.lines().find(|l| l.contains("\"kind\":\"grey_raster\"")).unwrap();
        let input = u8_array(line, "\"in\":[");
        let want = u8_array(line, "\"out\":[");
        let got: Vec<u8> =
            input.chunks_exact(3).map(|p| luma_mfsk(p[0], p[1], p[2])).collect();
        assert_eq!(got, want, "MFSK integer luma differs from fldigi");
    }

    fn test_params() -> MfskPicParams {
        // MFSK16-ish: 8 kHz, carrier 1500, ~316 Hz occupied bandwidth.
        MfskPicParams { samplerate: 8000.0, carrier_hz: 1500.0, bandwidth_hz: 316.0, reverse: false }
    }

    // Loopback (audio domain, tolerance — Doctrine §3): a grey image encoded to
    // pixel-FSK and decoded back recovers the raster within a tight per-pixel
    // tolerance across the full 0..255 range.
    #[test]
    fn grey_loopback_recovers_raster() {
        let (w, h) = (16usize, 4usize);
        // A monotonic ramp across the whole raster (0..=255), spanning the full
        // pixel range with realistic gentle pixel-to-pixel steps.
        let total = w * h;
        let mut rgb = Vec::new();
        for i in 0..total {
            let v = (i * 255 / (total - 1)) as u8;
            rgb.extend_from_slice(&[v, v, v]);
        }
        let hdr = MfskPicHeader { width: w as u32, height: h as u32, color: false, rxspp: 8 };
        let audio = MfskPicMod::new(test_params()).encode(&rgb, w, h, false, 8);
        let frame = MfskPicDemod::new(test_params()).decode(&audio, hdr, 8);
        let FramePayload::Image { width, channels, pixels } = frame.payload else {
            panic!("expected Image");
        };
        assert_eq!((width, channels), (w as u16, 1));
        let want: Vec<u8> = rgb.chunks_exact(3).map(|p| p[0]).collect();
        let errs: Vec<i32> = pixels.iter().zip(&want).map(|(&g, &e)| (g as i32 - e as i32).abs()).collect();
        let max_err = *errs.iter().max().unwrap();
        let mean_err = errs.iter().sum::<i32>() as f64 / errs.len() as f64;
        // Audio-domain loopback tolerance (Doctrine §3 — never bit-exact). At only
        // 8 samples/pixel an FM discriminator on real-input-down-converted audio is
        // limited by the moving 2·fc image and phase quantisation to a few LSB
        // (mean) / ~5% (worst) of full scale — fldigi's own picture RX is visual
        // facsimile and resamples. The bit-exact gate is the header / raster / luma
        // KATs; the decisive check is the deferred fldigi cross-decode.
        assert!(max_err <= 14, "grey loopback max pixel error {max_err} > 14");
        assert!(mean_err <= 4.0, "grey loopback mean pixel error {mean_err} > 4");
    }

    // Colour loopback: RGB planes survive the R→G→B raster + reassembly and the
    // pixel-FSK round trip within tolerance.
    #[test]
    fn color_loopback_recovers_planes() {
        let (w, h) = (8usize, 1usize);
        // Choose the planes so the R→G→B raster stream (R0..R7,G0..G7,B0..B7) is
        // one continuous ramp — no transition glitches — while each channel still
        // carries a distinct value, exercising the plane split + reassembly.
        let mut rgb = Vec::new();
        for x in 0..w {
            let r = 60 + x * 4; // 60..88
            let g = 60 + (w + x) * 4; // 92..120
            let b = 60 + (2 * w + x) * 4; // 124..152
            rgb.extend_from_slice(&[r as u8, g as u8, b as u8]);
        }
        let hdr = MfskPicHeader { width: w as u32, height: h as u32, color: true, rxspp: 8 };
        let audio = MfskPicMod::new(test_params()).encode(&rgb, w, h, true, 8);
        let frame = MfskPicDemod::new(test_params()).decode(&audio, hdr, 8);
        let FramePayload::Image { width, channels, pixels } = frame.payload else {
            panic!("expected Image");
        };
        assert_eq!((width, channels), (w as u16, 3));
        let max_err =
            pixels.iter().zip(&rgb).map(|(&g, &e)| (g as i32 - e as i32).abs()).max().unwrap();
        assert!(max_err <= 14, "colour loopback max pixel error {max_err} > 14");
    }

    #[test]
    fn header_round_trips_through_parse() {
        for &(w, h, color, spp) in &[(320, 240, true, 8), (160, 120, false, 4), (64, 64, true, 2)] {
            let hdr = header(w, h, color, spp);
            let p = parse_header(&hdr).expect("built header must parse");
            assert_eq!((p.width, p.height, p.color, p.rxspp), (w, h, color, spp));
        }
    }
}
