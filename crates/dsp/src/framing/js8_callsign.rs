//! JS8 callsign packing — the base-40 positional encoding behind the
//! heartbeat / compound / directed frame types.
//!
//! Port of `varicode.cpp` `packCallsign` / `unpackCallsign` (upstream
//! `js8call/js8call` @ a7ff1be). A standard callsign is normalised into the
//! 6-character field `[0-9A-Z ][0-9A-Z][0-9][A-Z ][A-Z ][A-Z ]` and packed
//! into a 22-bit value with positional weights `37·36·10·27·27·27`
//! (`nbasecall`); group calls (`@ALLCALL`, `@CQ`, …) and the incomplete-call
//! sentinel occupy the values just above `nbasecall`.
//!
//! The reference uses a `QRegularExpression` to find the aligned 6-char window;
//! since that pattern is a fixed positional character-class check, it is ported
//! here as a hand-rolled window scan (no regex dependency). Pack/unpack are
//! gated by round-trip; the on-air authority is the cross-decode gate.

/// Callsign/grid alphabet: index 0-9 = digits, 10-35 = A-Z, 36 = space,
/// 37 = `/`, 38 = `@`. ref: varicode.cpp:44 (`alphanumeric`).
const ALPHANUMERIC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ /@";

/// `nbasecall = 37·36·10·27·27·27`. ref: varicode.cpp:209.
pub const NBASECALL: u32 = 37 * 36 * 10 * 27 * 27 * 27;

/// Group / special base calls, stored as `(name, nbasecall + offset)`.
/// ref: varicode.cpp:214-266 (`basecalls`).
pub const BASECALLS: &[(&str, u32)] = &[
    ("<....>", 1),
    ("@ALLCALL", 2),
    ("@JS8NET", 3),
    ("@DX/NA", 4),
    ("@DX/SA", 5),
    ("@DX/EU", 6),
    ("@DX/AS", 7),
    ("@DX/AF", 8),
    ("@DX/OC", 9),
    ("@DX/AN", 10),
    ("@REGION/1", 11),
    ("@REGION/2", 12),
    ("@REGION/3", 13),
    ("@GROUP/0", 14),
    ("@GROUP/1", 15),
    ("@GROUP/2", 16),
    ("@GROUP/3", 17),
    ("@GROUP/4", 18),
    ("@GROUP/5", 19),
    ("@GROUP/6", 20),
    ("@GROUP/7", 21),
    ("@GROUP/8", 22),
    ("@GROUP/9", 23),
    ("@COMMAND", 24),
    ("@CONTROL", 25),
    ("@NET", 26),
    ("@NTS", 27),
    ("@RESERVE/0", 28),
    ("@RESERVE/1", 29),
    ("@RESERVE/2", 30),
    ("@RESERVE/3", 31),
    ("@RESERVE/4", 32),
    ("@APRSIS", 33),
    ("@RAGCHEW", 34),
    ("@JS8", 35),
    ("@EMCOMM", 36),
    ("@ARES", 37),
    ("@MARS", 38),
    ("@AMRRON", 39),
    ("@RACES", 40),
    ("@RAYNET", 41),
    ("@RADAR", 42),
    ("@SKYWARN", 43),
    ("@CQ", 44),
    ("@HB", 45),
    ("@QSO", 46),
    ("@QSOPARTY", 47),
    ("@CONTEST", 48),
    ("@FIELDDAY", 49),
    ("@SOTA", 50),
    ("@IOTA", 51),
    ("@POTA", 52),
    ("@QRP", 53),
    ("@QRO", 54),
];

fn idx(c: u8) -> i32 {
    ALPHANUMERIC.iter().position(|&a| a == c).map(|p| p as i32).unwrap_or(-1)
}

fn basecall_value(name: &str) -> Option<u32> {
    BASECALLS.iter().find(|(n, _)| *n == name).map(|(_, off)| NBASECALL + off)
}

/// Does the 6-byte window match `[0-9A-Z ][0-9A-Z][0-9][A-Z ][A-Z ][A-Z ]`?
/// ref: varicode.cpp:43 (`pack_callsign_pattern`).
fn window_matches(w: &[u8]) -> bool {
    let is_digit = |c: u8| c.is_ascii_digit();
    let is_alpha = |c: u8| c.is_ascii_uppercase();
    let is_alnum_sp = |c: u8| is_digit(c) || is_alpha(c) || c == b' ';
    let is_alpha_sp = |c: u8| is_alpha(c) || c == b' ';
    w.len() == 6
        && is_alnum_sp(w[0])
        && (is_digit(w[1]) || is_alpha(w[1]))
        && is_digit(w[2])
        && is_alpha_sp(w[3])
        && is_alpha_sp(w[4])
        && is_alpha_sp(w[5])
}

/// Leftmost 6-char window in `s` that matches the callsign pattern.
fn first_match(s: &[u8]) -> Option<[u8; 6]> {
    if s.len() < 6 {
        return None;
    }
    for start in 0..=s.len() - 6 {
        let w = &s[start..start + 6];
        if window_matches(w) {
            let mut out = [0u8; 6];
            out.copy_from_slice(w);
            return Some(out);
        }
    }
    None
}

/// Pack a callsign into its base value, returning `(packed, portable)`. Returns
/// `(0, portable)` for calls that don't fit the pattern (mirrors the reference's
/// `packed = 0` fallback). ref: varicode.cpp:946-1021.
pub fn pack_callsign(value: &str) -> (u32, bool) {
    let mut call = value.trim().to_ascii_uppercase();
    let mut portable = false;

    if let Some(v) = basecall_value(&call) {
        return (v, false);
    }

    if let Some(stripped) = call.strip_suffix("/P") {
        call = stripped.to_string();
        portable = true;
    }

    // Country workarounds. ref: varicode.cpp:958-968.
    if let Some(rest) = call.strip_prefix("3DA0") {
        call = format!("3D0{rest}");
    }
    if call.starts_with("3X") && call.as_bytes().get(2).is_some_and(|c| c.is_ascii_uppercase()) {
        call = format!("Q{}", &call[2..]);
    }

    let slen = call.len();
    if !(2..=6).contains(&slen) {
        return (0, portable);
    }

    // Space-padded permutations, matched in order; the last match wins (the
    // reference overwrites `matched` without breaking). ref: varicode.cpp:975-1007.
    let mut permutations: Vec<String> = vec![call.clone()];
    match slen {
        2 => permutations.push(format!(" {call}   ")),
        3 => {
            permutations.push(format!(" {call}  "));
            permutations.push(format!("{call}   "));
        }
        4 => {
            permutations.push(format!(" {call} "));
            permutations.push(format!("{call}  "));
        }
        5 => {
            permutations.push(format!(" {call}"));
            permutations.push(format!("{call} "));
        }
        _ => {}
    }

    let mut matched: Option<[u8; 6]> = None;
    for p in &permutations {
        if let Some(m) = first_match(p.as_bytes()) {
            matched = Some(m);
        }
    }
    let m = match matched {
        Some(m) => m,
        None => return (0, portable),
    };

    let mut packed = idx(m[0]) as i64;
    packed = 36 * packed + idx(m[1]) as i64;
    packed = 10 * packed + idx(m[2]) as i64;
    packed = 27 * packed + idx(m[3]) as i64 - 10;
    packed = 27 * packed + idx(m[4]) as i64 - 10;
    packed = 27 * packed + idx(m[5]) as i64 - 10;
    (packed as u32, portable)
}

/// Unpack a base value (with the `portable` flag) back to a callsign string.
/// ref: varicode.cpp:1023-1069.
pub fn unpack_callsign(value: u32, portable: bool) -> String {
    // Group / special base calls.
    if let Some((name, _)) = BASECALLS.iter().find(|(_, v)| NBASECALL + *v == value) {
        return name.to_string();
    }

    let mut v = value;
    let mut word = [0u8; 6];
    let take = |v: &mut u32, modu: u32, add: u32| -> u8 {
        let t = *v % modu + add;
        *v /= modu;
        ALPHANUMERIC[t as usize]
    };
    word[5] = take(&mut v, 27, 10);
    word[4] = take(&mut v, 27, 10);
    word[3] = take(&mut v, 27, 10);
    word[2] = take(&mut v, 10, 0);
    word[1] = take(&mut v, 36, 0);
    word[0] = ALPHANUMERIC[v as usize];

    let mut call = String::from_utf8_lossy(&word).into_owned();
    // Reverse the country workarounds. ref: varicode.cpp:1055-1062.
    if let Some(rest) = call.strip_prefix("3D0") {
        call = format!("3DA0{rest}");
    }
    if call.starts_with('Q') && call.as_bytes().get(1).is_some_and(|c| c.is_ascii_uppercase()) {
        call = format!("3X{}", &call[1..]);
    }
    let call = call.trim().to_string();
    if portable {
        format!("{call}/P")
    } else {
        call
    }
}

// ---------------------------------------------------------------------------
// 50-bit compound-callsign packing (heartbeat / compound frames)
// ---------------------------------------------------------------------------
//
// Packs an 11-char field into 50 bits: 9 base-38 alphanumeric positions plus a
// base-39 leading position and two boolean `/` positions (3 and 7), so compound
// calls like `VE3/K1ABC` fit. ref: varicode.cpp:863-944.

fn idx38(c: u8) -> u64 {
    ALPHANUMERIC.iter().position(|&a| a == c).unwrap_or(0) as u64
}

/// Pack a compound callsign (≤11 useful chars) into a 50-bit value.
/// ref: varicode.cpp:863-889 (`packAlphaNumeric50`).
pub fn pack_alphanumeric50(value: &str) -> u64 {
    // Keep only [A-Z0-9 /@] from the uppercased input.
    let mut word: Vec<u8> = value
        .to_ascii_uppercase()
        .bytes()
        .filter(|&c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b' ' || c == b'/' || c == b'@')
        .collect();
    // Align the `/` separators to positions 3 and 7.
    if word.len() > 3 && word[3] != b'/' {
        word.insert(3, b' ');
    }
    if word.len() > 7 && word[7] != b'/' {
        word.insert(7, b' ');
    }
    while word.len() < 11 {
        word.push(b' ');
    }

    // Positional weights (identical products to the reference).
    let w = |c: u8| idx38(c);
    let is_slash = |c: u8| (c == b'/') as u64;
    let a = 38 * 38 * 38 * 2 * 38 * 38 * 38 * 2 * 38 * 38 * w(word[0]);
    let b = 38 * 38 * 38 * 2 * 38 * 38 * 38 * 2 * 38 * w(word[1]);
    let c = 38 * 38 * 38 * 2 * 38 * 38 * 38 * 2 * w(word[2]);
    let d = 38 * 38 * 38 * 2 * 38 * 38 * 38 * is_slash(word[3]);
    let e = 38 * 38 * 38 * 2 * 38 * 38 * w(word[4]);
    let f = 38 * 38 * 38 * 2 * 38 * w(word[5]);
    let g = 38 * 38 * 38 * 2 * w(word[6]);
    let h = 38 * 38 * 38 * is_slash(word[7]);
    let i = 38 * 38 * w(word[8]);
    let j = 38 * w(word[9]);
    let k = w(word[10]);
    a + b + c + d + e + f + g + h + i + j + k
}

/// Unpack a 50-bit compound-callsign value, stripping spaces.
/// ref: varicode.cpp:893-944 (`unpackAlphaNumeric50`).
pub fn unpack_alphanumeric50(mut packed: u64) -> String {
    let mut word = [b' '; 11];
    let take38 = |p: &mut u64| -> u8 {
        let t = (*p % 38) as usize;
        *p /= 38;
        ALPHANUMERIC[t]
    };
    word[10] = take38(&mut packed);
    word[9] = take38(&mut packed);
    word[8] = take38(&mut packed);
    word[7] = if packed % 2 == 1 { b'/' } else { b' ' };
    packed /= 2;
    word[6] = take38(&mut packed);
    word[5] = take38(&mut packed);
    word[4] = take38(&mut packed);
    word[3] = if packed % 2 == 1 { b'/' } else { b' ' };
    packed /= 2;
    word[2] = take38(&mut packed);
    word[1] = take38(&mut packed);
    word[0] = ALPHANUMERIC[(packed % 39) as usize];

    String::from_utf8_lossy(&word).replace(' ', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Standard callsigns round-trip through pack → unpack.
    #[test]
    fn callsign_roundtrip() {
        for call in ["K1ABC", "W1AW", "VK3ABC", "G0ABC", "N5AC", "AA1A"] {
            let (packed, portable) = pack_callsign(call);
            assert!(packed < NBASECALL, "{call} should pack below nbasecall, got {packed}");
            assert_eq!(unpack_callsign(packed, portable), call, "roundtrip failed for {call}");
        }
    }

    /// Portable (`/P`) suffix round-trips via the flag.
    #[test]
    fn portable_roundtrip() {
        let (packed, portable) = pack_callsign("K1ABC/P");
        assert!(portable);
        assert_eq!(unpack_callsign(packed, portable), "K1ABC/P");
    }

    /// Group / special base calls map to the reserved values and back.
    #[test]
    fn basecalls_roundtrip() {
        for name in ["@ALLCALL", "@CQ", "@JS8NET", "@HB", "<....>"] {
            let (packed, _) = pack_callsign(name);
            assert!(packed > NBASECALL, "{name} should map above nbasecall");
            assert_eq!(unpack_callsign(packed, false), name);
        }
    }

    /// Packing is case-insensitive and trims whitespace.
    #[test]
    fn normalizes_input() {
        let (a, _) = pack_callsign("k1abc");
        let (b, _) = pack_callsign("  K1ABC  ");
        let (c, _) = pack_callsign("K1ABC");
        assert_eq!(a, c);
        assert_eq!(b, c);
    }

    /// The `@CQ` reserved value matches `nbasecall + 44`.
    #[test]
    fn cq_group_value() {
        let (packed, _) = pack_callsign("@CQ");
        assert_eq!(packed, NBASECALL + 44);
    }

    /// Compound callsigns round-trip through the 50-bit packer (spaces are
    /// stripped on unpack, so the `/` separators must land on positions 3/7).
    #[test]
    fn alphanumeric50_roundtrip() {
        for call in ["K1ABC", "W1AW", "VE3/K1ABC", "K1ABC/P", "@ALLCALL", "N5AC"] {
            let packed = pack_alphanumeric50(call);
            assert!(packed < (1u64 << 50), "{call} must fit in 50 bits");
            assert_eq!(unpack_alphanumeric50(packed), call, "50-bit roundtrip failed for {call}");
        }
    }

    /// The 50-bit value fits its field for a maximal 11-char compound call.
    #[test]
    fn alphanumeric50_bounds() {
        let packed = pack_alphanumeric50("VE3/K1ABC/P");
        assert!(packed < (1u64 << 50));
    }

    /// Bit-exact vs the real `varicode.cpp` `packCallsign` (Qt build). Golden
    /// values from `scratch/refvectors/js8/framesqt/frames_dump.cpp` (js8call @
    /// a7ff1be). Confirms the base-40 encoding + group calls against the reference.
    #[test]
    fn pack_callsign_matches_reference() {
        let cases: &[(&str, u32)] = &[
            ("K1ABC", 259047992),
            ("W1AW", 261410543),
            ("VK3ABC", 223657958),
            ("G0ABC", 258240989),
            ("N5AC", 259717265),
            ("AA1A", 72847511),
            ("@CQ", 262177604),
            ("@ALLCALL", 262177562),
        ];
        for (call, want) in cases {
            assert_eq!(pack_callsign(call).0, *want, "packCallsign({call}) mismatch");
        }
    }

    /// Bit-exact vs the real `varicode.cpp` `packAlphaNumeric50` (Qt build).
    #[test]
    fn pack_alphanumeric50_matches_reference() {
        let cases: &[(&str, u64)] = &[
            ("K1ABC", 348403268086540),
            ("W1AW", 557100718697932),
            ("VE3/K1ABC", 545578825598840),
            ("K1ABC/P", 348403268180400),
            ("@ALLCALL", 665697326060246),
            ("N5AC", 402407681626828),
        ];
        for (call, want) in cases {
            assert_eq!(pack_alphanumeric50(call), *want, "packAlphaNumeric50({call}) mismatch");
        }
    }
}
