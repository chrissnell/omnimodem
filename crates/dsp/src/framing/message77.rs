//! WSJT-X 77-bit message codec (FT8/FT4/MSK144), ported byte-exact from
//! `ft8_lib` (`ft8/message.c`, `ft8/text.c`).
//!
//! A 77-bit message is `[71 payload bits | 3-bit i3]` (with an `n3` sub-type
//! inside i3=0 messages), stored MSB-first big-endian across 10 bytes (the last
//! 3 bits of byte 9 are zero pad). This module reproduces ft8_lib's packing
//! bit-for-bit for the common message types:
//!   - i3 = 1 / 2: standard `Std Std Grid` (two 28+1-bit callsigns + 16-bit
//!     grid/report). i3=2 carries a `/P` suffix.
//!   - i3 = 0, n3 = 0: free text (13 chars over the 42-symbol FULL table).
//!   - i3 = 4: nonstandard call (one call hashed to 12 bits, the other packed to
//!     58 bits over ALPHANUM_SPACE_SLASH).
//!
//! The callsign hash (`save_callsign` in ft8_lib) packs the call into a 58-bit
//! base-38 integer and computes `n22 = (47055833459 * n58) >> 42 & 0x3FFFFF`;
//! `n12 = n22 >> 10`, `n10 = n22 >> 12`. A shared table maps n22 back to the
//! call so hashed forms round-trip after the full call has been seen.

use std::collections::HashMap;
use std::sync::Mutex;

const NTOKENS: u32 = 2063592;
const MAX22: u32 = 4194304; // 2^22
const MAXGRID4: u16 = 32400;

// Char tables from ft8_lib `text.h`.
const FULL: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./?"; // 42
const ALPHANUM_SPACE: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"; // 37
const ALPHANUM: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"; // 36
const NUMERIC: &[u8] = b"0123456789"; // 10
const LETTERS_SPACE: &[u8] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ"; // 27
const ALPHANUM_SPACE_SLASH: &[u8] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/"; // 38

fn nchar(c: u8, table: &[u8]) -> Option<i32> {
    table.iter().position(|&x| x == c).map(|p| p as i32)
}

fn charn(n: usize, table: &[u8]) -> u8 {
    table[n]
}

/// Shared callsign hash table (maps 22-bit hashes back to callsigns), mirroring
/// ft8_lib's `ftx_callsign_hash_interface_t`.
static HASHES: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

/// Compute n22/n12/n10 for a callsign and store it (ft8_lib `save_callsign`).
/// Returns the 22-bit hash, or `None` if the call has a char outside the base.
fn save_callsign(callsign: &str) -> Option<u32> {
    let bytes = callsign.as_bytes();
    let mut n58: u64 = 0;
    let mut i = 0;
    while i < bytes.len() && i < 11 {
        let j = nchar(bytes[i], ALPHANUM_SPACE_SLASH)?;
        n58 = 38u64.wrapping_mul(n58).wrapping_add(j as u64);
        i += 1;
    }
    // Pad with trailing spaces (index 0) up to 11 chars.
    while i < 11 {
        n58 = 38u64.wrapping_mul(n58);
        i += 1;
    }
    let n22 = ((47055833459u64.wrapping_mul(n58)) >> (64 - 22)) & 0x3FFFFF;
    let n22 = n22 as u32;
    let mut g = HASHES.lock().unwrap();
    g.get_or_insert_with(HashMap::new).insert(n22, callsign.to_string());
    Some(n22)
}

fn lookup_callsign(hash: u32, bits: u8) -> Option<String> {
    let shift = 22 - bits;
    let g = HASHES.lock().unwrap();
    g.as_ref()?
        .iter()
        .find(|(k, _)| (*k >> shift) == hash)
        .map(|(_, v)| v.clone())
}

/// Public hash helpers (n22 and its truncations).
pub fn hash22(call: &str) -> u32 {
    let bytes = call.as_bytes();
    let mut n58: u64 = 0;
    let mut i = 0;
    while i < bytes.len() && i < 11 {
        let j = nchar(bytes[i], ALPHANUM_SPACE_SLASH).unwrap_or(0);
        n58 = 38u64.wrapping_mul(n58).wrapping_add(j as u64);
        i += 1;
    }
    while i < 11 {
        n58 = 38u64.wrapping_mul(n58);
        i += 1;
    }
    (((47055833459u64.wrapping_mul(n58)) >> (64 - 22)) & 0x3FFFFF) as u32
}

pub fn hash12(call: &str) -> u32 {
    hash22(call) >> 10
}

pub fn hash10(call: &str) -> u32 {
    hash22(call) >> 12
}

// Small string predicates mirroring ft8_lib `text.c`.
fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}
fn is_letter(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

/// ft8_lib `parse_cq_modifier`: returns the numeric value for "CQ nnn" or
/// "CQ a[bcd]", else `None`. `s` is the full message starting with "CQ ".
fn parse_cq_modifier(s: &[u8]) -> Option<i32> {
    let mut nnum = 0;
    let mut nlet = 0;
    let mut m: i32 = 0;
    let mut i = 3;
    while i < 8 && i < s.len() {
        let c = s[i];
        if c == b' ' {
            break;
        } else if is_digit(c) {
            nnum += 1;
        } else if is_letter(c) {
            nlet += 1;
            m = 27 * m + (c - b'A' + 1) as i32;
        } else {
            return None;
        }
        i += 1;
    }
    if nnum == 3 && nlet == 0 {
        // atoi of the 3 digits after "CQ "
        let digits = std::str::from_utf8(&s[3..6]).ok()?;
        digits.parse::<i32>().ok()
    } else if nnum == 0 && nlet <= 4 {
        Some(1000 + m)
    } else {
        None
    }
}

/// ft8_lib `pack_basecall`: pack a standard base call into a 28-bit integer, or
/// `None` if it is not a standard callsign. `length` excludes any /P or /R.
fn pack_basecall(callsign: &str, length: usize) -> Option<i32> {
    if length <= 2 {
        return None;
    }
    let src = callsign.as_bytes();
    let mut c6 = [b' '; 6];

    if callsign.starts_with("3DA0") && length > 4 && length <= 7 {
        // Swaziland: 3DA0XYZ -> 3D0XYZ
        c6[..3].copy_from_slice(b"3D0");
        c6[3..3 + (length - 4)].copy_from_slice(&src[4..length]);
    } else if callsign.starts_with("3X") && length >= 3 && is_letter(src[2]) && length <= 7 {
        // Guinea: 3XA0XYZ -> QA0XYZ
        c6[0] = b'Q';
        c6[1..1 + (length - 2)].copy_from_slice(&src[2..length]);
    } else if length >= 3 && is_digit(src[2]) && length <= 6 {
        // AB0XYZ
        c6[..length].copy_from_slice(&src[..length]);
    } else if length >= 2 && is_digit(src[1]) && length <= 5 {
        // A0XYZ -> " A0XYZ"
        c6[1..1 + length].copy_from_slice(&src[..length]);
    }

    let i0 = nchar(c6[0], ALPHANUM_SPACE)?;
    let i1 = nchar(c6[1], ALPHANUM)?;
    let i2 = nchar(c6[2], NUMERIC)?;
    let i3 = nchar(c6[3], LETTERS_SPACE)?;
    let i4 = nchar(c6[4], LETTERS_SPACE)?;
    let i5 = nchar(c6[5], LETTERS_SPACE)?;

    let mut n = i0;
    n = n * 36 + i1;
    n = n * 10 + i2;
    n = n * 27 + i3;
    n = n * 27 + i4;
    n = n * 27 + i5;
    Some(n)
}

/// ft8_lib `pack28`: pack a special token, 22-bit hash, or base call into a
/// 28-bit value. Returns the value and sets `ip` (the /R or /P suffix flag).
fn pack28(callsign: &str) -> Option<(i32, u8)> {
    if callsign == "DE" {
        return Some((0, 0));
    }
    if callsign == "QRZ" {
        return Some((1, 0));
    }
    if callsign == "CQ" {
        return Some((2, 0));
    }

    let length = callsign.len();
    if callsign.starts_with("CQ ") && length < 8 {
        let v = parse_cq_modifier(callsign.as_bytes())?;
        return Some((3 + v, 0));
    }

    let mut ip = 0u8;
    let mut length_base = length;
    if callsign.ends_with("/P") || callsign.ends_with("/R") {
        ip = 1;
        length_base = length - 2;
    }

    if let Some(n28) = pack_basecall(callsign, length_base) {
        // Standard callsign with optional /P or /R suffix.
        save_callsign(callsign)?;
        return Some((NTOKENS as i32 + MAX22 as i32 + n28, ip));
    }

    if (3..=11).contains(&length) {
        // Nonstandard call: 22-bit hash.
        let n22 = save_callsign(callsign)?;
        return Some((NTOKENS as i32 + n22 as i32, 0));
    }

    None
}

/// ft8_lib `unpack28`: turn a 28-bit value + suffix flag back into a callsign.
fn unpack28(n28: u32, ip: u8, i3: u8) -> Option<String> {
    if n28 < NTOKENS {
        if n28 <= 2 {
            return Some(match n28 {
                0 => "DE",
                1 => "QRZ",
                _ => "CQ",
            }
            .to_string());
        }
        if n28 <= 1002 {
            return Some(format!("CQ {:03}", n28 - 3));
        }
        if n28 <= 532443 {
            let mut n = n28 - 1003;
            let mut aaaa = [0u8; 4];
            for i in (0..4).rev() {
                aaaa[i] = charn((n % 27) as usize, LETTERS_SPACE);
                n /= 27;
            }
            let tail = std::str::from_utf8(&aaaa).ok()?.trim_start();
            return Some(format!("CQ {tail}"));
        }
        return None;
    }

    let n28 = n28 - NTOKENS;
    if n28 < MAX22 {
        // 22-bit hash.
        let call = lookup_callsign(n28, 22).unwrap_or_else(|| "...".to_string());
        return Some(format!("<{call}>"));
    }

    let mut n = n28 - MAX22;
    let mut c = [0u8; 6];
    c[5] = charn((n % 27) as usize, LETTERS_SPACE);
    n /= 27;
    c[4] = charn((n % 27) as usize, LETTERS_SPACE);
    n /= 27;
    c[3] = charn((n % 27) as usize, LETTERS_SPACE);
    n /= 27;
    c[2] = charn((n % 10) as usize, NUMERIC);
    n /= 10;
    c[1] = charn((n % 36) as usize, ALPHANUM);
    n /= 36;
    c[0] = charn((n % 37) as usize, ALPHANUM_SPACE);

    let raw = std::str::from_utf8(&c).ok()?;
    let mut result = if raw.starts_with("3D0") && c[3] != b' ' {
        format!("3DA0{}", raw[3..].trim())
    } else if c[0] == b'Q' && is_letter(c[1]) {
        format!("3X{}", raw[1..].trim())
    } else {
        raw.trim().to_string()
    };

    if result.len() < 3 {
        return None;
    }
    if ip != 0 {
        match i3 {
            1 => result.push_str("/R"),
            2 => result.push_str("/P"),
            _ => return None,
        }
    }
    save_callsign(&result);
    Some(result)
}

/// ft8_lib `pack58`: pack a (possibly bracketed) call into a 58-bit base-38
/// integer over ALPHANUM_SPACE_SLASH, and store the trimmed call in the hash
/// table.
fn pack58(callsign: &str) -> Option<u64> {
    let mut bytes = callsign.as_bytes();
    if bytes.first() == Some(&b'<') {
        bytes = &bytes[1..];
    }
    let mut result: u64 = 0;
    let mut c11: Vec<u8> = Vec::with_capacity(11);
    for &c in bytes {
        if c == b'<' || c11.len() >= 11 {
            break;
        }
        let j = nchar(c, ALPHANUM_SPACE_SLASH)?;
        result = result.wrapping_mul(38).wrapping_add(j as u64);
        c11.push(c);
    }
    let trimmed = std::str::from_utf8(&c11).ok()?;
    save_callsign(trimmed)?;
    Some(result)
}

/// ft8_lib `unpack58`: reconstruct a call from a 58-bit value, store it.
fn unpack58(mut n58: u64) -> String {
    let mut c11 = [0u8; 11];
    for i in (0..11).rev() {
        c11[i] = charn((n58 % 38) as usize, ALPHANUM_SPACE_SLASH);
        n58 /= 38;
    }
    let call = std::str::from_utf8(&c11).unwrap_or("").trim().to_string();
    if call.len() >= 3 {
        save_callsign(&call);
    }
    call
}

/// ft8_lib `packgrid`: 4-char grid / report / token into a 16-bit field.
fn packgrid(grid4: &str) -> u16 {
    if grid4.is_empty() {
        return MAXGRID4 + 1;
    }
    if grid4 == "RRR" {
        return MAXGRID4 + 2;
    }
    if grid4 == "RR73" {
        return MAXGRID4 + 3;
    }
    if grid4 == "73" {
        return MAXGRID4 + 4;
    }

    let b = grid4.as_bytes();
    if b.len() == 4
        && (b'A'..=b'R').contains(&b[0])
        && (b'A'..=b'R').contains(&b[1])
        && is_digit(b[2])
        && is_digit(b[3])
    {
        let mut g = (b[0] - b'A') as u16;
        g = g * 18 + (b[1] - b'A') as u16;
        g = g * 10 + (b[2] - b'0') as u16;
        g = g * 10 + (b[3] - b'0') as u16;
        return g;
    }

    // Report: +dd / -dd / R+dd / R-dd
    if b[0] == b'R' {
        let dd = dd_to_int(&grid4[1..]);
        let irpt = (35 + dd) as u16;
        (MAXGRID4 + irpt) | 0x8000
    } else {
        let dd = dd_to_int(grid4);
        let irpt = (35 + dd) as u16;
        MAXGRID4 + irpt
    }
}

/// ft8_lib `dd_to_int` (width 3): parse a signed 2-digit report.
fn dd_to_int(s: &str) -> i32 {
    let b = s.as_bytes();
    if b.is_empty() {
        return 0;
    }
    let (negative, mut i) = match b[0] {
        b'-' => (true, 1),
        b'+' => (false, 1),
        _ => (false, 0),
    };
    let mut result = 0i32;
    while i < b.len() && i < 3 {
        if !is_digit(b[i]) {
            break;
        }
        result = result * 10 + (b[i] - b'0') as i32;
        i += 1;
    }
    if negative {
        -result
    } else {
        result
    }
}

/// ft8_lib `unpackgrid`.
fn unpackgrid(igrid4: u16, ir: u8) -> String {
    if igrid4 <= MAXGRID4 {
        let mut prefix = String::new();
        if ir > 0 {
            prefix.push_str("R ");
        }
        let mut n = igrid4;
        let mut g = [0u8; 4];
        g[3] = b'0' + (n % 10) as u8;
        n /= 10;
        g[2] = b'0' + (n % 10) as u8;
        n /= 10;
        g[1] = b'A' + (n % 18) as u8;
        n /= 18;
        g[0] = b'A' + (n % 18) as u8;
        prefix.push_str(std::str::from_utf8(&g).unwrap());
        prefix
    } else {
        let irpt = igrid4 - MAXGRID4;
        match irpt {
            1 => String::new(),
            2 => "RRR".to_string(),
            3 => "RR73".to_string(),
            4 => "73".to_string(),
            _ => {
                let mut s = String::new();
                if ir > 0 {
                    s.push('R');
                }
                let val = irpt as i32 - 35;
                s.push_str(&format!("{val:+03}"));
                s
            }
        }
    }
}

/// ft8_lib `ftx_message_encode_std` (i3 = 1 or 2).
fn encode_std(call_to: &str, call_de: &str, extra: &str) -> Option<[u8; 10]> {
    let (n28a, ipa) = pack28(call_to)?;
    let (n28b, ipb) = pack28(call_de)?;
    if n28a < 0 || n28b < 0 {
        return None;
    }

    let mut i3 = 1u8;
    if call_to.ends_with("/P") || call_de.ends_with("/P") {
        i3 = 2;
        if call_to.ends_with("/R") || call_de.ends_with("/R") {
            return None; // suffix error
        }
    }

    // Reject nonstandard /-call in call_de when call_to is CQ — needs type 4.
    let icq = call_to == "CQ" || call_to.starts_with("CQ ");
    if let Some(pos) = call_de.find('/') {
        if pos >= 2 && icq && call_de[pos..] != *"/P" && call_de[pos..] != *"/R" {
            return None;
        }
    }

    let igrid4 = packgrid(extra);

    let mut n29a = ((n28a as u32) << 1) | ipa as u32;
    let n29b = ((n28b as u32) << 1) | ipb as u32;
    if call_to.ends_with("/R") {
        n29a |= 1;
    } else if call_to.ends_with("/P") {
        n29a |= 1;
        i3 = 2;
    }

    let mut p = [0u8; 10];
    p[0] = (n29a >> 21) as u8;
    p[1] = (n29a >> 13) as u8;
    p[2] = (n29a >> 5) as u8;
    p[3] = ((n29a << 3) as u8) | (n29b >> 26) as u8;
    p[4] = (n29b >> 18) as u8;
    p[5] = (n29b >> 10) as u8;
    p[6] = (n29b >> 2) as u8;
    p[7] = ((n29b << 6) as u8) | (igrid4 >> 10) as u8;
    p[8] = (igrid4 >> 2) as u8;
    p[9] = ((igrid4 << 6) as u8) | (i3 << 3);
    Some(p)
}

/// ft8_lib `ftx_message_encode_nonstd` (i3 = 4).
fn encode_nonstd(call_to: &str, call_de: &str, extra: &str) -> Option<[u8; 10]> {
    let i3 = 4u8;
    let icq = call_to == "CQ" || call_to.starts_with("CQ ");

    if !icq && call_to.len() < 3 {
        return None;
    }
    if call_de.len() < 3 {
        return None;
    }

    let iflip: u8;
    let mut n12: u16 = 0;
    let call58: &str;

    if !icq {
        // call_de is plain-text unless it is the bracketed (hashed) one.
        iflip = if call_de.starts_with('<') && call_de.ends_with('>') {
            1
        } else {
            0
        };
        let (call12, c58) = if iflip == 0 {
            (call_to, call_de)
        } else {
            (call_de, call_to)
        };
        let h = save_callsign(call12.trim_start_matches('<').trim_end_matches('>'))?;
        n12 = (h >> 10) as u16;
        call58 = c58;
    } else {
        iflip = 0;
        call58 = call_de;
    }

    let n58 = pack58(call58)?;

    let nrpt: u8 = if icq {
        0
    } else if extra == "RRR" {
        1
    } else if extra == "RR73" {
        2
    } else if extra == "73" {
        3
    } else {
        0
    };
    let icq_bit = icq as u8;

    let mut p = [0u8; 10];
    p[0] = (n12 >> 4) as u8;
    p[1] = ((n12 << 4) as u8) | (n58 >> 54) as u8;
    p[2] = (n58 >> 46) as u8;
    p[3] = (n58 >> 38) as u8;
    p[4] = (n58 >> 30) as u8;
    p[5] = (n58 >> 22) as u8;
    p[6] = (n58 >> 14) as u8;
    p[7] = (n58 >> 6) as u8;
    p[8] = ((n58 << 2) as u8) | (iflip << 1) | (nrpt >> 1);
    p[9] = (nrpt << 7) | (icq_bit << 6) | (i3 << 3);
    Some(p)
}

/// ft8_lib `ftx_message_encode_free` + `ftx_message_encode_telemetry` (i3 = 0).
fn encode_free(text: &str) -> Option<[u8; 10]> {
    if text.len() > 13 {
        return None;
    }
    let tb = text.as_bytes();
    let mut b71 = [0u8; 9];
    for idx in 0..13 {
        let c = if idx < tb.len() { tb[idx] } else { b' ' };
        let cid = nchar(c, FULL)?;
        let mut rem: u16 = cid as u16;
        for i in (0..9).rev() {
            rem += b71[i] as u16 * 42;
            b71[i] = (rem & 0xff) as u8;
            rem >>= 8;
        }
    }
    // encode_telemetry: shift b71 left 1 bit into payload.
    let mut p = [0u8; 10];
    let mut carry = 0u8;
    for i in (0..9).rev() {
        p[i] = (b71[i] << 1) | (carry >> 7);
        carry = b71[i] & 0x80;
    }
    p[9] = 0; // i3.n3 = 0.0
    Some(p)
}

/// Tokenize like ft8_lib `copy_token`: whitespace-delimited, merging runs.
fn tokens(s: &str) -> Vec<&str> {
    s.split(' ').filter(|t| !t.is_empty()).collect()
}

/// Pack a WSJT-X message string to 77 bits (10 bytes), dispatching exactly like
/// ft8_lib `ftx_message_encode`: ≤3 tokens → try std, then nonstd; else free.
pub fn pack77(message: &str) -> [u8; 10] {
    let msg = message;
    let (call_to, call_de, extra, leftover);

    if let Some(rest) = msg.strip_prefix("CQ ") {
        let toks = tokens(rest);
        if let Some(v) = parse_cq_modifier(msg.as_bytes()) {
            let _ = v;
            // "CQ nnn" / "CQ a[bcd]" is a single call_to token.
            call_to = format!("CQ {}", toks.first().copied().unwrap_or(""));
            call_de = toks.get(1).copied().unwrap_or("").to_string();
            extra = toks.get(2).copied().unwrap_or("").to_string();
            leftover = toks.len() > 3;
        } else {
            call_to = "CQ".to_string();
            call_de = toks.first().copied().unwrap_or("").to_string();
            extra = toks.get(1).copied().unwrap_or("").to_string();
            leftover = toks.len() > 2;
        }
    } else {
        let toks = tokens(msg);
        call_to = toks.first().copied().unwrap_or("").to_string();
        call_de = toks.get(1).copied().unwrap_or("").to_string();
        extra = toks.get(2).copied().unwrap_or("").to_string();
        leftover = toks.len() > 3;
    }

    if !leftover {
        if let Some(p) = encode_std(&call_to, &call_de, &extra) {
            return p;
        }
        if let Some(p) = encode_nonstd(&call_to, &call_de, &extra) {
            return p;
        }
    }
    encode_free(message).unwrap_or([0u8; 10])
}

fn get_i3(p: &[u8; 10]) -> u8 {
    (p[9] >> 3) & 0x07
}

fn get_n3(p: &[u8; 10]) -> u8 {
    ((p[8] << 2) & 0x04) | ((p[9] >> 6) & 0x03)
}

/// ft8_lib `ftx_message_decode_std`.
fn decode_std(p: &[u8; 10]) -> String {
    let n29a = ((p[0] as u32) << 21)
        | ((p[1] as u32) << 13)
        | ((p[2] as u32) << 5)
        | ((p[3] as u32) >> 3);
    let n29b = (((p[3] & 0x07) as u32) << 26)
        | ((p[4] as u32) << 18)
        | ((p[5] as u32) << 10)
        | ((p[6] as u32) << 2)
        | ((p[7] as u32) >> 6);
    let ir = (p[7] & 0x20) >> 5;
    let igrid4 = (((p[7] & 0x1F) as u16) << 10) | ((p[8] as u16) << 2) | ((p[9] as u16) >> 6);
    let i3 = get_i3(p);

    let call_to = match unpack28(n29a >> 1, (n29a & 1) as u8, i3) {
        Some(s) => s,
        None => return String::new(),
    };
    let call_de = match unpack28(n29b >> 1, (n29b & 1) as u8, i3) {
        Some(s) => s,
        None => return String::new(),
    };
    let extra = unpackgrid(igrid4, ir);
    join3(&call_to, &call_de, &extra)
}

/// ft8_lib `ftx_message_decode_nonstd`.
fn decode_nonstd(p: &[u8; 10]) -> String {
    let n12 = (((p[0] as u16) << 4) | ((p[1] as u16) >> 4)) & 0x0FFF;
    let n58 = (((p[1] & 0x0F) as u64) << 54)
        | ((p[2] as u64) << 46)
        | ((p[3] as u64) << 38)
        | ((p[4] as u64) << 30)
        | ((p[5] as u64) << 22)
        | ((p[6] as u64) << 14)
        | ((p[7] as u64) << 6)
        | ((p[8] as u64) >> 2);
    let iflip = (p[8] >> 1) & 0x01;
    let nrpt = ((p[8] & 0x01) << 1) | (p[9] >> 7);
    let icq = (p[9] >> 6) & 0x01;

    let call_decoded = unpack58(n58);
    let call_3 = lookup_callsign(n12 as u32, 12)
        .map(|c| format!("<{c}>"))
        .unwrap_or_else(|| "<...>".to_string());

    let (call_1, call_2) = if iflip != 0 {
        (call_decoded.clone(), call_3.clone())
    } else {
        (call_3.clone(), call_decoded.clone())
    };

    let (call_to, extra) = if icq == 0 {
        let e = match nrpt {
            1 => "RRR",
            2 => "RR73",
            3 => "73",
            _ => "",
        };
        (call_1, e.to_string())
    } else {
        ("CQ".to_string(), String::new())
    };
    join3(&call_to, &call_2, &extra)
}

/// ft8_lib `ftx_message_decode_free`.
fn decode_free(p: &[u8; 10]) -> String {
    // decode_telemetry: shift payload right 1 bit.
    let mut b71 = [0u8; 9];
    let mut carry = 0u8;
    for i in 0..9 {
        b71[i] = (carry << 7) | (p[i] >> 1);
        carry = p[i] & 0x01;
    }
    let mut c14 = [b' '; 13];
    for slot in c14.iter_mut().rev() {
        let mut rem: u16 = 0;
        for byte in b71.iter_mut() {
            rem = (rem << 8) | *byte as u16;
            *byte = (rem / 42) as u8;
            rem %= 42;
        }
        *slot = charn(rem as usize, FULL);
    }
    std::str::from_utf8(&c14).unwrap_or("").trim().to_string()
}

fn join3(f1: &str, f2: &str, f3: &str) -> String {
    let mut out = f1.to_string();
    if !f2.is_empty() {
        out.push(' ');
        out.push_str(f2);
    }
    if !f3.is_empty() {
        out.push(' ');
        out.push_str(f3);
    }
    out
}

/// Unpack a 77-bit message (10 bytes) to its text form, routing on i3/n3.
pub fn unpack77(bytes: &[u8; 10]) -> String {
    match get_i3(bytes) {
        1 | 2 => decode_std(bytes),
        4 => decode_nonstd(bytes),
        0 if get_n3(bytes) == 0 => decode_free(bytes),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(p: &[u8; 10]) -> String {
        p.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn byte_exact_with_ft8_lib() {
        let cases = [
            ("CQ K1ABC FN42", "000000204def1a8a1988"),
            ("W9XYZ K1ABC FN42", "0c293b804def1a8a1988"),
            ("K1ABC W9XYZ RR73", "09bde3506149dc1fa4c8"),
            ("CQ N0CALL EM48", "000000201e5292084008"),
            ("HELLO WORLD", "039ddad02b9ddb1fa448"),
            ("TEST 123", "05b96a609f51de9fa448"),
            // i3 = 4 nonstandard-call form (reference: ft8_lib ftx_message_encode).
            ("CQ PJ4ABC/MM", "000001a3a1224cfb3460"),
        ];
        for (msg, expected) in cases {
            assert_eq!(hex(&pack77(msg)), expected, "payload mismatch for {msg}");
        }
    }

    #[test]
    fn vectors_file_matches() {
        let raw = include_str!("../../tests/vectors/ft8_reference.json");
        // Minimal extraction: find each {"msg":"...","payload":"..."} pair.
        for line in raw.lines() {
            let Some(mi) = line.find("\"msg\":\"") else { continue };
            let ms = &line[mi + 7..];
            let me = ms.find('"').unwrap();
            let msg = &ms[..me];
            let Some(pi) = line.find("\"payload\":\"") else { continue };
            let ps = &line[pi + 11..];
            let pe = ps.find('"').unwrap();
            let payload = &ps[..pe];
            assert_eq!(hex(&pack77(msg)), payload, "payload mismatch for {msg}");
        }
    }

    #[test]
    fn standard_roundtrip() {
        // Calls that pack as standard base calls round-trip identically.
        for m in ["CQ K1ABC FN42", "W9XYZ K1ABC FN42", "K1ABC W9XYZ RR73", "PJ4ABC K1ABC FN42"] {
            assert_eq!(unpack77(&pack77(m)), m, "roundtrip failed for {m}");
        }
    }

    #[test]
    fn nonstandard_hash_roundtrip() {
        // Nonstandard tokens are sent as 22-bit hashes and decode bracketed,
        // exactly like ft8_lib (verified against the reference decoder).
        assert_eq!(unpack77(&pack77("HELLO WORLD")), "<HELLO> <WORLD>");
        assert_eq!(unpack77(&pack77("TEST 123")), "<TEST> 123");
        assert_eq!(unpack77(&pack77("CQ N0CALL EM48")), "CQ <N0CALL> EM48");
    }

    #[test]
    fn hashed_callsign_roundtrip() {
        // Prime the table with the full call, then the 22-bit hash token in a
        // standard message resolves back to the bracketed call.
        let _ = pack77("PJ4ABC/MM K1ABC RR73");
        let out = unpack77(&pack77("PJ4ABC/MM K1ABC RR73"));
        assert_eq!(out, "<PJ4ABC/MM> K1ABC RR73", "hashed roundtrip: got {out}");

        // True i3 = 4 nonstandard-call form (CQ + slashed call).
        let m = "CQ PJ4ABC/MM";
        assert_eq!(get_i3(&pack77(m)), 4);
        assert_eq!(unpack77(&pack77(m)), m, "nonstd roundtrip: got {}", unpack77(&pack77(m)));
    }

    #[test]
    fn type_field_routes_by_i3() {
        assert_eq!(get_i3(&pack77("CQ K1ABC FN42")), 1);
        // "HELLO WORLD" packs as i3=1 with both tokens hashed (ft8_lib behavior).
        assert_eq!(get_i3(&pack77("HELLO WORLD")), 1);
        assert_eq!(get_i3(&pack77("CQ PJ4ABC/MM")), 4);
    }

    #[test]
    fn hash_layers() {
        let h22 = hash22("K1ABC");
        assert_eq!(hash12("K1ABC"), h22 >> 10);
        assert_eq!(hash10("K1ABC"), h22 >> 12);
        assert_ne!(hash22("K1ABC"), hash22("W9XYZ"));
    }

    #[test]
    fn pack28_special_tokens() {
        assert_eq!(pack28("DE").map(|(n, _)| n), Some(0));
        assert_eq!(pack28("QRZ").map(|(n, _)| n), Some(1));
        assert_eq!(pack28("CQ").map(|(n, _)| n), Some(2));
    }
}

/// Legacy WSJT-X packers: 72-bit (JT65/JT9) and 50-bit (WSPR/FST4W). Distinct
/// from the modern 77-bit codec above. Bit order MSB-first (WSJT-X convention).
///
/// The 72-bit JT65/JT9 message is `nc1(28) | nc2(28) | ng(16)` — two standard
/// base callsigns plus a 16-bit grid/report field, reusing this module's
/// [`super::pack_basecall`]/[`super::packgrid`]. The 50-bit WSPR message is the
/// genuine `nc(28) | (ng4*128 + power+64)(22)` layout from WSJT-X `wsprd`. Only
/// the common standard-callsign forms are handled (special tokens like `CQ`,
/// hashed nonstandard calls, and the JT65 report tokens beyond a plain grid are
/// out of scope here — they round-trip through the modern 77-bit codec instead).
pub mod legacy {
    use super::{
        charn, pack_basecall, packgrid, unpackgrid, ALPHANUM, ALPHANUM_SPACE, LETTERS_SPACE,
        NUMERIC,
    };

    fn bits_msb(mut v: u128, nbits: usize) -> Vec<u8> {
        let mut out = vec![0u8; nbits];
        for slot in out.iter_mut().rev() {
            *slot = (v & 1) as u8;
            v >>= 1;
        }
        out
    }

    fn unbits_msb(bits: &[u8]) -> u128 {
        bits.iter().fold(0u128, |v, &b| (v << 1) | (b as u128 & 1))
    }

    /// Reverse of [`super::pack_basecall`]: a raw 28-bit base-call integer back to
    /// the trimmed callsign string.
    fn unpack_basecall(mut n: u32) -> Option<String> {
        let mut c = [0u8; 6];
        c[5] = charn((n % 27) as usize, LETTERS_SPACE);
        n /= 27;
        c[4] = charn((n % 27) as usize, LETTERS_SPACE);
        n /= 27;
        c[3] = charn((n % 27) as usize, LETTERS_SPACE);
        n /= 27;
        c[2] = charn((n % 10) as usize, NUMERIC);
        n /= 10;
        c[1] = charn((n % 36) as usize, ALPHANUM);
        n /= 36;
        c[0] = charn((n % 37) as usize, ALPHANUM_SPACE);
        let call = std::str::from_utf8(&c).ok()?.trim().to_string();
        if call.len() < 3 {
            None
        } else {
            Some(call)
        }
    }

    /// Pack a standard `CALL1 CALL2 GRID` JT65/JT9 message to 72 bits.
    pub fn pack72(message: &str) -> Option<[u8; 72]> {
        let parts: Vec<&str> = message.split_whitespace().collect();
        if parts.len() != 3 {
            return None;
        }
        let n1 = pack_basecall(parts[0], parts[0].len())? as u128;
        let n2 = pack_basecall(parts[1], parts[1].len())? as u128;
        let ng = packgrid(parts[2]) as u128;
        if ng > 0xFFFF {
            return None;
        }
        let v = (n1 << 44) | (n2 << 16) | ng;
        bits_msb(v, 72).try_into().ok()
    }

    pub fn unpack72(bits: &[u8; 72]) -> Option<String> {
        let v = unbits_msb(bits);
        let ng = (v & 0xFFFF) as u16;
        let n2 = ((v >> 16) & 0x0FFF_FFFF) as u32;
        let n1 = ((v >> 44) & 0x0FFF_FFFF) as u32;
        let c1 = unpack_basecall(n1)?;
        let c2 = unpack_basecall(n2)?;
        let grid = unpackgrid(ng, 0);
        Some(format!("{c1} {c2} {grid}"))
    }

    /// Pack a WSPR `CALL GRID dBm` message to 50 bits (28-bit call + 22-bit
    /// grid/power), per WSJT-X `wsprd`. `grid4` is a 4-char Maidenhead locator,
    /// `dbm` a power 0..=60.
    pub fn pack50(call: &str, grid4: &str, dbm: u8) -> Option<[u8; 50]> {
        let n28 = pack_basecall(call, call.len())? as u128;
        let b = grid4.as_bytes();
        if b.len() != 4
            || !(b'A'..=b'R').contains(&b[0])
            || !(b'A'..=b'R').contains(&b[1])
            || !b[2].is_ascii_digit()
            || !b[3].is_ascii_digit()
            || dbm > 60
        {
            return None;
        }
        let a0 = (b[0] - b'A') as i64;
        let a1 = (b[1] - b'A') as i64;
        let c2 = (b[2] - b'0') as i64;
        let c3 = (b[3] - b'0') as i64;
        let ng = (179 - 10 * a0 - c2) * 180 + 10 * a1 + c3;
        let m = (ng * 128 + dbm as i64 + 64) as u128;
        let v = (n28 << 22) | m;
        bits_msb(v, 50).try_into().ok()
    }

    pub fn unpack50(bits: &[u8; 50]) -> Option<(String, String, u8)> {
        let v = unbits_msb(bits);
        let m = (v & 0x3F_FFFF) as i64;
        let n28 = (v >> 22) as u32;
        let call = unpack_basecall(n28)?;
        let dbm = (m % 128) - 64;
        let ng = m / 128;
        let hi = ng / 180;
        let lo = ng % 180;
        let a1 = lo / 10;
        let c3 = lo % 10;
        let rem = 179 - hi;
        let a0 = rem / 10;
        let c2 = rem % 10;
        if !(0..=17).contains(&a0) || !(0..=17).contains(&a1) || !(0..=60).contains(&dbm) {
            return None;
        }
        let grid = [
            b'A' + a0 as u8,
            b'A' + a1 as u8,
            b'0' + c2 as u8,
            b'0' + c3 as u8,
        ];
        Some((call, String::from_utf8(grid.to_vec()).ok()?, dbm as u8))
    }
}

#[cfg(test)]
mod legacy_tests {
    use super::legacy::*;

    #[test]
    fn jt65_message_round_trips() {
        let bits = pack72("K1ABC W9XYZ EN37").unwrap();
        assert_eq!(unpack72(&bits).unwrap(), "K1ABC W9XYZ EN37");
    }

    #[test]
    fn jt65_message_two_prefix_call_round_trips() {
        let bits = pack72("VK3ABC ZL2XYZ RF80").unwrap();
        assert_eq!(unpack72(&bits).unwrap(), "VK3ABC ZL2XYZ RF80");
    }

    #[test]
    fn wspr_message_round_trips() {
        let bits = pack50("K1ABC", "FN42", 37).unwrap();
        assert_eq!(unpack50(&bits).unwrap(), ("K1ABC".into(), "FN42".into(), 37));
    }

    #[test]
    fn wspr_power_and_grid_edges() {
        for (call, grid, dbm) in [("W9XYZ", "AA00", 0u8), ("G3ABC", "RR99", 60)] {
            let bits = pack50(call, grid, dbm).unwrap();
            assert_eq!(unpack50(&bits).unwrap(), (call.into(), grid.into(), dbm));
        }
    }

    #[test]
    fn wspr_rejects_bad_input() {
        assert!(pack50("K1ABC", "FN4", 37).is_none()); // short grid
        assert!(pack50("K1ABC", "FN42", 61).is_none()); // power too high
    }
}
