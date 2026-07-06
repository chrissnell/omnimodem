//! MFSK in-band picture sub-protocol — the header codec (T2).
//!
//! An MFSK picture transmission is announced in the text stream by a header the
//! receiver scans for: `"\nSending Pic:<W>x<H>[C][p<spp>];"`. `C` marks colour;
//! the optional `p<spp>` selects a faster 4- or 2-samples-per-pixel rate (absent
//! ⇒ 8). The pixel raster that follows is carried by the shared pixel-FSK codec
//! in [`super::picture`]. This module owns only the bit-exact header build/parse;
//! the modulator/demodulator that wire it onto the MFSK carrier are T4/T5.
//!
//! Reference: `fldigi/src/mfsk/mfsk-pic.cxx` (TX header builders :205-207,
//! :246-248) and `fldigi/src/mfsk/mfsk.cxx` (`check_picture_header` RX parser
//! :366-422), upstream 4.1.23 @ `61b97f413`. Golden vectors:
//! `tests/vectors/mfskpic.json` (driver `scratch/refvectors/build_mfskpic.sh`).

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

    #[test]
    fn header_round_trips_through_parse() {
        for &(w, h, color, spp) in &[(320, 240, true, 8), (160, 120, false, 4), (64, 64, true, 2)] {
            let hdr = header(w, h, color, spp);
            let p = parse_header(&hdr).expect("built header must parse");
            assert_eq!((p.width, p.height, p.color, p.rxspp), (w, h, color, spp));
        }
    }
}
