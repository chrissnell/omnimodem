//! CCIR-476 (SITOR) code + FEC-B time-diversity, ported from fldigi's NAVTEX
//! modem (`fldigi/src/navtex/navtex.cxx`, upstream w1hkj/fldigi 4.1.23 @
//! `61b97f413`).
//!
//! CCIR-476 is a 7-bit constant-weight code: every valid codeword has exactly
//! four bits set (`check_bits`), leaving three bits for error *detection*. The
//! `CODE_TO_LTRS` / `CODE_TO_FIGS` tables (transcribed verbatim from
//! navtex.cxx:465-487) map a codeword to a letter- or figure-shift character;
//! `'_'` marks an unused code. On the wire each codeword is sent **LSB first**
//! (`bytes_to_code`, navtex.cxx:554-561; `send_string`, :1798-1802).
//!
//! SITOR mode B (FEC-B / "collective") gives forward error correction by
//! **time diversity**: every character is transmitted twice, the repeat trailing
//! the direct copy by five codewords (35 bits). [`create_fec`] builds that
//! interleaved stream (navtex.cxx:1711-1732); [`FecBDecoder`] reverses it,
//! reproducing fldigi's direct/repeat/soft-combine/bit-flip fallback ladder
//! (navtex.cxx:1204-1289) and its character sync (`find_alpha_characters`,
//! :1095-1153).
//!
//! Bit-exact (Doctrine §3): the code tables, `char_to_code` output, and the
//! `create_fec` stream are asserted byte-for-byte against the reference
//! (`crates/dsp/tests/vectors/navtex_ccir476.json`, KAT in `tests/kat.rs`, and
//! the plain unit tests below since CI runs without `testutil`).

/// Letter-shift table: `CODE_TO_LTRS[code]` is the ASCII letter for a valid
/// codeword, `b'_'` if unused. ref: navtex.cxx:465-475 (verbatim).
// Kept as a per-codeword 16-column byte grid (not a packed byte string) so it
// stays auditable byte-for-byte against navtex.cxx; the KAT asserts it exactly.
#[allow(clippy::byte_char_slices)]
pub const CODE_TO_LTRS: [u8; 128] = [
    b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_',
    b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'J', b'_', b'_', b'_', b'F', b'_', b'C', b'K', b'_',
    b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'W', b'_', b'_', b'_', b'Y', b'_', b'P', b'Q', b'_',
    b'_', b'_', b'_', b'_', b'_', b'G', b'_', b'_', b'_', b'M', b'X', b'_', b'V', b'_', b'_', b'_',
    b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'A', b'_', b'_', b'_', b'S', b'_', b'I', b'U', b'_',
    b'_', b'_', b'_', b'D', b'_', b'R', b'E', b'_', b'_', b'N', b'_', b'_', b' ', b'_', b'_', b'_',
    b'_', b'_', b'_', b'Z', b'_', b'L', b'_', b'_', b'_', b'H', b'_', b'_', b'\n', b'_', b'_', b'_',
    b'_', b'O', b'B', b'_', b'T', b'_', b'_', b'_', b'\r', b'_', b'_', b'_', b'_', b'_', b'_', b'_',
];

/// Figure-shift table: `CODE_TO_FIGS[code]` is the ASCII figure/punctuation for a
/// valid codeword, `b'_'` if unused. `0x07` is the bell. ref: navtex.cxx:477-487
/// (verbatim).
pub const CODE_TO_FIGS: [u8; 128] = [
    b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'_',
    b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'\'', b'_', b'_', b'_', b'!', b'_', b':', b'(', b'_',
    b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'2', b'_', b'_', b'_', b'6', b'_', b'0', b'1', b'_',
    b'_', b'_', b'_', b'_', b'_', b'&', b'_', b'_', b'_', b'.', b'/', b'_', b';', b'_', b'_', b'_',
    b'_', b'_', b'_', b'_', b'_', b'_', b'_', b'-', b'_', b'_', b'_', 0x07, b'_', b'8', b'7', b'_',
    b'_', b'_', b'_', b'$', b'_', b'4', b'3', b'_', b'_', b',', b'_', b'_', b' ', b'_', b'_', b'_',
    b'_', b'_', b'_', b'"', b'_', b')', b'_', b'_', b'_', b'#', b'_', b'_', b'\n', b'_', b'_', b'_',
    b'_', b'9', b'?', b'_', b'5', b'_', b'_', b'_', b'\r', b'_', b'_', b'_', b'_', b'_', b'_', b'_',
];

// Control codewords. ref: navtex.cxx:489-495.
pub const CODE_LTRS: u8 = 0x5a;
pub const CODE_FIGS: u8 = 0x36;
pub const CODE_ALPHA: u8 = 0x0f;
pub const CODE_BETA: u8 = 0x33;
pub const CODE_CHAR32: u8 = 0x6a;
pub const CODE_REP: u8 = 0x66;
/// Bell (figure-shift 0x1b decodes to this). ref: navtex.cxx:495.
pub const CHAR_BELL: u8 = 0x07;

const UNUSED: u8 = b'_';

/// A valid CCIR-476 codeword has exactly four of its seven bits set; the other
/// three give error detection. ref: navtex.cxx:570-578 (`check_bits`).
#[inline]
pub fn check_bits(code: u8) -> bool {
    (code & 0x7f).count_ones() == 4
}

/// Decode a codeword to its shifted character, or `None` if the table slot is
/// unused for the current shift. ref: navtex.cxx:545-552 (`code_to_char`).
#[inline]
pub fn code_to_char(code: u8, figs_shift: bool) -> Option<u8> {
    let table = if figs_shift { &CODE_TO_FIGS } else { &CODE_TO_LTRS };
    let c = table[(code & 0x7f) as usize];
    if c != UNUSED {
        Some(c)
    } else {
        None
    }
}

/// The CCIR-476 encoder: holds the reverse (character → codeword) maps built the
/// same way fldigi's `CCIR476` constructor does — scan the 128 codes, and for
/// each valid one record its letter and figure characters. ref:
/// navtex.cxx:497-543.
pub struct Ccir476 {
    ltrs_to_code: [u8; 128],
    figs_to_code: [u8; 128],
}

impl Default for Ccir476 {
    fn default() -> Self {
        Self::new()
    }
}

impl Ccir476 {
    pub fn new() -> Self {
        let mut ltrs_to_code = [0u8; 128];
        let mut figs_to_code = [0u8; 128];
        for code in 0u8..128 {
            if check_bits(code) {
                let figv = CODE_TO_FIGS[code as usize];
                let ltrv = CODE_TO_LTRS[code as usize];
                if figv != UNUSED {
                    figs_to_code[figv as usize] = code;
                }
                if ltrv != UNUSED {
                    ltrs_to_code[ltrv as usize] = code;
                }
            }
        }
        Ccir476 { ltrs_to_code, figs_to_code }
    }

    /// Append the codeword(s) for one character, inserting a LTRS/FIGS shift code
    /// only when the running shift must change. `shift` (`true` = figures) is the
    /// carried modem state. ref: navtex.cxx:524-543 (`char_to_code`).
    pub fn char_to_code(&self, out: &mut Vec<u8>, ch: char, shift: &mut bool) {
        let ch = (ch as u32).min(255) as u8;
        let ch = ch.to_ascii_uppercase();
        if ch >= 128 {
            return;
        }
        let figc = self.figs_to_code[ch as usize];
        let ltrc = self.ltrs_to_code[ch as usize];
        if *shift && figc != 0 {
            out.push(figc);
        } else if !*shift && ltrc != 0 {
            out.push(ltrc);
        } else if figc != 0 {
            *shift = true;
            out.push(CODE_FIGS);
            out.push(figc);
        } else if ltrc != 0 {
            *shift = false;
            out.push(CODE_LTRS);
            out.push(ltrc);
        }
    }

    /// Encode a whole string to the codeword stream (no FEC yet). ref:
    /// navtex.cxx:1735-1743 (`encode`).
    pub fn encode(&self, text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut shift = false;
        for ch in text.chars() {
            self.char_to_code(&mut out, ch, &mut shift);
        }
        out
    }
}

/// FEC-B time diversity: interleave each codeword with the copy from two
/// characters earlier so the repeat trails the direct copy by five codewords
/// (35 bits). ref: navtex.cxx:1711-1732 (`create_fec`). Requires `codes.len()
/// >= OFFSET`.
pub fn create_fec(codes: &[u8]) -> Vec<u8> {
    const OFFSET: usize = 2;
    let sz = codes.len();
    let mut res = Vec::with_capacity(2 * sz + 8);
    for _ in 0..OFFSET {
        res.push(CODE_REP);
        res.push(CODE_ALPHA);
    }
    for i in 0..sz {
        res.push(codes[i]);
        res.push(if i >= OFFSET { codes[i - OFFSET] } else { CODE_ALPHA });
    }
    for i in 0..OFFSET {
        res.push(CODE_CHAR32);
        res.push(codes[sz - OFFSET + i]);
    }
    res
}

/// Serialize a codeword stream to on-air bits, LSB first per 7-bit code. ref:
/// navtex.cxx:1798-1802 (`send_string`).
pub fn codes_to_bits(codes: &[u8]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(codes.len() * 7);
    for &c in codes {
        let mut c = c;
        for _ in 0..7 {
            bits.push(c & 1 == 1);
            c >>= 1;
        }
    }
    bits
}

// --- receiver: FEC-B soft-diversity decoder (navtex.cxx:1046-1328) ---

/// Soft-bit buffer length — one second of 100-baud bits, fldigi's
/// `m_bit_values.resize(m_baud_rate)`. ref: navtex.cxx:939.
const BUFFERSIZE: usize = 100;
/// Repeat lead: the rep copy is five codewords (35 bits) before the direct copy.
/// ref: navtex.cxx:1046-1050 (`fec_offset`).
const FEC_LEAD: i32 = 35;

#[derive(Clone, Copy, PartialEq)]
enum State {
    SyncSetup,
    Sync,
    ReadData,
}

/// Streaming FEC-B decoder: fed one soft bit per 100-baud symbol (sign = tone
/// decision, `+` = mark/1; magnitude = confidence), it reproduces fldigi's
/// character sync + direct/repeat/soft-combine/bit-flip ladder and yields the
/// decoded characters (control codes handled internally). A faithful port of the
/// navtex.cxx modem's bit path, kept per-instance (no globals).
pub struct FecBDecoder {
    bits: [f32; BUFFERSIZE],
    cursor: i32,
    state: State,
    alpha_phase: bool,
    error_count: i32,
    figs_shift: bool,
    last_char: i32,
    out: Vec<u8>,
}

impl Default for FecBDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FecBDecoder {
    pub fn new() -> Self {
        FecBDecoder {
            bits: [0.0; BUFFERSIZE],
            cursor: 0,
            state: State::SyncSetup,
            alpha_phase: true,
            error_count: 0,
            figs_shift: false,
            last_char: 0,
            out: Vec::new(),
        }
    }

    /// Feed one soft bit; returns any characters decoded as a result. `soft > 0`
    /// is a mark (logic 1); its magnitude is the confidence used by the FEC
    /// soft-combine.
    pub fn feed_bit(&mut self, soft: f32) -> &[u8] {
        let start = self.out.len();
        self.handle_bit_value(soft);
        &self.out[start..]
    }

    /// Take all decoded characters accumulated so far.
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    fn code_at(&self, pos: i32) -> u8 {
        // ref: navtex.cxx:554-561 (bytes_to_code): bit i = (pos[i] > 0).
        let mut code = 0u8;
        for i in 0..7 {
            if self.bits[(pos + i) as usize] > 0.0 {
                code |= 1 << i;
            }
        }
        code
    }

    fn valid_char_at(&self, pos: i32) -> bool {
        // ref: navtex.cxx:581-590.
        (0..7).filter(|&i| self.bits[(pos + i) as usize] > 0.0).count() == 4
    }

    // ref: navtex.cxx:1095-1153 (find_alpha_characters).
    fn find_alpha_characters(&self) -> i32 {
        let mut best_offset = 0i32;
        let mut best_score = 0i32;
        let limit = BUFFERSIZE as i32 - 7;
        for offset in FEC_LEAD..(FEC_LEAD + 14) {
            let mut score = 0i32;
            let mut reps = 0i32;
            let mut i = offset;
            while i < limit {
                if self.valid_char_at(i) {
                    let ri = i - FEC_LEAD;
                    let code = self.code_at(i);
                    let rep = self.code_at(ri);
                    score += 1;
                    if code == rep {
                        if code == CODE_ALPHA || code == CODE_REP {
                            score = 0;
                            i += 7;
                            continue;
                        }
                        reps += 1;
                    } else if code == CODE_ALPHA {
                        let rep2 = self.code_at(i - 7);
                        if rep2 == CODE_REP {
                            reps += 1;
                        }
                    }
                }
                i += 7;
            }
            if reps >= 3 && score + reps > best_score {
                best_score = score + reps;
                best_offset = offset;
            }
        }
        if best_score > 8 {
            best_offset
        } else {
            -1
        }
    }

    // ref: navtex.cxx:1157-1196 (handle_bit_value) with the SYNC_SETUP→SYNC
    // reset folded in (navtex.cxx:1638-1642).
    fn handle_bit_value(&mut self, acc: f32) {
        for i in 0..BUFFERSIZE - 1 {
            self.bits[i] = self.bits[i + 1];
        }
        self.bits[BUFFERSIZE - 1] = acc;
        if self.cursor > 0 {
            self.cursor -= 1;
        }

        if self.state == State::SyncSetup {
            // A resync clears the error count AND the LTRS/FIGS shift, so stale
            // figures state can't corrupt the first chars after re-locking.
            // ref: navtex.cxx:1639-1642.
            self.error_count = 0;
            self.figs_shift = false;
            self.state = State::Sync;
        }

        if self.state == State::Sync {
            let offset = self.find_alpha_characters();
            if offset >= 0 {
                self.state = State::ReadData;
                self.cursor = offset;
                self.alpha_phase = true;
            } else {
                self.state = State::SyncSetup;
            }
        }

        if self.state == State::ReadData && self.cursor < BUFFERSIZE as i32 - 7 {
            if self.alpha_phase {
                let ret = self.process_bytes(self.cursor);
                self.error_count -= ret;
                if self.error_count > 5 {
                    self.state = State::SyncSetup;
                }
                if self.error_count < 0 {
                    self.error_count = 0;
                }
            }
            self.alpha_phase = !self.alpha_phase;
            self.cursor += 7;
        }
    }

    // ref: navtex.cxx:1054-1084 (flip_smallest_bit).
    fn flip_smallest_bit(pos: &mut [f32; 7]) {
        let (mut min_zero, mut min_one) = (f32::NEG_INFINITY, f32::INFINITY);
        let (mut min_zero_pos, mut min_one_pos) = (usize::MAX, usize::MAX);
        let (mut count_zero, mut count_one) = (0i32, 1i32);
        for (i, &val) in pos.iter().enumerate() {
            if val < 0.0 {
                count_zero += 1;
                if val > min_zero {
                    min_zero = val;
                    min_zero_pos = i;
                }
            } else {
                count_one += 1;
                if val < min_one {
                    min_one = val;
                    min_one_pos = i;
                }
            }
        }
        if count_zero == 4 && min_zero_pos != usize::MAX {
            pos[min_zero_pos] = -pos[min_zero_pos];
        } else if count_one == 5 && min_one_pos != usize::MAX {
            pos[min_one_pos] = -pos[min_one_pos];
        }
    }

    fn code_from_soft(soft: &[f32; 7]) -> u8 {
        let mut code = 0u8;
        for (i, &s) in soft.iter().enumerate() {
            if s > 0.0 {
                code |= 1 << i;
            }
        }
        code
    }

    // ref: navtex.cxx:1204-1289 (process_bytes). Returns 1 on a clean direct
    // decode, 0 on a rep skip, -1 on FEC recovery, -2 on hard failure.
    fn process_bytes(&mut self, cursor: i32) -> i32 {
        let code = self.code_at(cursor);
        if check_bits(code) {
            self.process_char(code as i32);
            return 1;
        }
        let reppos = cursor - FEC_LEAD;
        if reppos < 0 {
            return -1;
        }

        let rep = self.code_at(reppos);
        if check_bits(rep) {
            if rep == CODE_REP {
                return 0;
            }
            self.process_char(rep as i32);
            return 1;
        }

        // Soft-combine the direct and repeat confidences.
        let mut avg = [0.0f32; 7];
        for (i, a) in avg.iter_mut().enumerate() {
            *a = self.bits[(cursor + i as i32) as usize] + self.bits[(reppos + i as i32) as usize];
        }
        let calc = Self::code_from_soft(&avg);
        if check_bits(calc) {
            self.process_char(calc as i32);
            return -1;
        }

        // Flip the least-certain bit in the direct copy.
        let mut alpha = self.window(cursor);
        Self::flip_smallest_bit(&mut alpha);
        let calc = Self::code_from_soft(&alpha);
        if check_bits(calc) {
            self.write_window(cursor, &alpha);
            self.process_char(calc as i32);
            return -1;
        }
        self.write_window(cursor, &alpha);

        // Flip the least-certain bit in the repeat copy.
        let mut rep_w = self.window(reppos);
        Self::flip_smallest_bit(&mut rep_w);
        let calc = Self::code_from_soft(&rep_w);
        if check_bits(calc) {
            self.write_window(reppos, &rep_w);
            self.process_char(calc as i32);
            return -1;
        }
        self.write_window(reppos, &rep_w);

        // Flip the least-certain bit in the soft-combined copy.
        Self::flip_smallest_bit(&mut avg);
        let calc = Self::code_from_soft(&avg);
        if check_bits(calc) {
            self.process_char(calc as i32);
            return -1;
        }
        -2
    }

    fn window(&self, pos: i32) -> [f32; 7] {
        let mut w = [0.0f32; 7];
        for (i, slot) in w.iter_mut().enumerate() {
            *slot = self.bits[(pos + i as i32) as usize];
        }
        w
    }

    fn write_window(&mut self, pos: i32, w: &[f32; 7]) {
        for (i, &v) in w.iter().enumerate() {
            self.bits[(pos + i as i32) as usize] = v;
        }
    }

    // ref: navtex.cxx:1291-1328 (process_char).
    fn process_char(&mut self, chr: i32) {
        let code = chr as u8;
        match code {
            CODE_REP => {
                if self.last_char == CODE_REP as i32 {
                    self.alpha_phase = false;
                }
            }
            CODE_ALPHA | CODE_BETA | CODE_CHAR32 => {}
            CODE_LTRS => self.figs_shift = false,
            CODE_FIGS => self.figs_shift = true,
            _ => {
                if let Some(c) = code_to_char(code, self.figs_shift) {
                    self.filter_print(c);
                }
            }
        }
        self.last_char = chr;
    }

    // ref: navtex.cxx:1330-1337 (filter_print): bell shows as a quote, CR is
    // swallowed, control codes never print.
    fn filter_print(&mut self, c: u8) {
        if c == CHAR_BELL {
            self.out.push(b'\'');
        } else if c != b'\r' && c != CODE_ALPHA && c != CODE_REP {
            self.out.push(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_35_valid_codewords() {
        // C(7,4) = 35 constant-weight codewords.
        assert_eq!((0u8..128).filter(|&c| check_bits(c)).count(), 35);
    }

    #[test]
    fn control_codes_are_valid_4of7() {
        for c in [CODE_LTRS, CODE_FIGS, CODE_ALPHA, CODE_BETA, CODE_CHAR32, CODE_REP] {
            assert!(check_bits(c), "control code {c:#x} must be 4-of-7");
        }
    }

    #[test]
    fn encode_inserts_minimal_shifts() {
        let c = Ccir476::new();
        // "CQ DE K1ABC": letters, then FIGS for '1', then LTRS back for 'A'.
        let codes = c.encode("CQ DE K1ABC");
        assert_eq!(codes, vec![29, 46, 92, 83, 86, 92, 30, 54, 46, 90, 71, 114, 29]);
    }

    #[test]
    fn encode_uppercases() {
        let c = Ccir476::new();
        assert_eq!(c.encode("cq"), c.encode("CQ"));
    }

    #[test]
    fn create_fec_preamble_and_diversity() {
        let codes = vec![10u8, 20, 30, 40];
        let fec = create_fec(&codes);
        // 2×[REP,ALPHA] preamble.
        assert_eq!(&fec[..4], &[CODE_REP, CODE_ALPHA, CODE_REP, CODE_ALPHA]);
        // Each source code appears, its repeat trailing by five codewords.
        assert_eq!(fec[4], 10);
        assert_eq!(fec[9], 10); // rep of codes[0], 5 codewords later
    }

    #[test]
    fn codes_to_bits_is_lsb_first() {
        // code 0x0f = 0b0001111 -> bits 1,1,1,1,0,0,0 (LSB first).
        assert_eq!(codes_to_bits(&[0x0f]), vec![true, true, true, true, false, false, false]);
    }

    /// Drive the streaming decoder from a bit stream at strong confidence and
    /// confirm the direct/repeat FEC path reproduces the message.
    #[test]
    fn fecb_roundtrip_recovers_text() {
        let c = Ccir476::new();
        let msg = "CQ DE K1ABC";
        let fec = create_fec(&c.encode(msg));
        // Repeat the stream so the 100-bit sync window fills before the payload.
        let mut bits = Vec::new();
        for _ in 0..3 {
            bits.extend(codes_to_bits(&fec));
        }
        let mut dec = FecBDecoder::new();
        for b in bits {
            dec.feed_bit(if b { 1.0 } else { -1.0 });
        }
        let text = String::from_utf8_lossy(&dec.take()).to_string();
        assert!(text.contains(msg), "decoded {text:?}");
    }

    /// A single flipped bit in a direct codeword must be corrected from the
    /// repeat copy (the FEC-B point).
    #[test]
    fn fecb_repeats_repair_single_errors() {
        let c = Ccir476::new();
        let msg = "SECURITE";
        let fec = create_fec(&c.encode(msg));
        let mut bits = Vec::new();
        for _ in 0..3 {
            bits.extend(codes_to_bits(&fec));
        }
        // Corrupt every 37th bit (sparse, unlikely to hit both copies of a char).
        let mut soft: Vec<f32> = bits.iter().map(|&b| if b { 4.0 } else { -4.0 }).collect();
        for i in (0..soft.len()).step_by(37) {
            soft[i] = -soft[i] * 0.25; // weak, wrong-sign bit
        }
        let mut dec = FecBDecoder::new();
        for s in soft {
            dec.feed_bit(s);
        }
        let text = String::from_utf8_lossy(&dec.take()).to_string();
        assert!(text.contains(msg), "decoded {text:?}");
    }

    /// A resync must clear the LTRS/FIGS shift (navtex.cxx:1639-1642): a message
    /// that leaves the decoder in figures mode, then a loss of lock, then a
    /// letters-only message must decode as letters — not figures — even though
    /// the second stream carries no leading LTRS.
    #[test]
    fn fecb_resync_resets_figs_shift() {
        let c = Ccir476::new();
        let figs_msg = "12 907"; // pure figures — leaves the decoder in FIGS
        let ltrs_msg = "CQ DE"; // pure letters, encoded assuming LTRS state
        let mut bits = Vec::new();
        for _ in 0..3 {
            bits.extend(codes_to_bits(&create_fec(&c.encode(figs_msg))));
        }
        // A long invalid-code burst forces the decoder to lose lock and resync.
        bits.extend(std::iter::repeat_n(true, 300)); // all-mark → weight-7, never valid
        for _ in 0..3 {
            bits.extend(codes_to_bits(&create_fec(&c.encode(ltrs_msg))));
        }
        let mut dec = FecBDecoder::new();
        for b in bits {
            dec.feed_bit(if b { 1.0 } else { -1.0 });
        }
        let text = String::from_utf8_lossy(&dec.take()).to_string();
        assert!(text.contains(ltrs_msg), "letters after resync mis-decoded: {text:?}");
    }
}
