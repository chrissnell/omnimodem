//! Morse / CW: element table, timing encoder, and a SOM/fuzzy decoder.
//!
//! Morse has no byte/bit wire order — it is on/off keying in the *time* domain.
//! Timing follows the standard ratios in units of one "dot" (dit):
//!   - dot = 1 unit on, dash = 3 units on
//!   - intra-character gap = 1 unit off
//!   - inter-character gap = 3 units off
//!   - word gap = 7 units off
//!
//! [`encode`] produces a sequence of keyed on/off [`Element`]s. The
//! [`MorseDecoder`] is driven by real keyed *durations* (in dot-units, e.g.
//! recovered from an envelope/edge detector) and classifies each mark/space by
//! nearest-ratio best fit, tolerating timing jitter (±20% verified in tests).

/// Standard Morse code table for the printable characters we support.
const TABLE: &[(char, &str)] = &[
    ('A', ".-"),
    ('B', "-..."),
    ('C', "-.-."),
    ('D', "-.."),
    ('E', "."),
    ('F', "..-."),
    ('G', "--."),
    ('H', "...."),
    ('I', ".."),
    ('J', ".---"),
    ('K', "-.-"),
    ('L', ".-.."),
    ('M', "--"),
    ('N', "-."),
    ('O', "---"),
    ('P', ".--."),
    ('Q', "--.-"),
    ('R', ".-."),
    ('S', "..."),
    ('T', "-"),
    ('U', "..-"),
    ('V', "...-"),
    ('W', ".--"),
    ('X', "-..-"),
    ('Y', "-.--"),
    ('Z', "--.."),
    ('0', "-----"),
    ('1', ".----"),
    ('2', "..---"),
    ('3', "...--"),
    ('4', "....-"),
    ('5', "....."),
    ('6', "-...."),
    ('7', "--..."),
    ('8', "---.."),
    ('9', "----."),
    ('.', ".-.-.-"),
    (',', "--..--"),
    ('?', "..--.."),
    ('/', "-..-."),
    ('=', "-...-"),
    ('+', ".-.-."),
    ('-', "-....-"),
];

/// One keyed event: a mark (key-down) or space (key-up) of some duration in
/// dot-units.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Element {
    /// Key-down for `units` dot-lengths (1 = dot, 3 = dash).
    Mark(u8),
    /// Key-up for `units` dot-lengths (1 = intra-char, 3 = inter-char, 7 = word).
    Space(u8),
}

fn morse_for(c: char) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|(ch, _)| *ch == c.to_ascii_uppercase())
        .map(|(_, m)| *m)
}

/// Encode text into a keyed on/off element stream in dot-units. Spaces in the
/// input become 7-unit word gaps.
pub fn encode(text: &str) -> Vec<Element> {
    let mut out = Vec::new();
    let mut first_word = true;
    for word in text.split(' ') {
        if word.is_empty() {
            continue;
        }
        if !first_word {
            out.push(Element::Space(7));
        }
        first_word = false;
        let mut first_char = true;
        for c in word.chars() {
            let Some(code) = morse_for(c) else { continue };
            if !first_char {
                out.push(Element::Space(3));
            }
            first_char = false;
            let mut first_el = true;
            for sym in code.chars() {
                if !first_el {
                    out.push(Element::Space(1));
                }
                first_el = false;
                out.push(Element::Mark(if sym == '-' { 3 } else { 1 }));
            }
        }
    }
    out
}

/// Decode a clean element stream back to text (the inverse of [`encode`]).
pub fn decode(elements: &[Element]) -> String {
    let mut out = String::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut String| {
        if !cur.is_empty() {
            if let Some(c) = from_morse(cur) {
                out.push(c);
            }
            cur.clear();
        }
    };
    for &el in elements {
        match el {
            Element::Mark(u) => cur.push(if u >= 2 { '-' } else { '.' }),
            Element::Space(u) => {
                if u >= 5 {
                    flush(&mut cur, &mut out);
                    out.push(' ');
                } else if u >= 2 {
                    flush(&mut cur, &mut out);
                }
                // u == 1 is an intra-character gap: keep accumulating.
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn from_morse(code: &str) -> Option<char> {
    TABLE.iter().find(|(_, m)| *m == code).map(|(c, _)| *c)
}

/// SOM/fuzzy best-fit Morse decoder driven by real keyed *durations* in
/// dot-units (which need not be integers). Marks are classified dot/dash and
/// spaces classified intra/inter/word by nearest standard ratio, so ±20%
/// timing jitter still decodes correctly.
pub struct MorseDecoder {
    cur: String,
    out: String,
}

impl Default for MorseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl MorseDecoder {
    pub fn new() -> Self {
        MorseDecoder { cur: String::new(), out: String::new() }
    }

    fn flush(&mut self) {
        if !self.cur.is_empty() {
            if let Some(c) = from_morse(&self.cur) {
                self.out.push(c);
            }
            self.cur.clear();
        }
    }

    /// Feed one key-down duration (dot-units). Best-fit dot (1) vs dash (3).
    pub fn key_down(&mut self, units: f32) {
        // Nearest of {1, 3}: threshold at the geometric mean (~1.73).
        self.cur.push(if units >= 2.0 { '-' } else { '.' });
    }

    /// Feed one key-up duration (dot-units). Best-fit of {1, 3, 7}.
    pub fn key_up(&mut self, units: f32) {
        // Thresholds at geometric means: 1↔3 ≈ 1.73, 3↔7 ≈ 4.58.
        if units >= 4.58 {
            self.flush();
            self.out.push(' ');
        } else if units >= 1.73 {
            self.flush();
        }
        // else: intra-character gap, keep accumulating.
    }

    /// Finalise and return the decoded text.
    pub fn finish(mut self) -> String {
        self.flush();
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cq_element_timing() {
        // C = -.-.  Q = --.-  separated by a 3-unit inter-char gap.
        let els = encode("CQ");
        let expected = vec![
            Element::Mark(3),
            Element::Space(1),
            Element::Mark(1),
            Element::Space(1),
            Element::Mark(3),
            Element::Space(1),
            Element::Mark(1),
            Element::Space(3), // inter-char
            Element::Mark(3),
            Element::Space(1),
            Element::Mark(3),
            Element::Space(1),
            Element::Mark(1),
            Element::Space(1),
            Element::Mark(3),
        ];
        assert_eq!(els, expected);
    }

    #[test]
    fn clean_stream_roundtrips() {
        for s in ["CQ", "SOS", "HELLO WORLD", "DE N0CALL"] {
            assert_eq!(decode(&encode(s)), s);
        }
    }

    #[test]
    fn fuzzy_decoder_handles_jitter() {
        // Drive the fuzzy decoder from keyed durations with ±20% jitter.
        let text = "CQ DE K1ABC";
        let els = encode(text);
        // Deterministic jitter pattern within ±20%.
        let jitter = [1.18f32, 0.82, 1.15, 0.85, 1.2, 0.8, 1.1, 0.9];
        let mut dec = MorseDecoder::new();
        for (i, el) in els.iter().enumerate() {
            let j = jitter[i % jitter.len()];
            match *el {
                Element::Mark(u) => dec.key_down(u as f32 * j),
                Element::Space(u) => dec.key_up(u as f32 * j),
            }
        }
        assert_eq!(dec.finish(), text);
    }

    #[test]
    fn fuzzy_single_char_from_mistimed_elements() {
        // 'C' = -.-. with each element off by ±20%.
        let mut dec = MorseDecoder::new();
        dec.key_down(3.5); // dash, +17%
        dec.key_up(0.85);
        dec.key_down(1.18); // dot
        dec.key_up(1.15);
        dec.key_down(3.4); // dash
        dec.key_up(0.8);
        dec.key_down(0.83); // dot
        assert_eq!(dec.finish(), "C");
    }
}
