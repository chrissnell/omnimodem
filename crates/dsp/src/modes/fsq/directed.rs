//! FSQCALL directed-message protocol layer — the CRC8-keyed header, callsign
//! addressing, trigger classification, heard list, and reply synthesis.
//!
//! Port of fldigi `fsq.cxx` `parse_rx_text` (436-623), `valid_callsign`
//! (419-434), and the `parse_*` responders (625-990). A received transmission is
//! `<from>:<crc><addressees><trigger><payload>`; `parse_rx_text` verifies the
//! CRC8 over the sender callsign, registers the sender in the heard list,
//! resolves whether we are addressed (our call / `allcall` / `cqcqcq`), and
//! returns the leading trigger + payload. Reply synthesis for the core triggers
//! (`?` snr, `*` ack, `$` heard) is a pure function; the async station services
//! (sounder, aging, relay/store-and-forward) are out of scope (see phase plan).
//!
//! `valid_callsign`'s POSIX regex is approximated by a structural check
//! (alnum/`/` charset with at least one letter and one digit) — sufficient to
//! classify sender/addressee words for the directed protocol.

use crate::framing::fsq_varicode::crc8_hex;

/// Trigger characters. Leading `' '` makes space itself a trigger (a plain-text
/// directed message). ref: fsq.cxx:405.
const TRIGGERS: &str = " !#$%&'()*+,-.;<=>?@[\\]^_{|}~";

fn is_trigger(c: u8) -> bool {
    TRIGGERS.as_bytes().contains(&c)
}

/// Callsign classification, mirroring fldigi's `valid_callsign`: `0` = not a
/// call, `1` = our call, `2` = `allcall`, `4` = `cqcqcq`, `8` = some other valid
/// call. ref: fsq.cxx:419-434.
pub fn valid_callsign(s: &str, mycall: &str) -> u8 {
    if s.len() < 3 || s.len() > 20 {
        return 0;
    }
    if s == "allcall" {
        return 2;
    }
    if s == "cqcqcq" {
        return 4;
    }
    if !mycall.is_empty() && s == mycall {
        return 1;
    }
    if s.contains("Heard") {
        return 0;
    }
    if looks_like_callsign(s) {
        8
    } else {
        0
    }
}

/// Faithful reimplementation of fldigi's callsign regex (fsq.cxx:409), applied
/// unanchored like `regexec`: `([[:alnum:]]?[[:alpha:]/]+[[:digit:]]+[[:alnum:]/]+)`
/// — some substring must be an optional leading alnum, then one-or-more
/// letters/`/`, then one-or-more digits, then one-or-more alnum/`/`. The three
/// character classes partition cleanly at each boundary (letters/`/` never
/// contain a digit; digits never contain a letter/`/`), so a greedy scan needs no
/// backtracking. This matters for interop: the earlier "any letter + any digit"
/// approximation accepted digit-leading tokens fldigi rejects (`999z`, `1a1`,
/// `1234a`), which would classify an addressee word differently and diverge the
/// directed-message parse.
fn looks_like_callsign(s: &str) -> bool {
    let b = s.as_bytes();
    let alpha_or_slash = |c: u8| c.is_ascii_alphabetic() || c == b'/';
    let alnum_or_slash = |c: u8| c.is_ascii_alphanumeric() || c == b'/';
    for start in 0..b.len() {
        // The leading `[[:alnum:]]?` is optional: try consuming it and not.
        for lead in [0usize, 1] {
            let mut i = start;
            if lead == 1 {
                if i < b.len() && b[i].is_ascii_alphanumeric() {
                    i += 1;
                } else {
                    continue;
                }
            }
            let mark = i;
            while i < b.len() && alpha_or_slash(b[i]) {
                i += 1;
            }
            if i == mark {
                continue; // need ≥1 [alpha/]
            }
            let mark = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == mark {
                continue; // need ≥1 [digit]
            }
            let mark = i;
            while i < b.len() && alnum_or_slash(b[i]) {
                i += 1;
            }
            if i > mark {
                return true; // need ≥1 [alnum/]
            }
        }
    }
    false
}

/// A station heard on the air, with its most recent SNR estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeardEntry {
    pub call: String,
    pub snr: Option<String>,
}

/// The heard list: most-recently-heard stations, de-duplicated by callsign.
/// Aging/eviction timers (fldigi's `aging` thread) are out of scope.
#[derive(Debug, Clone, Default)]
pub struct HeardList {
    entries: Vec<HeardEntry>,
}

impl HeardList {
    pub fn new() -> Self {
        HeardList::default()
    }

    /// Register (or refresh) a station. Most recent moves to the front. ref:
    /// fsq.cxx `add_to_heard_list`.
    pub fn add(&mut self, call: &str, snr: Option<String>) {
        self.entries.retain(|e| e.call != call);
        self.entries.insert(0, HeardEntry { call: call.to_string(), snr });
    }

    pub fn contains(&self, call: &str) -> bool {
        self.entries.iter().any(|e| e.call == call)
    }

    pub fn entries(&self) -> &[HeardEntry] {
        &self.entries
    }

    /// A newline-separated `call snr` list, as the `$` responder embeds. ref:
    /// fsq.cxx `heard_list`.
    pub fn list(&self) -> String {
        self.entries
            .iter()
            .map(|e| match &e.snr {
                Some(s) => format!("{} {}\n", e.call, s),
                None => format!("{}\n", e.call),
            })
            .collect()
    }
}

/// A parsed directed message. `to_me`/`to_all` follow fldigi's `directed`/`all`
/// flags; `trigger` is the leading trigger character (`' '` = plain text);
/// `payload` is the message body after addressing (verbatim, including fldigi's
/// leading space for a text line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectedMessage {
    pub from: String,
    pub crc_ok: bool,
    pub to_me: bool,
    pub to_all: bool,
    pub trigger: char,
    pub payload: String,
}

/// Parse an accumulated transmission (`rx_text`, from BOT to EOT) into a directed
/// message, registering the CRC-verified sender in `heard`. Returns `None` when
/// there is no valid `<call>:<crc>` header, when the sender is our own call, or
/// when the message addresses neither us nor an all-call. Faithful port of
/// `parse_rx_text` (fsq.cxx:436-623).
pub fn parse_rx_text(
    rx_text: &str,
    mycall: &str,
    heard: &mut HeardList,
) -> Option<DirectedMessage> {
    let mut rx: Vec<u8> = rx_text.bytes().collect();
    if rx.is_empty() {
        return None;
    }
    let p = rx.iter().position(|&c| c == b':')?;
    if p == 0 || p + 2 >= rx.len() {
        return None;
    }
    let rxcrc = [rx[p + 1], rx[p + 2]];

    // Scan backward from the colon for the sender callsign whose CRC matches.
    // ref: fsq.cxx:470-483.
    let maxi = (p + 1).min(20);
    let mut station: Option<String> = None;
    for i in 1..maxi {
        let c = rx[p - i];
        if c <= b' ' || c > b'z' {
            return None; // header charset violated
        }
        let substr = String::from_utf8_lossy(&rx[p - i..p]).into_owned();
        if crc8_hex(&substr).as_bytes() == rxcrc && valid_callsign(&substr, mycall) != 0 {
            station = Some(substr);
            break;
        }
    }
    let from = station?;
    if from == mycall {
        return None; // do not act on our own echo. ref: fsq.cxx:485-489.
    }
    heard.add(&from, None);

    // Strip "<call>:<crc>". ref: fsq.cxx:502.
    rx.drain(0..p + 3);

    // Walk addressees + resolve trigger. ref: fsq.cxx:507-563.
    let mut all = false;
    let mut directed = false;

    while rx.len() > 1 && is_trigger(rx[0]) {
        rx.remove(0);
    }
    let mut tr_pos = first_trigger(&rx);
    while tr_pos < rx.len() {
        let word = String::from_utf8_lossy(&rx[0..tr_pos]).into_owned();
        match valid_callsign(&word, mycall) {
            0 => {
                rx.insert(0, b' ');
                break;
            }
            1 => directed = true,
            8 => {}
            _ => all = true, // allcall / cqcqcq
        }
        rx.drain(0..tr_pos);
        while rx.len() > 1 && rx[0] == b' ' && rx[1] == b' ' {
            rx.remove(0);
        }
        if rx.first() != Some(&b' ') {
            break;
        }
        rx.remove(0);
        tr_pos = first_trigger(&rx);
    }

    if !all && !directed {
        return None;
    }

    // Remove the EOT tail if still present. ref: fsq.cxx:566.
    if rx.len() > 3 {
        rx.truncate(rx.len() - 3);
    }

    // fldigi's `if (trigger == NIT) { tr = ' '; insert space }` guard
    // (fsq.cxx:571-577) forces a non-trigger leading char to a space (text line).
    // The addressee walk above only exits with `rx[0]` being a trigger char or a
    // space (it inserts a leading space when a word isn't a callsign), so the
    // guard is unreachable here and `rx.first()` is already the effective trigger.
    let trigger = rx.first().map(|&c| c as char).unwrap_or(' ');
    let payload = String::from_utf8_lossy(&rx).into_owned();

    Some(DirectedMessage { from, crc_ok: true, to_me: directed, to_all: all, trigger, payload })
}

fn first_trigger(rx: &[u8]) -> usize {
    let mut i = 0;
    while i < rx.len() && !is_trigger(rx[i]) {
        i += 1;
    }
    i
}

/// Synthesise the reply **body** a station would transmit for a directed message,
/// for the core auto-responder triggers. The caller frames it via
/// `modes::fsq::build_tx(mycall, body, true)`. Returns `None` for triggers with
/// no automatic reply (plain text) or the deferred async triggers. ref:
/// fsq.cxx:630-653 (`parse_qmark`/`parse_star`/`parse_dollar`).
pub fn reply_for(msg: &DirectedMessage, snr: &str, heard: &HeardList) -> Option<String> {
    match msg.trigger {
        '?' => Some(format!("{} snr={}", msg.from, snr)), // ref: fsq.cxx:632-634
        '*' => Some(format!("{} ack", msg.from)),         // ref: fsq.cxx:652
        '$' => Some(format!("{} Heard:\n{}", msg.from, heard.list())), // ref: fsq.cxx:641-644
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callsign_classification() {
        assert_eq!(valid_callsign("k1abc", "k1abc"), 1);
        assert_eq!(valid_callsign("w1hkj", "k1abc"), 8);
        assert_eq!(valid_callsign("allcall", "k1abc"), 2);
        assert_eq!(valid_callsign("cqcqcq", "k1abc"), 4);
        assert_eq!(valid_callsign("test", "k1abc"), 0); // no digit
        assert_eq!(valid_callsign("hi", "k1abc"), 0); // too short
        assert_eq!(valid_callsign("ve3xyz/p", "k1abc"), 8);
        // mycall matches by exact string equality (fsq.cxx:426), so a portable
        // suffix is a different (other) call, not our own.
        assert_eq!(valid_callsign("k1abc/7", "k1abc"), 8);
    }

    #[test]
    fn callsign_regex_matches_fldigi_posix_regex() {
        // Pinned against the compiled reference regex
        // ([[:alnum:]]?[[:alpha:]/]+[[:digit:]]+[[:alnum:]/]+), applied unanchored.
        // The structural order (letters → digits → alnum) matters: digit-leading
        // tokens fldigi rejects must NOT classify as a call.
        for s in ["k1abc", "w1hkj", "ve3xyz/p", "n0call", "3a4b", "ab12cd"] {
            assert_eq!(valid_callsign(s, ""), 8, "{s:?} should be a callsign");
        }
        for s in ["999z", "1a1", "1234a", "test", "hello", "aa", "1234"] {
            assert_eq!(valid_callsign(s, ""), 0, "{s:?} should NOT be a callsign");
        }
    }

    #[test]
    fn international_callsigns_match_fldigi() {
        // Parity with the compiled reference regex over a broad set of real-world
        // international callsigns (verified against fldigi's POSIX regex via
        // regexec): standard letter-prefix, numeric-prefix (2E0/4X1/3DA0/…),
        // special (1A0/4U1), and portable/prefix slash forms all classify as a
        // valid *other* call (8); the port must not diverge from fldigi on any of
        // them, or a directed frame addressed by a foreign station would parse
        // differently than the sender intended.
        let calls = [
            "DL1ABC", "G0XYZ", "M0ABC", "F5ABC", "EA1ABC", "OH2BH", "HB9ABC", "OE1ABC", "SP1ABC",
            "JA1ABC", "VK2ABC", "ZL1ABC", "PY2ABC", "LU1ABC", "VE3ABC", "VU2ABC", "ZS1ABC",
            "BV1ABC", "2E0ABC", "4X1ABC", "3DA0RS", "9A1ABC", "5B4ABC", "7Q7ABC", "8P6ABC",
            "3B8ABC", "9M2ABC", "4U1UN", "1A0KM", "RI1ANF", "KH6ABC", "VP8ABC", "K1ABC/P",
            "K1ABC/MM", "DL/K1ABC", "K1ABC/QRP", "ZS6/G0ABC", "VP2E/K1ABC", "EA9IB", "3Z0X",
            "OEM2O", "a1a",
        ];
        for s in calls {
            assert_eq!(valid_callsign(s, ""), 8, "{s:?} is a valid international call in fldigi");
        }
    }

    #[test]
    fn heard_list_dedups_and_orders() {
        let mut h = HeardList::new();
        h.add("w1hkj", Some("-5 db".into()));
        h.add("k1abc", None);
        h.add("w1hkj", Some("-3 db".into())); // refresh → front
        assert_eq!(h.entries().len(), 2);
        assert_eq!(h.entries()[0].call, "w1hkj");
        assert!(h.contains("k1abc"));
    }

    #[test]
    fn parse_directed_message_to_me() {
        // The rx_text the demod accumulates for build_tx("w1hkj","k1abc test",true)
        // — a leading LF (BOT), the header, body, and the "  \b" EOT prefix.
        let mut heard = HeardList::new();
        let rx = "\nw1hkj:efk1abc test  \x08";
        let m = parse_rx_text(rx, "k1abc", &mut heard).expect("directed msg");
        assert_eq!(m.from, "w1hkj");
        assert!(m.crc_ok);
        assert!(m.to_me);
        assert!(!m.to_all);
        assert_eq!(m.trigger, ' ');
        assert_eq!(m.payload, " test");
        assert!(heard.contains("w1hkj"));
    }

    #[test]
    fn parse_query_trigger_and_reply() {
        let mut heard = HeardList::new();
        // w1hkj sends "k1abc?" — a directed SNR query.
        let rx = "\nw1hkj:efk1abc?  \x08";
        let m = parse_rx_text(rx, "k1abc", &mut heard).expect("query");
        assert_eq!(m.trigger, '?');
        assert!(m.to_me);
        let reply = reply_for(&m, "-7 db", &heard).expect("reply");
        assert_eq!(reply, "w1hkj snr=-7 db");
    }

    #[test]
    fn parse_allcall() {
        let mut heard = HeardList::new();
        // w1hkj sends "allcall " (a plain all-call). CRC of w1hkj = ef.
        let rx = "\nw1hkj:efallcall hello all  \x08";
        let m = parse_rx_text(rx, "k1abc", &mut heard).expect("allcall");
        assert!(m.to_all);
        assert!(!m.to_me);
        assert_eq!(m.from, "w1hkj");
    }

    #[test]
    fn reject_bad_crc() {
        // Wrong CRC (00) for w1hkj → no valid sender → None.
        let mut heard = HeardList::new();
        assert!(parse_rx_text("\nw1hkj:00k1abc test  \x08", "k1abc", &mut heard).is_none());
        assert!(!heard.contains("w1hkj"));
    }

    #[test]
    fn reject_own_echo() {
        let mut heard = HeardList::new();
        // Our own call as sender is ignored. crc(k1abc)=f3.
        assert!(parse_rx_text("\nk1abc:f3w1hkj test  \x08", "k1abc", &mut heard).is_none());
    }

    #[test]
    fn reject_unaddressed() {
        // Sender valid but message addresses neither us nor an all-call.
        let mut heard = HeardList::new();
        assert!(parse_rx_text("\nw1hkj:efn0xyz test  \x08", "k1abc", &mut heard).is_none());
        // ...but the sender is still registered as heard.
        assert!(heard.contains("w1hkj"));
    }
}
