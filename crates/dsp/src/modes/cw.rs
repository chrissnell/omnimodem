//! CW (Morse) mode assembly (Phase 4).
//!
//! TX: text -> Morse element string -> on/off-keyed tone via [`CwKeyer`].
//! RX: down-convert to the tone, follow the magnitude envelope with an adaptive
//! squelch, measure key-down / key-up run lengths in dot-units, and drive the
//! fuzzy [`MorseDecoder`]. One dot-unit is `1.2 / wpm` seconds (PARIS timing).

use crate::framing::morse::{encode, Element, MorseDecoder};
use crate::frontend::detector::EnvelopeDetector;
use crate::frontend::modulate::CwKeyer;
use crate::frontend::nco::DownConverter;
use crate::mode::{
    DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator,
};
use crate::types::{Frame, FramePayload, Sample};

const RATE: f32 = 8_000.0;

fn caps(tx: bool) -> ModeCaps {
    ModeCaps {
        native_rate: RATE as u32,
        bandwidth_hz: 200.0,
        tx,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

/// Render a Morse element stream as the `. - ` string [`CwKeyer`] expects.
/// Dits/dahs within a character are adjacent. [`CwKeyer`] renders every space as
/// a fixed 3-unit silence, so a 3-unit inter-character gap is one space and a
/// 7-unit word gap is emitted as enough spaces to clear the decoder's word
/// threshold (≈4.58 units) on the receive side.
fn elements_to_keyer_string(els: &[Element]) -> String {
    let mut s = String::new();
    for el in els {
        match el {
            Element::Mark(u) => s.push(if *u >= 2 { '-' } else { '.' }),
            // Intra-character gaps are implicit (adjacent symbols).
            Element::Space(u) if *u >= 5 => s.push_str("   "), // word gap
            Element::Space(u) if *u >= 2 => s.push(' '),       // inter-character gap
            Element::Space(_) => {}
        }
    }
    s
}

/// CW transmitter: `FramePayload::Text` only.
pub struct CwMod {
    keyer: CwKeyer,
}

impl CwMod {
    pub fn new(wpm: u16, tone_hz: f32) -> Self {
        CwMod { keyer: CwKeyer::new(RATE, tone_hz, wpm as f32) }
    }
}

impl Modulator for CwMod {
    fn caps(&self) -> ModeCaps {
        caps(true)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("CW carries text only")),
        };
        let morse = elements_to_keyer_string(&encode(text));
        Ok(self.keyer.modulate(&morse))
    }
}

/// A key-up gap at least this many dot-units long ends a word; on seeing one we
/// flush the completed word. Sits between the inter-character gap (3 units) and
/// the inter-word gap (7 units), matching `MorseDecoder`'s own word threshold.
const WORD_GAP_UNITS: f32 = 5.0;

/// CW receiver: envelope-keyed run-length classifier feeding a Morse decoder.
/// Unlike a framed mode, CW has no terminator per character, so it emits one
/// `Frame` per *word* — flushed when the trailing inter-word gap (or end-of-
/// stream silence) is seen. This is what makes it produce output through the
/// daemon's `feed()`-only RX path, not just via `flush()`.
pub struct CwDemod {
    wpm: u16,
    tone_hz: f32,
    dc: DownConverter,
    env: EnvelopeDetector,
    dec: MorseDecoder,
    /// Samples per dot-unit at this WPM (1.2/wpm seconds).
    unit_samples: f32,
    /// Whether the squelch is currently open (key-down).
    keyed: bool,
    /// Length of the current open/closed run, in samples.
    run: u32,
    /// True until the first key-down is seen (leading silence is not a gap).
    started: bool,
    /// Set once the current silence run has flushed its word, so the run is not
    /// flushed again nor re-fed to the next word's decoder.
    word_flushed: bool,
    sample_index: u64,
}

impl CwDemod {
    pub fn new(wpm: u16, tone_hz: f32) -> Self {
        // Attack faster than decay so the gate snaps up on a keyed element and
        // holds through the intra-tone envelope ripple; the slow floor adapts to
        // noise only while closed and opens the gate well above it.
        CwDemod {
            wpm,
            tone_hz,
            dc: DownConverter::new(tone_hz, RATE),
            env: EnvelopeDetector::new(0.02, 0.02, 0.02, 2.5),
            dec: MorseDecoder::new(),
            unit_samples: (1.2 / wpm as f32) * RATE,
            keyed: false,
            run: 0,
            started: false,
            word_flushed: false,
            sample_index: 0,
        }
    }

    fn units(&self, run: u32) -> f32 {
        run as f32 / self.unit_samples
    }

    /// Decode and emit the accumulated word, resetting the decoder for the next
    /// one. Returns `None` if nothing decoded.
    fn flush_word(&mut self) -> Option<Frame> {
        let text = std::mem::take(&mut self.dec).finish();
        let text = text.trim().to_string();
        if text.is_empty() {
            return None;
        }
        Some(Frame {
            payload: FramePayload::Text(text),
            meta: crate::types::FrameMeta {
                crc_ok: true,
                sample_offset: self.sample_index,
                decoder: Some("cw".into()),
                ..Default::default()
            },
        })
    }
}

impl Demodulator for CwDemod {
    fn caps(&self) -> ModeCaps {
        caps(false)
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        let mut out = Vec::new();
        for &x in samples {
            self.sample_index += 1;
            let mag = self.dc.push(x).norm();
            self.env.push(mag);
            let open = self.env.squelch_open();
            if open == self.keyed {
                self.run += 1;
                // A long enough silence ends the current word — flush it now so
                // output appears live (this is also how the final word emits
                // once trailing silence accumulates).
                if !open
                    && self.started
                    && !self.word_flushed
                    && self.units(self.run) >= WORD_GAP_UNITS
                {
                    if let Some(f) = self.flush_word() {
                        out.push(f);
                    }
                    self.word_flushed = true;
                }
            } else {
                let was_keyed = self.keyed;
                let run = self.run;
                if was_keyed {
                    // A mark ended; feed its duration.
                    self.started = true;
                    self.dec.key_down(self.units(run));
                } else if self.started {
                    // A silence ended (a mark is starting). If that silence was
                    // already flushed as a word gap, don't re-feed it to the
                    // fresh decoder; otherwise it's an intra-word gap.
                    if self.word_flushed {
                        self.word_flushed = false;
                    } else {
                        self.dec.key_up(self.units(run));
                    }
                }
                self.keyed = open;
                self.run = 1;
            }
        }
        out
    }

    fn reset(&mut self) {
        *self = CwDemod::new(self.wpm, self.tone_hz);
    }

    fn flush(&mut self) -> Vec<Frame> {
        self.finish_text()
    }
}

impl CwDemod {
    /// Flush accumulated state at end-of-stream into a text [`Frame`]. The final
    /// keyed element (no trailing key-up follows it) is committed here. Called
    /// by the trait `flush()` on capture teardown, and usable directly in tests.
    pub fn finish_text(&mut self) -> Vec<Frame> {
        if self.keyed && self.run > 0 {
            let run = self.run;
            self.dec.key_down(self.units(run));
            self.keyed = false;
            self.run = 0;
        }
        let text = std::mem::take(&mut self.dec).finish();
        if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![Frame::text(text)]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulator_produces_samples_for_cq() {
        let mut m = CwMod::new(20, 700.0);
        let sig = m.modulate(&Frame::text("CQ")).unwrap();
        assert!(!sig.is_empty());
        // Carrier energy present at the tone.
        let energy: f32 = sig.iter().map(|s| s * s).sum();
        assert!(energy > 1.0, "expected keyed-tone energy, got {energy}");
    }

    #[test]
    fn rejects_non_text_payload() {
        let mut m = CwMod::new(20, 700.0);
        let f = Frame::packet(vec![1, 2, 3]);
        assert!(matches!(m.modulate(&f), Err(ModError::UnsupportedPayload(_))));
    }

    #[test]
    fn loopback_recovers_cq() {
        use crate::testutil::{add_awgn, Rng};
        let wpm = 20u16;
        let tone = 700.0;
        let mut m = CwMod::new(wpm, tone);
        let mut sig = m.modulate(&Frame::text("CQ TEST")).unwrap();

        // A light, realistic noise floor: a clean (all-zero) channel gives the
        // adaptive squelch no noise reference to gate against.
        let mut rng = Rng::new(1);
        let mut lead = vec![0.0f32; 1600];
        add_awgn(&mut lead, 0.02, &mut rng);
        add_awgn(&mut sig, 0.02, &mut rng);

        let mut d = CwDemod::new(wpm, tone);
        // Words may emit live during `feed` (on their trailing gap) or at
        // `finish_text` — collect from both.
        let mut frames = d.feed(&lead);
        frames.extend(d.feed(&sig));
        frames.extend(d.finish_text());

        let decoded: String = frames
            .iter()
            .map(|f| match &f.payload {
                FramePayload::Text(t) => format!(" {t}"),
                _ => String::new(),
            })
            .collect();
        assert!(
            decoded.to_uppercase().contains("CQ") && decoded.to_uppercase().contains("TEST"),
            "loopback decoded {decoded:?}, want CQ and TEST"
        );
    }

    #[test]
    fn chunked_feed_emits_words_via_daemon_path() {
        // The daemon drives streaming demods with `feed()` per ~20 ms chunk and
        // `flush()` on capture teardown — it never calls `finish_text`. CW must
        // still produce its words. Append trailing silence so the last word's
        // gap is seen during streaming.
        use crate::testutil::{add_awgn, Rng};
        let (wpm, tone) = (20u16, 700.0);
        let mut sig = CwMod::new(wpm, tone).modulate(&Frame::text("CQ TEST")).unwrap();
        let mut rng = Rng::new(2);
        let mut lead = vec![0.0f32; 1600];
        add_awgn(&mut lead, 0.02, &mut rng);
        add_awgn(&mut sig, 0.02, &mut rng);
        // Trailing silence (with the same light noise floor) for the final word.
        let mut tail = vec![0.0f32; 8000];
        add_awgn(&mut tail, 0.02, &mut rng);

        let mut d = CwDemod::new(wpm, tone);
        let mut text = String::new();
        for chunk in lead.iter().chain(sig.iter()).chain(tail.iter()).copied()
            .collect::<Vec<_>>()
            .chunks(160)
        {
            for f in d.feed(chunk) {
                if let FramePayload::Text(t) = &f.payload {
                    text.push(' ');
                    text.push_str(t);
                }
            }
        }
        for f in d.flush() {
            if let FramePayload::Text(t) = &f.payload {
                text.push(' ');
                text.push_str(t);
            }
        }
        let up = text.to_uppercase();
        assert!(up.contains("CQ") && up.contains("TEST"), "daemon-path decoded {text:?}");
    }
}
