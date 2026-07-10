//! DominoEX family: 18-tone IFK+ (incremental frequency keying) MFSK with the
//! self-framing DominoEX Varicode, parametric by submode.
//!
//! Port of fldigi's DominoEX modem (`fldigi/src/dominoex/{dominoex.cxx,
//! dominovar.cxx}`, upstream 4.1.23 @ 61b97f413). DominoEX is MFSK where each
//! symbol is a *tone offset* relative to the previous tone (differential in
//! frequency), not an absolute tone: `tone = (prev + 2 + nibble) % 18`, where the
//! `+2` guard keeps successive tones ≥2 apart so a repeat is never sent. Text is
//! carried by the DominoEX Varicode (`framing::dominoex_varicode`), a self-framing
//! 1–3 nibble code. ref: dominoex.cxx:651-681 (`sendsymbol`/`sendchar`),
//! 397-419 (`decodesymbol` IFK+ inverse), dominoex.h:46 (`NUMTONES`).
//!
//! Wire-determining arithmetic is bit-exact vs fldigi: the Varicode nibble stream
//! and the IFK+ tone-index sequence are asserted byte-for-byte against vectors
//! extracted from the unmodified fldigi tables (`tests/vectors/dominoex_varicode.
//! json`, `scratch/refvectors/build_dominoex_varicode.sh`). Modulated audio is
//! gated on a loopback decode only, never bit-exact (Doctrine §3): fldigi's
//! `sendtone` audio path is entangled with its modem/FLTK runtime.
//!
//! The streaming demod assumes symbol alignment to the fed buffer — sync, AFC and
//! the TX framing envelope (idle/STX/EOT preamble) of real DominoEX are deferred;
//! the loopback is the gate and cross-decode against fldigi is the `#[ignore]`
//! nightly gate, matching the `olivia`/`psk` precedent. The optional MultiPsk
//! (`DOMINOEX_FEC`) secondary path is a separate Phase-9b slice.

use crate::framing::dominoex_varicode::{encode_char, Framer, NUMTONES};
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::{argmax, tone_powers};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

/// The DominoEX submodes ported here. Each fixes `(symlen, doublespaced,
/// samplerate)`; everything else (tone spacing, baud, samples/symbol) derives
/// from those. ref: dominoex.cxx:220-275.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DominoVariant {
    Micro,
    D4,
    D5,
    D8,
    D11,
    D16,
    D22,
    D44,
    D88,
}

/// Resolved per-submode parameters. ref: dominoex.cxx:220-277.
#[derive(Debug, Clone, Copy)]
pub struct DominoParams {
    /// Samples per symbol at `samplerate` (fldigi `symlen`).
    pub symlen: usize,
    /// IFK+ tone doubling factor (fldigi `doublespaced`): tone spacing multiplier.
    pub doublespaced: usize,
    /// Native sample rate (8000 or 11025 Hz).
    pub samplerate: u32,
}

impl DominoVariant {
    /// ref: dominoex.cxx:220-275.
    pub fn params(self) -> DominoParams {
        use DominoVariant::*;
        let p = |symlen, doublespaced, samplerate| DominoParams { symlen, doublespaced, samplerate };
        match self {
            Micro => p(4000, 1, 8000),
            D4 => p(2048, 2, 8000),
            D5 => p(2048, 2, 11025),
            D8 => p(1024, 2, 8000),
            D11 => p(1024, 1, 11025),
            D16 => p(512, 1, 8000),
            D22 => p(512, 1, 11025),
            D44 => p(256, 2, 11025),
            D88 => p(128, 1, 11025),
        }
    }

    pub fn samplerate(self) -> u32 {
        self.params().samplerate
    }

    /// Samples per symbol (== `symlen`).
    pub fn samples_per_symbol(self) -> usize {
        self.params().symlen
    }

    /// Tone spacing in Hz: `samplerate * doublespaced / symlen`. ref: dominoex.cxx:277.
    pub fn tone_spacing(self) -> f32 {
        let p = self.params();
        p.samplerate as f32 * p.doublespaced as f32 / p.symlen as f32
    }

    /// Occupied bandwidth: `NUMTONES * tone_spacing`. ref: dominoex.cxx:279.
    pub fn bandwidth(self) -> f32 {
        NUMTONES as f32 * self.tone_spacing()
    }

    pub fn baud(self) -> f32 {
        let p = self.params();
        p.samplerate as f32 / p.symlen as f32
    }

    pub fn from_label(s: &str) -> Option<DominoVariant> {
        use DominoVariant::*;
        Some(match s {
            "dominoexmicro" => Micro,
            "dominoex4" => D4,
            "dominoex5" => D5,
            "dominoex8" => D8,
            "dominoex11" => D11,
            "dominoex16" => D16,
            "dominoex22" => D22,
            "dominoex44" => D44,
            "dominoex88" => D88,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        use DominoVariant::*;
        match self {
            Micro => "dominoexmicro",
            D4 => "dominoex4",
            D5 => "dominoex5",
            D8 => "dominoex8",
            D11 => "dominoex11",
            D16 => "dominoex16",
            D22 => "dominoex22",
            D44 => "dominoex44",
            D88 => "dominoex88",
        }
    }

    /// Every ported submode, for table-driven tests and the TUI selector.
    pub fn all() -> &'static [DominoVariant] {
        use DominoVariant::*;
        &[Micro, D4, D5, D8, D11, D16, D22, D44, D88]
    }
}

/// The IFK+ tone-index sequence for a varicode nibble stream: `tone = (prev + 2 +
/// nibble) % NUMTONES`, `prev` starting at 0. This is the exact sequence fldigi's
/// `sendsymbol` emits (reverse = false) and is asserted bit-exact vs the golden
/// vector. ref: dominoex.cxx:651-661.
pub fn ifk_tones(nibbles: &[u8]) -> Vec<u32> {
    let mut prev = 0u32;
    nibbles
        .iter()
        .map(|&n| {
            let tone = (prev + 2 + n as u32) % NUMTONES as u32;
            prev = tone;
            tone
        })
        .collect()
}

/// The varicode nibble stream `sendchar` feeds the modulator for `text`
/// (primary alphabet). ref: dominoex.cxx:664-681.
pub fn text_nibbles(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for b in text.bytes() {
        out.extend(encode_char(b, false));
    }
    out
}

/// The IFK+ tone-index sequence for a text message (nibbles → IFK+ tones).
pub fn text_tones(text: &str) -> Vec<u32> {
    ifk_tones(&text_nibbles(text))
}

fn caps(v: DominoVariant) -> ModeCaps {
    ModeCaps {
        native_rate: v.samplerate(),
        bandwidth_hz: v.bandwidth(),
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

pub struct DominoMod {
    v: DominoVariant,
    center_hz: f32,
}

impl DominoMod {
    pub fn new(v: DominoVariant, center_hz: f32) -> Self {
        DominoMod { v, center_hz }
    }

    /// Lowest tone frequency: tone `k` sits at `center + (k - (NUMTONES-1)/2) *
    /// spacing`, so tone 0 is `center - 8.5 * spacing`. ref: dominoex.cxx:639
    /// (`f = (tone + 0.5) * tonespacing + carrier - bandwidth/2`).
    fn base_hz(&self) -> f32 {
        self.center_hz - 0.5 * (NUMTONES as f32 - 1.0) * self.v.tone_spacing()
    }
}

impl Modulator for DominoMod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("dominoex needs text")),
        };
        let tones = text_tones(text);
        let p = self.v.params();
        let mfsk = MFsk::new(
            p.samplerate as f32,
            p.symlen,
            self.base_hz(),
            self.v.tone_spacing(),
            NUMTONES as u32,
        );
        Ok(mfsk.modulate(&tones))
    }
}

pub struct DominoDemod {
    v: DominoVariant,
    center_hz: f32,
    buf: Vec<Sample>,
    framer: Framer,
    // The previous tone the IFK+ inverse differences against. Seeded to 0 — the
    // same `txprevtone` fldigi's `sendsymbol` starts from — so the first symbol
    // decodes directly. ref: dominoex.cxx:56 (`txprevtone = 0`).
    prev_tone: u32,
}

impl DominoDemod {
    pub fn new(v: DominoVariant, center_hz: f32) -> Self {
        DominoDemod { v, center_hz, buf: Vec::new(), framer: Framer::new(), prev_tone: 0 }
    }

    fn base_hz(&self) -> f32 {
        self.center_hz - 0.5 * (NUMTONES as f32 - 1.0) * self.v.tone_spacing()
    }

    /// Decode every fully-buffered symbol: detect the tone (Goertzel argmax over
    /// the 18 candidate frequencies), invert the IFK+ (`nibble = (tone - prev - 2)
    /// mod 18`; ref: dominoex.cxx:404-417), then frame it. Returns any completed
    /// characters.
    fn drain_symbols(&mut self) -> Vec<Frame> {
        let p = self.v.params();
        let sps = p.symlen;
        let spacing = self.v.tone_spacing();
        let base = self.base_hz();
        let rate = p.samplerate as f32;
        let mut out = Vec::new();
        let mut consumed = 0;
        while self.buf.len() - consumed >= sps {
            let block = &self.buf[consumed..consumed + sps];
            let powers = tone_powers(block, rate, base, spacing, NUMTONES as u32);
            let tone = argmax(&powers) as u32;
            let nibble = (tone + NUMTONES as u32 * 2 - self.prev_tone - 2) % NUMTONES as u32;
            if let Some(ch) = self.framer.push(nibble as u8) {
                push_char(&mut out, ch);
            }
            self.prev_tone = tone;
            consumed += sps;
        }
        self.buf.drain(..consumed);
        out
    }
}

/// A completed varicode value (`0..=511`) as a decoded frame. Primary values
/// (`0..=255`) are the character; secondary values (`256..=511`) belong to the
/// deferred MultiPsk secondary-text path and are dropped here.
fn push_char(out: &mut Vec<Frame>, ch: u16) {
    if ch <= 0xFF {
        if let Some(c) = char::from_u32(ch as u32) {
            out.push(Frame {
                payload: FramePayload::Text(c.to_string()),
                meta: FrameMeta { crc_ok: true, decoder: Some("dominoex".into()), ..Default::default() },
            });
        }
    }
}

impl Demodulator for DominoDemod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.buf.extend_from_slice(samples);
        self.drain_symbols()
    }

    fn flush(&mut self) -> Vec<Frame> {
        let mut out = self.drain_symbols();
        // Complete the character still in the framer at end of stream (fldigi
        // flushes with idle symbols; here EOF completes it directly).
        if let Some(ch) = self.framer.push(0) {
            push_char(&mut out, ch);
        }
        out
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.framer.reset();
        self.prev_tone = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- bit-exact KATs mirrored from the fldigi golden vector -------------
    // These run in plain `cargo test` (CI does not enable the `testutil`
    // feature); the reference-vector KAT in tests/kat.rs asserts against the same
    // JSON. Provenance: tests/vectors/dominoex_varicode.json.

    #[test]
    fn ifk_tones_match_reference_cq() {
        // ref: dominoex_varicode.json message "CQ DE K1ABC".
        let want: Vec<u32> =
            vec![5, 1, 9, 8, 10, 15, 13, 0, 10, 12, 2, 14, 2, 14, 1, 12, 0, 16, 3, 17];
        assert_eq!(text_tones("CQ DE K1ABC"), want);
    }

    #[test]
    fn nibbles_match_reference_cq() {
        // ref: dominoex_varicode.json message "CQ DE K1ABC" nibbles.
        let want: Vec<u8> =
            "3c6f03e3806a4a394e3c".chars().map(|c| c.to_digit(16).unwrap() as u8).collect();
        assert_eq!(text_nibbles("CQ DE K1ABC"), want);
    }

    #[test]
    fn every_tone_stays_in_range_and_never_repeats() {
        // IFK+ guard: successive tones differ by ≥2, so a tone is never repeated.
        let tones = text_tones("The quick brown fox 0123456789");
        assert!(tones.iter().all(|&t| t < NUMTONES as u32));
        for w in tones.windows(2) {
            assert_ne!(w[0], w[1], "IFK+ must never repeat a tone");
        }
    }

    #[test]
    fn params_derive_spacing_and_baud() {
        // ref: dominoex.cxx:220-277 spot checks.
        assert!((DominoVariant::D16.tone_spacing() - 15.625).abs() < 1e-3);
        assert!((DominoVariant::D16.baud() - 15.625).abs() < 1e-3);
        assert!((DominoVariant::Micro.baud() - 2.0).abs() < 1e-3);
        assert_eq!(DominoVariant::D5.samplerate(), 11025);
        assert_eq!(DominoVariant::D8.samples_per_symbol(), 1024);
    }

    #[test]
    fn labels_round_trip() {
        for &v in DominoVariant::all() {
            assert_eq!(DominoVariant::from_label(v.label()), Some(v));
        }
        assert_eq!(DominoVariant::from_label("dominoex99"), None);
        assert_eq!(DominoVariant::all().len(), 9);
    }

    // ---- loopback gates -----------------------------------------------------

    fn loopback(v: DominoVariant, msg: &str) -> String {
        let mut tx = DominoMod::new(v, 1500.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = DominoDemod::new(v, 1500.0);
        let mut frames = rx.feed(&samples);
        frames.extend(rx.flush());
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn loopback_recovers_text_all_submodes() {
        let msg = "CQ DE K1ABC/7 2026";
        for &v in DominoVariant::all() {
            assert_eq!(loopback(v, msg), msg, "submode {}", v.label());
        }
    }

    #[test]
    fn loopback_recovers_punctuation_and_case() {
        let msg = "The quick brown fox! (73) $5 @ 90%";
        assert_eq!(loopback(DominoVariant::D16, msg), msg);
    }
}
