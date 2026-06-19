//! Baudot / ITA2 (5-bit) code with LTRS/FIGS shift handling, for RTTY.
//!
//! ITA2 is a 5-bit code with two shift states: **LTRS** (letters) and **FIGS**
//! (figures/symbols). The two shift codes (`0x1F` = LTRS, `0x1B` = FIGS) toggle
//! the active table; all other codes are interpreted per the current shift.
//!
//! Bit order: each character is 5 bits. On the RTTY wire these are sent
//! **LSB-first** (bit 1 first) framed by a start bit and 1.5 stop bits. This
//! module deals in the 5-bit code *values* (0..=31); the modulator applies the
//! LSB-first start/stop framing. The encoder inserts shift codes as needed.

const LTRS: u8 = 0x1F;
const FIGS: u8 = 0x1B;

/// Letters table indexed by 5-bit code; `'\0'` marks shift/unused slots.
const LETTERS: [char; 32] = [
    '\0', 'E', '\n', 'A', ' ', 'S', 'I', 'U', '\r', 'D', 'R', 'J', 'N', 'F', 'C', 'K', 'T', 'Z',
    'L', 'W', 'H', 'Y', 'P', 'Q', 'O', 'B', 'G', '\0', 'M', 'X', 'V', '\0',
];

/// US-TTY figures table indexed by 5-bit code.
const FIGURES: [char; 32] = [
    '\0', '3', '\n', '-', ' ', '\x07', '8', '7', '\r', '$', '4', '\'', ',', '!', ':', '(', '5',
    '"', ')', '2', '#', '6', '0', '1', '9', '?', '&', '\0', '.', '/', ';', '\0',
];

/// Shift state for ITA2 coding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shift {
    Letters,
    Figures,
}

fn find(table: &[char; 32], c: char) -> Option<u8> {
    table.iter().position(|&x| x == c).map(|i| i as u8)
}

/// Encode text to a stream of 5-bit ITA2 codes, inserting LTRS/FIGS shift
/// codes whenever the required table differs from the current shift. The
/// stream starts in the Letters state (matching a typical idle line).
pub fn encode(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut shift = Shift::Letters;
    for c in text.to_ascii_uppercase().chars() {
        // CR/LF/space exist identically in both tables; emit without a shift.
        if let (Some(_), Some(fig)) = (find(&LETTERS, c), find(&FIGURES, c)) {
            // Ambiguous-but-identical slot (space, CR, LF): use current table.
            let code = match shift {
                Shift::Letters => find(&LETTERS, c).unwrap(),
                Shift::Figures => fig,
            };
            out.push(code);
            continue;
        }
        if let Some(code) = find(&LETTERS, c) {
            if shift != Shift::Letters {
                out.push(LTRS);
                shift = Shift::Letters;
            }
            out.push(code);
        } else if let Some(code) = find(&FIGURES, c) {
            if shift != Shift::Figures {
                out.push(FIGS);
                shift = Shift::Figures;
            }
            out.push(code);
        }
        // Untranslatable characters are dropped.
    }
    out
}

/// Stateful ITA2 decoder tracking the LTRS/FIGS shift across calls.
pub struct Decoder {
    shift: Shift,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Decoder { shift: Shift::Letters }
    }

    /// Feed one 5-bit code, returning the decoded char (shift codes return
    /// `None` and update internal state).
    pub fn feed(&mut self, code: u8) -> Option<char> {
        match code {
            LTRS => {
                self.shift = Shift::Letters;
                None
            }
            FIGS => {
                self.shift = Shift::Figures;
                None
            }
            _ => {
                let table = match self.shift {
                    Shift::Letters => &LETTERS,
                    Shift::Figures => &FIGURES,
                };
                let c = table[(code & 0x1F) as usize];
                if c == '\0' {
                    None
                } else {
                    Some(c)
                }
            }
        }
    }

    /// Decode a full code stream to text.
    pub fn decode(&mut self, codes: &[u8]) -> String {
        codes.iter().filter_map(|&c| self.feed(c)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ryry_123_roundtrip_with_shift_tracking() {
        let text = "RYRY 123";
        let codes = encode(text);
        // Must contain a FIGS shift before the digits.
        assert!(codes.contains(&FIGS), "expected FIGS shift inserted");
        let decoded = Decoder::new().decode(&codes);
        assert_eq!(decoded, text);
    }

    #[test]
    fn figs_then_letters_tracks_shift() {
        // A digit followed by letters must reinsert LTRS.
        let codes = encode("1A");
        let mut saw_figs = false;
        let mut saw_ltrs_after = false;
        for &c in &codes {
            if c == FIGS {
                saw_figs = true;
            }
            if c == LTRS && saw_figs {
                saw_ltrs_after = true;
            }
        }
        assert!(saw_figs && saw_ltrs_after);
        assert_eq!(Decoder::new().decode(&codes), "1A");
    }

    #[test]
    fn codes_are_five_bit() {
        for &c in &encode("THE QUICK BROWN FOX 0123456789") {
            assert!(c < 32, "code {c} exceeds 5 bits");
        }
    }

    #[test]
    fn lsb_first_value_is_low_five_bits() {
        // 'E' is code 0x01 — its LSB-first wire bit is a 1 then four 0s.
        let codes = encode("E");
        assert_eq!(codes, vec![0x01]);
        let lsb_first: Vec<u8> = (0..5).map(|i| (codes[0] >> i) & 1).collect();
        assert_eq!(lsb_first, vec![1, 0, 0, 0, 0]);
    }
}
