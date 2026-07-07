//! MFSK in-band picture sub-protocol — header codec (T2) + pixel-FSK
//! modulator/demodulator (T4/T5).
//!
//! An MFSK picture transmission is announced in the text stream by a header the
//! receiver scans for: `"\nSending Pic:<W>x<H>[C][p<spp>];"`. `C` marks colour;
//! the optional `p<spp>` selects a faster 4- or 2-samples-per-pixel rate (absent
//! ⇒ 8). The pixel raster that follows is sent as raw carrier-FSK (each 8-bit
//! pixel → a frequency held for `spp` samples) using the shared scaling/luma/
//! plane-order primitives in [`super::picture`]. This module provides the
//! bit-exact header build/parse and configures the shared [`PictureCodec`] with
//! MFSK's conventions ([`codec`]); wiring the header emission onto the live MFSK
//! text state machine and a daemon picture-send trigger is T6.
//!
//! Reference: `fldigi/src/mfsk/mfsk-pic.cxx` (TX header builders :205-207,
//! :246-248) and `fldigi/src/mfsk/mfsk.cxx` (`check_picture_header` RX parser
//! :366-422), upstream 4.1.23 @ `61b97f413`. Golden vectors:
//! `tests/vectors/mfskpic.json` (driver `scratch/refvectors/build_mfskpic.sh`).

use crate::mode::Modulator;
use crate::modes::mfsk::{MfskMod, MfskVariant};
use crate::modes::picture::{LumaKind, PictureCodec, PixelScale, PlaneOrder, RasterRef};
use crate::types::{Frame, Sample};

/// Build the shared pixel-FSK codec configured for MFSK pictures: linear
/// `Deviation256` scaling over the submode's occupied `bandwidth_hz`
/// (`(numtones-1)*tonespacing`, mfsk.cxx:330), MFSK integer luma, R->G->B planes.
/// `reverse` mirrors fldigi's `reverse`/`CAP_REV` sideband flip
/// (mfsk.cxx:1000-1002). The pixel raster path (T4/T5) is the shared engine; this
/// module supplies MFSK's header codec and conventions.
pub fn codec(carrier_hz: f32, bandwidth_hz: f64, samplerate: f32, reverse: bool) -> PictureCodec {
    PictureCodec {
        samplerate,
        carrier_hz,
        reverse,
        scale: PixelScale::Deviation256 { bandwidth_hz },
        luma: LumaKind::Mfsk,
        order: PlaneOrder::Rgb,
        label: "mfsk-pic",
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

/// **Picture-send assembler (T6 TX core).** Build the complete on-air audio for
/// an MFSK picture transmission on submode `v` centred at `center_hz`: the
/// in-band header text (`"\nSending Pic:WxH…;"`) modulated through the live MFSK
/// text path, immediately followed by the pixel-FSK raster over the shared
/// [`PictureCodec`]. `rgb` is row-major interleaved RGB (`width*height*3` bytes);
/// `color` picks the R→G→B plane raster vs the grey luma reduction; `txspp`
/// selects 8/4/2 samples-per-pixel (the header advertises it). A daemon
/// picture-send trigger keys the rig and plays this buffer.
///
/// The header rides the mode's existing varicode modulator, so a stock fldigi
/// MFSK receiver sees the `Pic:` announcement then decodes the raster — the
/// symmetric partner of the typed `Image` RX frame.
pub fn build_tx(
    v: MfskVariant,
    center_hz: f32,
    img: RasterRef,
    color: bool,
    txspp: u8,
    reverse: bool,
) -> Vec<Sample> {
    let hdr = header(img.width, img.height, color, txspp);
    let mut audio = MfskMod::new(v, center_hz).modulate(&Frame::text(hdr)).unwrap_or_default();
    let cdc = codec(center_hz, v.params().bandwidth() as f64, v.samplerate() as f32, reverse);
    let pixels = cdc.encode(img.rgb, img.width as usize, img.height as usize, color, txspp as usize);
    audio.extend_from_slice(&pixels);
    audio
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Demodulator;
    use crate::modes::mfsk::MfskDemod;
    use crate::modes::picture::{color_tx_raster, luma_mfsk, PlaneOrder};
    use crate::types::FramePayload;

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

    fn test_codec() -> PictureCodec {
        // MFSK16-ish: 8 kHz, carrier 1500, ~316 Hz occupied bandwidth.
        codec(1500.0, 316.0, 8000.0, false)
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
        let audio = test_codec().encode(&rgb, w, h, false, 8);
        let frame = test_codec().decode(&audio, w, h, false, 8);
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
        let audio = test_codec().encode(&rgb, w, h, true, 8);
        let frame = test_codec().decode(&audio, w, h, true, 8);
        let FramePayload::Image { width, channels, pixels } = frame.payload else {
            panic!("expected Image");
        };
        assert_eq!((width, channels), (w as u16, 3));
        let max_err =
            pixels.iter().zip(&rgb).map(|(&g, &e)| (g as i32 - e as i32).abs()).max().unwrap();
        assert!(max_err <= 14, "colour loopback max pixel error {max_err} > 14");
    }

    // T6 TX assembler: the built audio carries the header (recoverable by the
    // real MFSK text demod → parse_header) immediately followed by the pixel-FSK
    // raster (recoverable by the shared codec at the known header offset).
    #[test]
    fn build_tx_carries_header_then_raster() {
        let v = MfskVariant::M16;
        let center = 1500.0f32;
        let (w, h) = (16u32, 4u32);
        let txspp = 8u8;
        // Grey ramp raster.
        let total = (w * h) as usize;
        let mut rgb = Vec::new();
        for i in 0..total {
            let g = (i * 255 / (total - 1)) as u8;
            rgb.extend_from_slice(&[g, g, g]);
        }
        // Header-only audio length is the split point between text and pixels.
        let hdr_audio = MfskMod::new(v, center)
            .modulate(&Frame::text(header(w, h, false, txspp)))
            .unwrap();
        let full = build_tx(v, center, RasterRef { rgb: &rgb, width: w, height: h }, false, txspp, false);
        assert!(full.len() > hdr_audio.len(), "picture audio must extend past the header");

        // The header prefix demodulates back to text containing the Pic: header,
        // which parse_header accepts with the right dims.
        let text: String = MfskDemod::new(v, center)
            .feed(&full[..hdr_audio.len()])
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        let parsed = parse_header(&text).expect("demodulated header must parse");
        assert_eq!((parsed.width, parsed.height, parsed.color, parsed.rxspp), (w, h, false, txspp));

        // The pixel suffix decodes to the raster within loopback tolerance.
        let cdc = codec(center, v.params().bandwidth() as f64, v.samplerate() as f32, false);
        let frame = cdc.decode(&full[hdr_audio.len()..], w as usize, h as usize, false, txspp as usize);
        let FramePayload::Image { pixels, .. } = frame.payload else { panic!("expected Image") };
        let want: Vec<u8> = rgb.chunks_exact(3).map(|p| p[0]).collect();
        let max_err =
            pixels.iter().zip(&want).map(|(&g, &e)| (g as i32 - e as i32).abs()).max().unwrap();
        assert!(max_err <= 14, "built-raster loopback max pixel error {max_err} > 14");
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
