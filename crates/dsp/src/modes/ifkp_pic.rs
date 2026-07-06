//! IFKP in-band picture sub-protocol — header codec (T2) + shared pixel-FSK
//! path ([`super::picture::PictureCodec`]). Both the TX encode (T4) and the RX
//! loopback (T5) run over the shared engine: `PictureCodec::decode` now uses a
//! rate-robust analytic (Hilbert) front-end, so the discriminator is image-free
//! at IFKP's 16 kHz and the loopback closes (grey + colour tests below).
//!
//! An IFKP picture is announced by a `pic%X` token in the text stream, where the
//! single mode char `X` selects both a fixed image size and colour/grey (upper =
//! colour, lower = grey; `F` = 640×480 grey; `A` = a 59×74 avatar, out of scope
//! here). Unlike MFSK there is no explicit `WxH` — the size comes from the char.
//! The raster that follows is raw carrier-FSK over the shared engine: 16 kHz,
//! `Deviation256` scaling, BT.601 luma, R→G→B planes.
//!
//! Reference: `fldigi/src/ifkp/ifkp.cxx` (`parse_pic` RX table :385-420,
//! `send_image` :807-850) and `fldigi/src/ifkp/ifkp-pic.cxx` (TX header char
//! table :461-470), upstream 4.1.23 @ `61b97f413`. Golden vectors:
//! `tests/vectors/ifkppic.json` (driver `scratch/refvectors/build_ifkppic.sh`).

use crate::mode::Modulator;
use crate::modes::ifk33::NUMTONES;
use crate::modes::ifkp::{tone_spacing, IfkpMod, IfkpSpeed};
use crate::modes::picture::{LumaKind, PictureCodec, PixelScale, PlaneOrder};
use crate::types::{Frame, Sample};

/// IFKP picture occupied bandwidth (Hz) — `NUMTONES · tone_spacing`, the value
/// the `Deviation256` pixel scale spans. ref: ifkp.cxx:270.
pub fn bandwidth_hz() -> f64 {
    NUMTONES as f64 * tone_spacing() as f64
}

/// IFKP picture sample rate and samples-per-pixel (ifkp.cxx:58, ifkp.h:48).
pub const SAMPLE_RATE: f32 = 16000.0;
pub const SPP: usize = 8;

/// A fixed IFKP picture size (the TX size selector; ifkp-pic.cxx:461-470).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfkpPicSize {
    Thumb,    // 59×74   (T/t)
    Mini,     // 120×150 (M/m)
    Portrait, // 240×300 (P/p)
    Small,    // 160×120 (S/s)
    Large,    // 320×240 (L/l)
    Vga,      // 640×480 (V / F-grey)
}

impl IfkpPicSize {
    pub fn dims(self) -> (u32, u32) {
        match self {
            IfkpPicSize::Thumb => (59, 74),
            IfkpPicSize::Mini => (120, 150),
            IfkpPicSize::Portrait => (240, 300),
            IfkpPicSize::Small => (160, 120),
            IfkpPicSize::Large => (320, 240),
            IfkpPicSize::Vga => (640, 480),
        }
    }
    /// The on-air mode char for this size at the given colour. ref: ifkp-pic.cxx:461-470.
    fn tx_char(self, grey: bool) -> char {
        match (self, grey) {
            (IfkpPicSize::Thumb, false) => 'T',
            (IfkpPicSize::Thumb, true) => 't',
            (IfkpPicSize::Mini, false) => 'M',
            (IfkpPicSize::Mini, true) => 'm',
            (IfkpPicSize::Portrait, false) => 'P',
            (IfkpPicSize::Portrait, true) => 'p',
            (IfkpPicSize::Small, false) => 'S',
            (IfkpPicSize::Small, true) => 's',
            (IfkpPicSize::Large, false) => 'L',
            (IfkpPicSize::Large, true) => 'l',
            (IfkpPicSize::Vga, false) => 'V',
            (IfkpPicSize::Vga, true) => 'F',
        }
    }
}

/// A parsed IFKP picture header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfkpPicHeader {
    pub width: u32,
    pub height: u32,
    pub color: bool,
    /// 59×74 avatar (`A`) — recognised but not decoded here.
    pub avatar: bool,
}

/// Build the TX header token `" pic%X"` for a size at the given colour.
/// ref: ifkp-pic.cxx:460-470 (`picmode = " pic%" + ch`).
pub fn header(size: IfkpPicSize, grey: bool) -> String {
    format!(" pic%{}", size.tx_char(grey))
}

/// Parse an IFKP picture header from the current RX text window. Mirrors
/// `parse_pic`: locate `"pic%"`, map the following char through the fixed
/// size/colour table; an unrecognised char yields `None`. ref: ifkp.cxx:385-420.
pub fn parse_header(window: &str) -> Option<IfkpPicHeader> {
    let idx = window.find("pic%")?;
    let c = window.as_bytes().get(idx + 4).copied()? as char;
    let (width, height, color, avatar) = match c {
        'A' => (59, 74, true, true),
        'T' => (59, 74, true, false),
        't' => (59, 74, false, false),
        'S' => (160, 120, true, false),
        's' => (160, 120, false, false),
        'L' => (320, 240, true, false),
        'l' => (320, 240, false, false),
        'V' => (640, 480, true, false),
        'v' => (640, 480, false, false),
        'F' => (640, 480, false, false),
        'P' => (240, 300, true, false),
        'p' => (240, 300, false, false),
        'M' => (120, 150, true, false),
        'm' => (120, 150, false, false),
        _ => return None,
    };
    Some(IfkpPicHeader { width, height, color, avatar })
}

/// Build the shared pixel-FSK codec configured for IFKP pictures: 16 kHz,
/// linear `Deviation256` scaling over the mode's occupied `bandwidth_hz`
/// (ifkp.cxx:270), BT.601 luma, R→G→B planes.
pub fn codec(carrier_hz: f32, bandwidth_hz: f64, reverse: bool) -> PictureCodec {
    PictureCodec {
        samplerate: SAMPLE_RATE,
        carrier_hz,
        reverse,
        scale: PixelScale::Deviation256 { bandwidth_hz },
        luma: LumaKind::Std,
        order: PlaneOrder::Rgb,
        label: "ifkp-pic",
    }
}

/// **Picture-send assembler (T6 TX core).** Build the complete on-air audio for
/// an IFKP picture at `speed` centred at `center_hz`: the ` pic%X` header
/// modulated through the live IFKP text path, then the pixel-FSK raster over the
/// shared [`PictureCodec`] at 16 kHz IMAGESPP=8. `size` selects the fixed W×H;
/// `grey` picks the luma reduction vs the R→G→B plane raster. A daemon
/// picture-send trigger keys the rig and plays this buffer.
pub fn build_tx(
    speed: IfkpSpeed,
    center_hz: f32,
    size: IfkpPicSize,
    rgb: &[u8],
    grey: bool,
    reverse: bool,
) -> Vec<Sample> {
    let (w, h) = size.dims();
    let mut audio =
        IfkpMod::new(speed, center_hz).modulate(&Frame::text(header(size, grey))).unwrap_or_default();
    let cdc = codec(center_hz, bandwidth_hz(), reverse);
    let pixels = cdc.encode(rgb, w as usize, h as usize, !grey, SPP);
    audio.extend_from_slice(&pixels);
    audio
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Demodulator;
    use crate::modes::ifkp::IfkpDemod;
    use crate::modes::picture::{color_tx_raster, PlaneOrder};
    use crate::types::FramePayload;

    const VECTORS: &str = include_str!("../../tests/vectors/ifkppic.json");

    fn str_field(line: &str, key: &str) -> String {
        let i = line.find(key).unwrap() + key.len();
        line[i..line[i..].find('"').unwrap() + i].to_string()
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
        line[i..end].split(',').filter(|s| !s.trim().is_empty()).map(|s| s.trim().parse().unwrap()).collect()
    }

    fn size_for(w: u32, h: u32) -> IfkpPicSize {
        [
            IfkpPicSize::Thumb,
            IfkpPicSize::Mini,
            IfkpPicSize::Portrait,
            IfkpPicSize::Small,
            IfkpPicSize::Large,
            IfkpPicSize::Vga,
        ]
        .into_iter()
        .find(|s| s.dims() == (w, h))
        .unwrap()
    }

    #[test]
    fn tx_header_matches_fldigi_vector() {
        for line in VECTORS.lines().filter(|l| l.contains("\"kind\":\"header\"")) {
            let (w, h) = (num_field(line, "\"w\":"), num_field(line, "\"h\":"));
            let grey = bool_field(line, "\"grey\":");
            let want = str_field(line, "\"s\":\"");
            assert_eq!(header(size_for(w, h), grey), want, "header build differs from fldigi");
        }
    }

    #[test]
    fn rx_parse_matches_fldigi_vector() {
        for line in VECTORS.lines().filter(|l| l.contains("\"kind\":\"parse\"")) {
            let c = str_field(line, "\"c\":\"");
            let ok = bool_field(line, "\"ok\":");
            // Feed a realistic window; the parser scans for "pic%".
            let window = format!("some text pic%{c}");
            let got = parse_header(&window);
            assert_eq!(got.is_some(), ok, "parse ok/{ok} differs for {c:?}");
            if ok {
                let g = got.unwrap();
                assert_eq!(g.width, num_field(line, "\"w\":"), "W for {c:?}");
                assert_eq!(g.height, num_field(line, "\"h\":"), "H for {c:?}");
                assert_eq!(g.color, bool_field(line, "\"color\":"), "colour for {c:?}");
                assert_eq!(g.avatar, bool_field(line, "\"avatar\":"), "avatar for {c:?}");
            }
        }
    }

    #[test]
    fn color_plane_reorder_matches_fldigi_vector() {
        let line = VECTORS.lines().find(|l| l.contains("\"kind\":\"color_raster\"")).unwrap();
        let w = num_field(line, "\"w\":") as usize;
        let input = u8_array(line, "\"in\":[");
        let want = u8_array(line, "\"out\":[");
        assert_eq!(color_tx_raster(&input, w, PlaneOrder::Rgb), want);
    }

    fn test_codec() -> PictureCodec {
        // 16 kHz, carrier 1500, ~386 Hz occupied bandwidth (ifkp.cxx:270).
        codec(1500.0, 386.0, false)
    }

    // T6 TX assembler: header (recoverable by the real IFKP demod → parse_header)
    // then the pixel-FSK raster (recoverable by the shared codec at the split).
    #[test]
    fn build_tx_carries_header_then_raster() {
        let speed = IfkpSpeed::Normal;
        let center = 1500.0f32;
        let size = IfkpPicSize::Thumb; // 59×74
        let (w, h) = size.dims();
        let total = (w * h) as usize;
        let mut rgb = Vec::new();
        for i in 0..total {
            let g = (i * 255 / (total - 1)) as u8;
            rgb.extend_from_slice(&[g, g, g]);
        }
        let hdr_len = IfkpMod::new(speed, center)
            .modulate(&Frame::text(header(size, true)))
            .unwrap()
            .len();
        let full = build_tx(speed, center, size, &rgb, true, false);
        assert!(full.len() > hdr_len, "picture audio must extend past the header");

        // The streaming IFKP demod emits a varicode char only once the next
        // symbol arrives, so the header's final char lands after the split — feed
        // the whole stream (as a live receiver does) and locate the header, which
        // is the first `pic%` token.
        let text: String = IfkpDemod::new(speed, center)
            .feed(&full)
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(parse_header(&text).is_some(), "demodulated IFKP header must parse: {text:?}");

        let cdc = codec(center, bandwidth_hz(), false);
        let frame = cdc.decode(&full[hdr_len..], w as usize, h as usize, false, SPP);
        let FramePayload::Image { pixels, .. } = frame.payload else { panic!("expected Image") };
        let want: Vec<u8> = rgb.chunks_exact(3).map(|p| p[0]).collect();
        let max_err =
            pixels.iter().zip(&want).map(|(&g, &e)| (g as i32 - e as i32).abs()).max().unwrap();
        assert!(max_err <= 14, "IFKP built-raster loopback max pixel error {max_err} > 14");
    }

    // Loopback (audio domain, tolerance — Doctrine §3) at IFKP's 16 kHz. Closed by
    // the analytic front-end in `PictureCodec::decode`; the same gate MFSK holds.
    #[test]
    fn grey_loopback_recovers_raster() {
        use crate::types::FramePayload;
        let (w, h) = (16usize, 4usize);
        let total = w * h;
        let mut rgb = Vec::new();
        for i in 0..total {
            let v = (i * 255 / (total - 1)) as u8;
            rgb.extend_from_slice(&[v, v, v]);
        }
        let audio = test_codec().encode(&rgb, w, h, false, SPP);
        let frame = test_codec().decode(&audio, w, h, false, SPP);
        let FramePayload::Image { width, channels, pixels } = frame.payload else {
            panic!("expected Image");
        };
        assert_eq!((width, channels), (w as u16, 1));
        // BT.601 grey of a grey ramp is the ramp itself.
        let want: Vec<u8> = rgb.chunks_exact(3).map(|p| p[0]).collect();
        let errs: Vec<i32> =
            pixels.iter().zip(&want).map(|(&g, &e)| (g as i32 - e as i32).abs()).collect();
        let max_err = *errs.iter().max().unwrap();
        let mean_err = errs.iter().sum::<i32>() as f64 / errs.len() as f64;
        assert!(max_err <= 14, "IFKP grey loopback max pixel error {max_err} > 14");
        assert!(mean_err <= 4.0, "IFKP grey loopback mean pixel error {mean_err} > 4");
    }

    #[test]
    fn color_loopback_recovers_planes() {
        use crate::types::FramePayload;
        let (w, h) = (8usize, 1usize);
        // Planes chosen so the R→G→B raster is one continuous ramp (no glitches)
        // while each channel keeps a distinct value.
        let mut rgb = Vec::new();
        for x in 0..w {
            let r = 60 + x * 4;
            let g = 60 + (w + x) * 4;
            let b = 60 + (2 * w + x) * 4;
            rgb.extend_from_slice(&[r as u8, g as u8, b as u8]);
        }
        let audio = test_codec().encode(&rgb, w, h, true, SPP);
        let frame = test_codec().decode(&audio, w, h, true, SPP);
        let FramePayload::Image { width, channels, pixels } = frame.payload else {
            panic!("expected Image");
        };
        assert_eq!((width, channels), (w as u16, 3));
        let max_err =
            pixels.iter().zip(&rgb).map(|(&g, &e)| (g as i32 - e as i32).abs()).max().unwrap();
        assert!(max_err <= 14, "IFKP colour loopback max pixel error {max_err} > 14");
    }
}
