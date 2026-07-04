//! WSJT-X 77-bit message packing (the FT8/FT4/FST4/… source encoding), ported
//! from `wsjtx/lib/77bit/packjt77.f90`. Covers the **standard Type-1 message**
//! (`CALL CALL GRID`, `CALL CALL [R] report/RRR/RR73/73`, `CQ CALL [GRID]`) with
//! standard base callsigns and the `CQ`/`DE`/`QRZ` tokens — the overwhelming
//! majority of on-air traffic. Nonstandard/hashed calls, `/P` `/R` suffixes, and
//! the contest/telemetry/free-text types return `None` (future work).
//!
//! Bit-exact vs the reference `pack77` (KAT'd against `encode77`/`pack77_dump`;
//! see `scratch/refvectors/build_pack77.sh`). ref: packjt77.f90 pack28/pack77_1.

const A1: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"; // 37
const A2: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"; // 36
const A3: &[u8] = b"0123456789"; // 10
const A4: &[u8] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ"; // 27
const NTOKENS: u32 = 2_063_592;
const MAX22: u32 = 4_194_304;
const MAXGRID4: i32 = 32_400;

fn idx(tab: &[u8], c: u8) -> Option<u32> {
    tab.iter().position(|&t| t == c).map(|p| p as u32)
}

/// Pack a callsign or `CQ`/`DE`/`QRZ` token into 28 bits. Returns `None` for
/// anything this port doesn't yet handle (hashed/nonstandard calls, suffixes).
/// ref: packjt77.f90 `pack28` (special tokens + standard-call branch).
pub fn pack28(call: &str) -> Option<u32> {
    match call {
        "DE" => return Some(0),
        "QRZ" => return Some(1),
        "CQ" => return Some(2),
        _ => {}
    }
    let c = call.as_bytes();
    if call.contains('/') || call.contains('<') || !c.iter().all(|b| b.is_ascii_alphanumeric()) {
        return None; // nonstandard/hashed/suffix — not yet supported
    }
    // Call-area digit = last digit position (1-origin), scanning to position 2.
    let n = c.len();
    let mut iarea = 0usize; // 1-origin
    for i in (1..n).rev() {
        if c[i].is_ascii_digit() {
            iarea = i + 1; // 1-origin
            break;
        }
    }
    // Validate the standard-call shape (letters before area, area digit 2 or 3,
    // <=3 letters after). ref: pack28 iarea/nplet/npdig/nslet checks.
    if !(2..=3).contains(&iarea) {
        return None;
    }
    let (npdig, nplet) = c[..iarea - 1].iter().fold((0, 0), |(d, l), &b| {
        (d + b.is_ascii_digit() as usize, l + b.is_ascii_uppercase() as usize)
    });
    let nslet = c[iarea..].iter().filter(|b| b.is_ascii_uppercase()).count();
    if nplet == 0 || npdig >= iarea - 1 || nslet > 3 {
        return None;
    }
    // Right-justify into the 6-char field: iarea==2 prepends a space.
    let mut field = [b' '; 6];
    let call6: Vec<u8> = if iarea == 2 {
        std::iter::once(b' ').chain(c.iter().copied()).collect()
    } else {
        c.to_vec()
    };
    for (f, &b) in field.iter_mut().zip(call6.iter()) {
        *f = b;
    }
    let i1 = idx(A1, field[0])?;
    let i2 = idx(A2, field[1])?;
    let i3 = idx(A3, field[2])?;
    let i4 = idx(A4, field[3])?;
    let i5 = idx(A4, field[4])?;
    let i6 = idx(A4, field[5])?;
    let n28 = ((((i1 * 36 + i2) * 10 + i3) * 27 + i4) * 27 + i5) * 27 + i6;
    Some((n28 + NTOKENS + MAX22) & ((1 << 28) - 1))
}

fn is_grid4(w: &str) -> bool {
    let b = w.as_bytes();
    w.len() == 4
        && (b'A'..=b'R').contains(&b[0])
        && (b'A'..=b'R').contains(&b[1])
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
}

/// Encode the final `grid / report / RRR / RR73 / 73` token into `(ir, igrid4)`.
/// `has_r` is true when a standalone `R` precedes it (`CALL CALL R GRID`).
/// ref: packjt77.f90 pack77_1 (igrid4 / irpt logic).
fn encode_grid_report(tok: &str, has_r: bool) -> Option<(u32, i32)> {
    if is_grid4(tok) {
        let b = tok.as_bytes();
        let j1 = (b[0] - b'A') as i32 * 18 * 10 * 10;
        let j2 = (b[1] - b'A') as i32 * 10 * 10;
        let j3 = (b[2] - b'0') as i32 * 10;
        let j4 = (b[3] - b'0') as i32;
        return Some((has_r as u32, j1 + j2 + j3 + j4));
    }
    let rpt = |s: &str, ir: u32| -> Option<(u32, i32)> {
        let mut v: i32 = s.parse().ok()?;
        if (-50..=-31).contains(&v) {
            v += 101;
        }
        Some((ir, MAXGRID4 + v + 35))
    };
    match tok {
        "RRR" => Some((0, MAXGRID4 + 2)),
        "RR73" => Some((0, MAXGRID4 + 3)),
        "73" => Some((0, MAXGRID4 + 4)),
        _ if tok.starts_with('+') || tok.starts_with('-') => rpt(tok, 0),
        _ if tok.starts_with("R+") || tok.starts_with("R-") => rpt(&tok[1..], 1),
        _ => None,
    }
}

fn push_bits(out: &mut Vec<u8>, val: u32, nbits: usize) {
    for i in (0..nbits).rev() {
        out.push(((val >> i) & 1) as u8);
    }
}

/// Pack a standard Type-1 message string into 77 bits, or `None` if it is not a
/// standard message this port supports. ref: packjt77.f90 pack77_1.
pub fn pack77_standard(msg: &str) -> Option<[u8; 77]> {
    let words: Vec<&str> = msg.split_whitespace().collect();
    if words.len() < 2 || words.len() > 4 {
        return None;
    }
    let n28a = pack28(words[0])?;
    let n28b = pack28(words[1])?;
    let (ipa, ipb) = (0u32, 0u32); // /P /R suffixes unsupported for now

    let (ir, igrid4) = if words.len() == 2 {
        (0u32, MAXGRID4 + 1) // two calls, no grid: "CQ CALL"
    } else {
        // The grid/report is the last word; a bare "R" may precede it (nwords==4).
        let has_r = words.len() == 4 && words[2] == "R";
        if words.len() == 4 && !has_r {
            return None; // not a standard "CALL CALL R GRID"
        }
        encode_grid_report(words[words.len() - 1], has_r)?
    };

    let mut bits = Vec::with_capacity(77);
    push_bits(&mut bits, n28a, 28);
    push_bits(&mut bits, ipa, 1);
    push_bits(&mut bits, n28b, 28);
    push_bits(&mut bits, ipb, 1);
    push_bits(&mut bits, ir, 1);
    push_bits(&mut bits, igrid4 as u32, 15);
    push_bits(&mut bits, 1, 3); // i3 = 1 (standard)
    let mut out = [0u8; 77];
    out.copy_from_slice(&bits);
    Some(out)
}

/// Unpack a 28-bit callsign field back to its string (inverse of [`pack28`]).
/// Returns `None` for the hashed-call range (recovering it needs the runtime
/// hash table). ref: packjt77.f90 `unpack28`.
pub fn unpack28(n28: u32) -> Option<String> {
    if n28 < NTOKENS {
        return Some(match n28 {
            0 => "DE".to_string(),
            1 => "QRZ".to_string(),
            2 => "CQ".to_string(),
            n if n < 3 + 1000 => format!("CQ {:03}", n - 3), // CQ nnn (freq offset)
            n => {
                // CQ + up to 4 letters (base-27), right-justified.
                let mut m = n - 3 - 1000;
                let mut chars = [b' '; 4];
                for c in chars.iter_mut().rev() {
                    *c = A4[(m % 27) as usize];
                    m /= 27;
                }
                format!("CQ {}", String::from_utf8_lossy(&chars).trim())
            }
        });
    }
    if n28 < NTOKENS + MAX22 {
        return None; // hashed nonstandard call — needs the hash table
    }
    let mut n = n28 - NTOKENS - MAX22;
    let i6 = (n % 27) as usize;
    n /= 27;
    let i5 = (n % 27) as usize;
    n /= 27;
    let i4 = (n % 27) as usize;
    n /= 27;
    let i3 = (n % 10) as usize;
    n /= 10;
    let i2 = (n % 36) as usize;
    n /= 36;
    let i1 = (n % 37) as usize;
    let call = [A1[i1], A2[i2], A3[i3], A4[i4], A4[i5], A4[i6]];
    Some(String::from_utf8_lossy(&call).trim().to_string())
}

fn bits_to_u32(bits: &[u8]) -> u32 {
    bits.iter().fold(0u32, |a, &b| (a << 1) | b as u32)
}

/// Unpack a 77-bit standard Type-1 message back to its string, or `None` if it
/// is not a Type-1 message this port supports (i3 != 1, or a hashed call).
/// ref: packjt77.f90 `unpack77` (i3=1 branch).
pub fn unpack77_standard(bits: &[u8; 77]) -> Option<String> {
    if bits_to_u32(&bits[74..77]) != 1 {
        return None; // only i3 = 1 (standard) here
    }
    let n28a = bits_to_u32(&bits[0..28]);
    let n28b = bits_to_u32(&bits[29..57]);
    let ir = bits[58];
    let igrid4 = bits_to_u32(&bits[59..74]) as i32;
    let call1 = unpack28(n28a)?;
    let call2 = unpack28(n28b)?;

    if igrid4 < MAXGRID4 {
        // Maidenhead grid; a leading "R" is a separate word.
        let i = igrid4;
        let grid = [
            b'A' + (i / 1800) as u8,
            b'A' + ((i % 1800) / 100) as u8,
            b'0' + ((i % 100) / 10) as u8,
            b'0' + (i % 10) as u8,
        ];
        let grid = String::from_utf8_lossy(&grid).to_string();
        let r = if ir == 1 { "R " } else { "" };
        return Some(format!("{call1} {call2} {r}{grid}"));
    }
    let irpt = igrid4 - MAXGRID4;
    let tail = match irpt {
        1 => return Some(format!("{call1} {call2}")), // two calls, no grid
        2 => "RRR".to_string(),
        3 => "RR73".to_string(),
        4 => "73".to_string(),
        _ => {
            let mut r = irpt - 35;
            if (51..=70).contains(&r) {
                r -= 101; // reverse the pack-side -50..-31 wrap
            }
            let rp = if ir == 1 { "R" } else { "" };
            if r >= 0 {
                format!("{rp}+{r:02}")
            } else {
                format!("{rp}-{:02}", -r)
            }
        }
    };
    Some(format!("{call1} {call2} {tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(s: &str) -> [u8; 77] {
        let v: Vec<u8> = s.bytes().map(|c| c - b'0').collect();
        let mut a = [0u8; 77];
        a.copy_from_slice(&v);
        a
    }

    #[test]
    fn pack77_standard_matches_wsjtx_reference() {
        // Golden 77-bit encodings from the UNMODIFIED pack77 (packjt77.f90) via
        // scratch/refvectors/build_pack77.sh.
        let cases = [
            ("CQ K1ABC FN42", "00000000000000000000000000100000010011011110111100011010100010100001100110001"),
            ("K1ABC W9XYZ EN37", "00001001101111011110001101010000011000010100100111011100000010000101011001001"),
            ("W9XYZ K1ABC -11", "00001100001010010011101110000000010011011110111100011010100111111010101000001"),
            ("K1ABC W9XYZ R-09", "00001001101111011110001101010000011000010100100111011100001111111010101010001"),
            ("W9XYZ K1ABC RRR", "00001100001010010011101110000000010011011110111100011010100111111010010010001"),
            ("K1ABC W9XYZ RR73", "00001001101111011110001101010000011000010100100111011100000111111001110101001"),
            ("CQ W1AW", "00000000000000000000000000100000010111111111010101101000100111111010010001001"),
        ];
        for (msg, want) in cases {
            let got = pack77_standard(msg).unwrap_or_else(|| panic!("failed to pack {msg:?}"));
            assert_eq!(got, bits(want), "pack77 mismatch for {msg:?}");
        }
    }

    #[test]
    fn unpack77_matches_wsjtx_reference() {
        // (77-bit golden vector, canonical message) from unpack77 via
        // scratch/refvectors/build_unpack77.sh.
        let cases = [
            ("00000000000000000000000000100000010011011110111100011010100010100001100110001", "CQ K1ABC FN42"),
            ("00001100001010010011101110000000010011011110111100011010100111111010101000001", "W9XYZ K1ABC -11"),
            ("00001001101111011110001101010000011000010100100111011100001111111010101010001", "K1ABC W9XYZ R-09"),
            ("00001001101111011110001101010000011000010100100111011100000111111001110101001", "K1ABC W9XYZ RR73"),
            ("00000000000000000000000000100000010111111111010101101000100111111010010001001", "CQ W1AW"),
        ];
        for (b, want) in cases {
            assert_eq!(unpack77_standard(&bits(b)).as_deref(), Some(want), "unpack {b}");
        }
    }

    #[test]
    fn pack_unpack_round_trips() {
        for msg in [
            "CQ K1ABC FN42",
            "K1ABC W9XYZ EN37",
            "W9XYZ K1ABC -11",
            "K1ABC W9XYZ R-09",
            "W9XYZ K1ABC RRR",
            "K1ABC W9XYZ RR73",
            "CQ W1AW",
        ] {
            let packed = pack77_standard(msg).unwrap();
            assert_eq!(unpack77_standard(&packed).as_deref(), Some(msg), "round-trip {msg}");
        }
    }

    #[test]
    fn pack77_rejects_unsupported() {
        assert!(pack77_standard("K1ABC/P W9XYZ FN42").is_none()); // suffix
        assert!(pack77_standard("HELLO WORLD FROM ME NOW").is_none()); // too many words
        assert!(pack28("W1XYZ/R").is_none());
    }
}
