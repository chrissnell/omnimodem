//! FSQ in-band picture sub-protocol — header codec (T2) + shared pixel-FSK path
//! ([`super::picture::PictureCodec`]). Both TX encode (T4) and RX loopback (T5)
//! run over the shared engine (rate-robust analytic front-end), so FSQ's 12 kHz
//! RXspp=10 raster round-trips within tolerance.
//!
//! Unlike the MFSK/THOR/IFKP families, FSQ pictures are a **directed** message:
//! the header is the `%` trigger token `"% X"` (percent, space, mode char) that
//! FSQ's directed layer dispatches to `parse_pcnt`, which reads the mode char at
//! `rx_text[2]`. The char selects one of eight `image_mode`s (size + colour).
//! Two FSQ specifics differ from the other families:
//!   - **`FsqLinear` quantiser**: TX `dev = −200 + px·1.5`, RX `byte = dev/1.5 +
//!     128`. These affines are **not** inverse (RX mixes at `frequency`), so a
//!     clean loopback lands ~6 counts low — fldigi's real behaviour, pinned by
//!     the golden vector.
//!   - **B→G→R plane order** ([`PlaneOrder::Bgr`], `RGB[]={2,1,0}`).
//!
//! Grey luma is the BT.601 `0.3/0.6/0.1` reduction, continuous on the wire.
//!
//! Reference: `fldigi/src/fsq/fsq.cxx` (`parse_pcnt` RX table :876-902, `sendpic`
//! :1419-1465, `recvpic` byte :1206, plane map :1210, `phidiff` :188) and
//! `fldigi/src/fsq/fsq-pic.cxx` (TX header tokens :369-378), upstream 4.1.23 @
//! `61b97f413`. Golden vectors: `tests/vectors/fsqpic.json` (driver
//! `scratch/refvectors/build_fsqpic.sh`).

use crate::modes::picture::{LumaKind, PictureCodec, PixelScale, PlaneOrder};

/// FSQ picture sample rate and samples-per-pixel (`SR` fsq.h:40, `RXspp`
/// fsq.cxx:273).
pub const SAMPLE_RATE: f32 = 12000.0;
pub const SPP: usize = 10;

/// One of FSQ's eight picture `image_mode`s (fsq.cxx:876-902). Each fixes a size,
/// a colour/grey flag, and the on-air mode char.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsqPicMode {
    Small,         // 'S' 160×120 colour (image_mode 1)
    Large,         // 'L' 320×240 colour (image_mode 0)
    VgaGrey,       // 'F' 640×480 grey   (image_mode 2)
    VgaColor,      // 'V' 640×480 colour (image_mode 3)
    PortraitColor, // 'P' 240×300 colour (image_mode 4)
    PortraitGrey,  // 'p' 240×300 grey   (image_mode 5)
    MiniColor,     // 'M' 120×150 colour (image_mode 6)
    MiniGrey,      // 'm' 120×150 grey   (image_mode 7)
}

impl FsqPicMode {
    pub fn dims(self) -> (u32, u32) {
        match self {
            FsqPicMode::Small => (160, 120),
            FsqPicMode::Large => (320, 240),
            FsqPicMode::VgaGrey | FsqPicMode::VgaColor => (640, 480),
            FsqPicMode::PortraitColor | FsqPicMode::PortraitGrey => (240, 300),
            FsqPicMode::MiniColor | FsqPicMode::MiniGrey => (120, 150),
        }
    }
    /// True for the grey `image_mode`s (2, 5, 7 → chars `F`, `p`, `m`).
    pub fn color(self) -> bool {
        !matches!(self, FsqPicMode::VgaGrey | FsqPicMode::PortraitGrey | FsqPicMode::MiniGrey)
    }
    /// The on-air mode char. ref: fsq-pic.cxx:369-378 / fsq.cxx:876-902.
    fn mode_char(self) -> char {
        match self {
            FsqPicMode::Small => 'S',
            FsqPicMode::Large => 'L',
            FsqPicMode::VgaGrey => 'F',
            FsqPicMode::VgaColor => 'V',
            FsqPicMode::PortraitColor => 'P',
            FsqPicMode::PortraitGrey => 'p',
            FsqPicMode::MiniColor => 'M',
            FsqPicMode::MiniGrey => 'm',
        }
    }
}

/// A parsed FSQ picture header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsqPicHeader {
    pub width: u32,
    pub height: u32,
    pub color: bool,
    /// The 0..7 `image_mode` index (fsq.cxx:876-902).
    pub image_mode: u8,
}

/// Build the TX picture header token `"% X"` for a mode. This is the directed
/// payload FSQ appends before the raster; the enclosing `to_call` framing is the
/// directed-protocol layer's concern. ref: fsq-pic.cxx:369-378.
pub fn header(mode: FsqPicMode) -> String {
    format!("% {}", mode.mode_char())
}

/// Parse an FSQ picture header from a directed message payload. Mirrors
/// `parse_pcnt`, which is reached on the `%` trigger and reads the mode char at
/// `rx_text[2]` (i.e. after the `"% "` token). An unrecognised char yields
/// `None`. ref: fsq.cxx:603/876-902.
pub fn parse_header(window: &str) -> Option<FsqPicHeader> {
    let idx = window.find("% ")?;
    let c = window.as_bytes().get(idx + 2).copied()? as char;
    let (width, height, image_mode) = match c {
        'L' => (320, 240, 0u8),
        'S' => (160, 120, 1),
        'F' => (640, 480, 2),
        'V' => (640, 480, 3),
        'P' => (240, 300, 4),
        'p' => (240, 300, 5),
        'M' => (120, 150, 6),
        'm' => (120, 150, 7),
        _ => return None,
    };
    let color = !matches!(image_mode, 2 | 5 | 7);
    Some(FsqPicHeader { width, height, color, image_mode })
}

/// Build the shared pixel-FSK codec configured for FSQ pictures: 12 kHz,
/// `FsqLinear` scaling (`fc−200 + px·1.5`), BT.601 luma, **B→G→R** planes.
pub fn codec(carrier_hz: f32) -> PictureCodec {
    PictureCodec {
        samplerate: SAMPLE_RATE,
        carrier_hz,
        reverse: false, // FSQ pictures never flip the sideband.
        scale: PixelScale::FsqLinear,
        luma: LumaKind::Std,
        order: PlaneOrder::Bgr,
        label: "fsq-pic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::picture::{color_tx_raster, PlaneOrder};

    const VECTORS: &str = include_str!("../../tests/vectors/fsqpic.json");

    fn str_field(line: &str, key: &str) -> String {
        let i = line.find(key).unwrap() + key.len();
        line[i..line[i..].find('"').unwrap() + i].to_string()
    }
    fn num_field(line: &str, key: &str) -> u32 {
        let i = line.find(key).unwrap() + key.len();
        line[i..].split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()).unwrap().parse().unwrap()
    }
    fn int_field(line: &str, key: &str) -> i32 {
        let i = line.find(key).unwrap() + key.len();
        let s: String = line[i..].chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
        s.parse().unwrap()
    }
    fn float_field(line: &str, key: &str) -> f64 {
        let i = line.find(key).unwrap() + key.len();
        let s: String =
            line[i..].chars().take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '.').collect();
        s.parse().unwrap()
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

    fn mode_for(w: u32, h: u32, grey: bool) -> FsqPicMode {
        [
            FsqPicMode::Small,
            FsqPicMode::Large,
            FsqPicMode::VgaGrey,
            FsqPicMode::VgaColor,
            FsqPicMode::PortraitColor,
            FsqPicMode::PortraitGrey,
            FsqPicMode::MiniColor,
            FsqPicMode::MiniGrey,
        ]
        .into_iter()
        .find(|m| m.dims() == (w, h) && m.color() != grey)
        .unwrap()
    }

    #[test]
    fn tx_header_matches_fldigi_vector() {
        for line in VECTORS.lines().filter(|l| l.contains("\"kind\":\"header\"")) {
            let (w, h) = (num_field(line, "\"w\":"), num_field(line, "\"h\":"));
            let grey = bool_field(line, "\"grey\":");
            let want = str_field(line, "\"s\":\"");
            assert_eq!(header(mode_for(w, h, grey)), want, "header build differs from fldigi");
        }
    }

    #[test]
    fn rx_parse_matches_fldigi_vector() {
        for line in VECTORS.lines().filter(|l| l.contains("\"kind\":\"parse\"")) {
            let c = str_field(line, "\"c\":\"");
            let ok = bool_field(line, "\"ok\":");
            // FSQ reads rx_text[2] after the "% " token.
            let window = format!("% {c}");
            let got = parse_header(&window);
            assert_eq!(got.is_some(), ok, "parse ok/{ok} differs for {c:?}");
            if ok {
                let g = got.unwrap();
                assert_eq!(g.width, num_field(line, "\"w\":"), "W for {c:?}");
                assert_eq!(g.height, num_field(line, "\"h\":"), "H for {c:?}");
                assert_eq!(g.color, bool_field(line, "\"color\":"), "colour for {c:?}");
                assert_eq!(g.image_mode as u32, num_field(line, "\"mode\":"), "mode for {c:?}");
            }
        }
    }

    // Bit-exact quantiser KAT: the FsqLinear TX deviation and RX byte match the
    // pinned fldigi affines exactly (they are deliberately non-inverse).
    #[test]
    fn quantiser_matches_fldigi_vector() {
        let scale = PixelScale::FsqLinear;
        for line in VECTORS.lines().filter(|l| l.contains("\"kind\":\"quant\"")) {
            let px = num_field(line, "\"px\":") as u8;
            let dev = float_field(line, "\"dev\":");
            let byte = int_field(line, "\"byte\":") as u8;
            assert!(
                (scale.tx_deviation_hz(px, false) - dev).abs() < 1e-9,
                "TX deviation for px={px} differs from fldigi"
            );
            assert_eq!(scale.rx_byte(dev, false), byte, "RX byte for px={px} differs from fldigi");
        }
    }

    #[test]
    fn color_plane_reorder_matches_fldigi_vector() {
        let line = VECTORS.lines().find(|l| l.contains("\"kind\":\"color_raster\"")).unwrap();
        let w = num_field(line, "\"w\":") as usize;
        let input = u8_array(line, "\"in\":[");
        let want = u8_array(line, "\"out\":[");
        assert_eq!(color_tx_raster(&input, w, PlaneOrder::Bgr), want);
    }

    fn test_codec() -> PictureCodec {
        codec(1500.0)
    }

    // Loopback (audio domain, tolerance — Doctrine §3) at FSQ's 12 kHz / spp=10.
    // The gate is that the decoded raster tracks the *pinned* quantiser output
    // (byte = rx_byte(tx_deviation(px)), ~6 low), not the input pixel — that
    // asymmetry is fldigi's real behaviour.
    #[test]
    fn grey_loopback_tracks_pinned_quantiser() {
        use crate::types::FramePayload;
        let scale = PixelScale::FsqLinear;
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
        // Expected = the pinned round trip of each grey (BT.601 of a grey ramp is
        // the ramp itself), through the non-inverse FSQ affines.
        let want: Vec<u8> = rgb
            .chunks_exact(3)
            .map(|p| scale.rx_byte(scale.tx_deviation_hz(p[0], false), false))
            .collect();
        let max_err =
            pixels.iter().zip(&want).map(|(&g, &e)| (g as i32 - e as i32).abs()).max().unwrap();
        assert!(max_err <= 10, "FSQ grey loopback max deviation from pinned quantiser {max_err} > 10");
    }

    #[test]
    fn color_loopback_recovers_bgr_planes() {
        use crate::types::FramePayload;
        let scale = PixelScale::FsqLinear;
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
        // Each channel tracks its pinned quantiser value in place (B→G→R survives).
        let want: Vec<u8> =
            rgb.iter().map(|&v| scale.rx_byte(scale.tx_deviation_hz(v, false), false)).collect();
        let max_err =
            pixels.iter().zip(&want).map(|(&g, &e)| (g as i32 - e as i32).abs()).max().unwrap();
        assert!(max_err <= 10, "FSQ colour loopback max deviation from pinned quantiser {max_err} > 10");
    }
}
