//! THOR in-band picture sub-protocol — header codec (T2) + shared pixel-FSK
//! path ([`super::picture::PictureCodec`]). Both TX encode (T4) and RX loopback
//! (T5) run over the shared engine, which uses a rate-robust analytic front-end,
//! so THOR's 8 kHz IMAGEspp=10 raster round-trips within tolerance.
//!
//! A THOR picture is announced by a `pic%X` token in the text stream (fldigi
//! transmits `"pic%X\n"`), where the single mode char `X` selects both a fixed
//! image size and colour/grey (upper = colour, lower = grey; `F` = 640×480 grey;
//! `A` = a 59×74 avatar, out of scope here). Like IFKP there is no explicit
//! `WxH` — the size comes from the char. The raster that follows is raw
//! carrier-FSK over the shared engine: 8 kHz, `Deviation256` scaling, BT.601
//! luma, R→G→B planes, held IMAGEspp=10 samples per pixel.
//!
//! Reference: `fldigi/src/thor/thor.cxx` (`parse_pic` RX table :404-420,
//! `send_image` colour/grey loops :1324-1362, RX byte :974) and
//! `fldigi/src/thor/thor-pic.cxx` (TX header builder :370-439), upstream
//! 4.1.23 @ `61b97f413`. Golden vectors: `tests/vectors/thorpic.json`
//! (driver `scratch/refvectors/build_thorpic.sh`).

use crate::modes::picture::{LumaKind, PictureCodec, PixelScale, PlaneOrder};

/// THOR picture sample rate and samples-per-pixel (thor.cxx:243 family,
/// `THOR_IMAGESPP` thor.h:68).
pub const SAMPLE_RATE: f32 = 8000.0;
pub const SPP: usize = 10;

/// A fixed THOR picture size (the TX size selector; thor-pic.cxx:390-439).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThorPicSize {
    Thumb,    // 59×74   (T/t)
    Mini,     // 120×150 (M/m)
    Portrait, // 240×300 (P/p)
    Small,    // 160×120 (S/s)
    Large,    // 320×240 (L/l)
    Vga,      // 640×480 (V colour / F grey)
}

impl ThorPicSize {
    pub fn dims(self) -> (u32, u32) {
        match self {
            ThorPicSize::Thumb => (59, 74),
            ThorPicSize::Mini => (120, 150),
            ThorPicSize::Portrait => (240, 300),
            ThorPicSize::Small => (160, 120),
            ThorPicSize::Large => (320, 240),
            ThorPicSize::Vga => (640, 480),
        }
    }
    /// The on-air mode char for this size at the given colour. ref: thor-pic.cxx:390-439.
    fn tx_char(self, grey: bool) -> char {
        match (self, grey) {
            (ThorPicSize::Thumb, false) => 'T',
            (ThorPicSize::Thumb, true) => 't',
            (ThorPicSize::Mini, false) => 'M',
            (ThorPicSize::Mini, true) => 'm',
            (ThorPicSize::Portrait, false) => 'P',
            (ThorPicSize::Portrait, true) => 'p',
            (ThorPicSize::Small, false) => 'S',
            (ThorPicSize::Small, true) => 's',
            (ThorPicSize::Large, false) => 'L',
            (ThorPicSize::Large, true) => 'l',
            (ThorPicSize::Vga, false) => 'V',
            (ThorPicSize::Vga, true) => 'F',
        }
    }
}

/// A parsed THOR picture header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThorPicHeader {
    pub width: u32,
    pub height: u32,
    pub color: bool,
    /// 59×74 avatar (`A`) — recognised but not decoded here.
    pub avatar: bool,
}

/// Build the TX header token `"pic%X\n"` for a size at the given colour.
/// ref: thor-pic.cxx:370,390-439 (`picmode = "pic% \n"`, `picmode[4] = ch`).
pub fn header(size: ThorPicSize, grey: bool) -> String {
    format!("pic%{}\n", size.tx_char(grey))
}

/// Parse a THOR picture header from the current RX text window. Mirrors
/// `parse_pic`: locate `"pic%"`, map the following char through the fixed
/// size/colour table; an unrecognised char yields `None`. `image_mode==1` is
/// grey (`color=false`). ref: thor.cxx:404-420.
pub fn parse_header(window: &str) -> Option<ThorPicHeader> {
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
    Some(ThorPicHeader { width, height, color, avatar })
}

/// Build the shared pixel-FSK codec configured for THOR pictures: 8 kHz, linear
/// `Deviation256` scaling over the submode's occupied `bandwidth_hz`
/// (`THORNUMTONES*tonespacing`, thor.cxx:301), BT.601 luma, R→G→B planes.
/// `reverse` mirrors THOR's reverse sideband flip.
pub fn codec(carrier_hz: f32, bandwidth_hz: f64, samplerate: f32, reverse: bool) -> PictureCodec {
    PictureCodec {
        samplerate,
        carrier_hz,
        reverse,
        scale: PixelScale::Deviation256 { bandwidth_hz },
        luma: LumaKind::Std,
        order: PlaneOrder::Rgb,
        label: "thor-pic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::picture::{color_tx_raster, PlaneOrder};

    const VECTORS: &str = include_str!("../../tests/vectors/thorpic.json");

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
        line[i..end].split(',').filter(|s| !s.trim().is_empty()).map(|s| s.trim().parse().unwrap()).collect()
    }

    fn size_for(w: u32, h: u32) -> ThorPicSize {
        [
            ThorPicSize::Thumb,
            ThorPicSize::Mini,
            ThorPicSize::Portrait,
            ThorPicSize::Small,
            ThorPicSize::Large,
            ThorPicSize::Vga,
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

    #[test]
    fn header_round_trips_through_parse() {
        for &(size, grey) in &[
            (ThorPicSize::Large, false),
            (ThorPicSize::Small, true),
            (ThorPicSize::Vga, true), // grey VGA is 'F'
            (ThorPicSize::Vga, false), // colour VGA is 'V'
        ] {
            let hdr = header(size, grey);
            let p = parse_header(&hdr).expect("built header must parse");
            assert_eq!((p.width, p.height, p.color), (size.dims().0, size.dims().1, !grey));
        }
    }

    fn test_codec() -> PictureCodec {
        // THOR: 8 kHz, carrier 1500, ~244 Hz occupied bandwidth (THOR16-ish).
        codec(1500.0, 244.0, SAMPLE_RATE, false)
    }

    // Loopback (audio domain, tolerance — Doctrine §3) at THOR's 8 kHz / spp=10.
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
        let want: Vec<u8> = rgb.chunks_exact(3).map(|p| p[0]).collect();
        let errs: Vec<i32> =
            pixels.iter().zip(&want).map(|(&g, &e)| (g as i32 - e as i32).abs()).collect();
        let max_err = *errs.iter().max().unwrap();
        let mean_err = errs.iter().sum::<i32>() as f64 / errs.len() as f64;
        assert!(max_err <= 14, "THOR grey loopback max pixel error {max_err} > 14");
        assert!(mean_err <= 4.0, "THOR grey loopback mean pixel error {mean_err} > 4");
    }

    #[test]
    fn color_loopback_recovers_planes() {
        use crate::types::FramePayload;
        let (w, h) = (8usize, 1usize);
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
        assert!(max_err <= 14, "THOR colour loopback max pixel error {max_err} > 14");
    }
}
