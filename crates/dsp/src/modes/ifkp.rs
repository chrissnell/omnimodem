//! IFKP — 33-tone IFK MFSK with the self-framing IFKP Varicode, parametric by
//! speed (slow / normal / fast).
//!
//! Port of fldigi's IFKP modem (`fldigi/src/ifkp/{ifkp.cxx,ifkp_varicode.cxx}`,
//! `include/ifkp.h`, upstream 4.2.x @ ../fldigi). IFKP is IFK where each symbol
//! advances the tone by `sym + 1` over 33 tones (`tone = (prev + sym + 1) % 33`,
//! ifkp.cxx:706-710). Text is carried by the IFKP Varicode
//! (`framing::ifkp_varicode`): one symbol per character, or two when
//! `sym2 > 28`. ref: ifkp.cxx:717-728 (`send_char`), 423-480 (`process_symbol`).
//!
//! `IFKP_SR = 16000`, `IFKP_SYMLEN = 4096`, `IFKP_SPACING = 3` bins, so the tone
//! spacing is a fixed `3·16000/4096 = 11.72 Hz` regardless of speed; only the
//! emitted symbol duration changes: slow = 2× symlen, normal = 1×, fast = 0.5×
//! (ifkp.cxx:694-696), i.e. 1.95 / 3.91 / 7.81 baud.
//!
//! The symbol stream and IFK tone indices are asserted **bit-exact** against the
//! fldigi golden vector (`tests/vectors/ifkp_varicode.json`); modulated audio is
//! gated on a loopback/AWGN decode only, never bit-exact (Doctrine §3). Sync/AFC
//! and the idle preamble of real IFKP are deferred — the loopback is the gate and
//! fldigi cross-decode is the `#[ignore]` nightly gate, matching the DominoEX
//! precedent. The IFKP picture sub-protocol (`ifkp-pic.cxx`) is Phase 15.

use crate::framing::ifkp_varicode::{encode_text, Framer, OFFSET};
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::ifk33::{syms_to_tones, IfkDemod, IfkGeom, NUMTONES};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

const IFKP_SR: u32 = 16000; // ref: ifkp.cxx:58
const IFKP_SYMLEN: usize = 4096; // ref: ifkp.h:38
const IFKP_SPACING: u32 = 3; // ref: ifkp.h:45

/// The IFKP speed selector. Fixes only the emitted samples/symbol; tone spacing
/// is constant. ref: ifkp.cxx:694-696.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfkpSpeed {
    Slow,
    Normal,
    Fast,
}

impl IfkpSpeed {
    /// Emitted samples per symbol. ref: ifkp.cxx:694-696 (`symlen * factor`).
    pub fn symlen(self) -> usize {
        match self {
            IfkpSpeed::Slow => IFKP_SYMLEN * 2,
            IfkpSpeed::Normal => IFKP_SYMLEN,
            IfkpSpeed::Fast => IFKP_SYMLEN / 2,
        }
    }

    /// Baud = rate / emitted-samples-per-symbol.
    pub fn baud(self) -> f32 {
        IFKP_SR as f32 / self.symlen() as f32
    }

    pub fn from_label(s: &str) -> Option<IfkpSpeed> {
        Some(match s {
            "ifkp" | "ifkp-normal" => IfkpSpeed::Normal,
            "ifkp-slow" => IfkpSpeed::Slow,
            "ifkp-fast" => IfkpSpeed::Fast,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            IfkpSpeed::Normal => "ifkp",
            IfkpSpeed::Slow => "ifkp-slow",
            IfkpSpeed::Fast => "ifkp-fast",
        }
    }

    pub fn all() -> &'static [IfkpSpeed] {
        &[IfkpSpeed::Slow, IfkpSpeed::Normal, IfkpSpeed::Fast]
    }
}

/// Fixed tone spacing: `IFKP_SPACING · SR / IFKP_SYMLEN`. ref: ifkp.cxx:687,270.
pub fn tone_spacing() -> f32 {
    IFKP_SPACING as f32 * IFKP_SR as f32 / IFKP_SYMLEN as f32
}

/// The symbol stream `send_char` emits for `text`. ref: ifkp.cxx:717-728.
pub fn text_syms(text: &str) -> Vec<u8> {
    encode_text(text)
}

/// The IFK tone-index sequence for `text`. Asserted bit-exact vs the golden
/// vector. ref: ifkp.cxx:706-728.
pub fn text_tones(text: &str) -> Vec<u32> {
    syms_to_tones(&text_syms(text), OFFSET)
}

fn geom(speed: IfkpSpeed, center_hz: f32) -> IfkGeom {
    IfkGeom {
        rate: IFKP_SR as f32,
        symlen: speed.symlen(),
        center_hz,
        spacing_hz: tone_spacing(),
    }
}

fn caps() -> ModeCaps {
    ModeCaps {
        native_rate: IFKP_SR,
        bandwidth_hz: NUMTONES as f32 * tone_spacing(),
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

pub struct IfkpMod {
    speed: IfkpSpeed,
    center_hz: f32,
}

impl IfkpMod {
    pub fn new(speed: IfkpSpeed, center_hz: f32) -> Self {
        IfkpMod { speed, center_hz }
    }
}

impl Modulator for IfkpMod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("ifkp needs text")),
        };
        let tones = text_tones(text);
        let g = geom(self.speed, self.center_hz);
        let mfsk = MFsk::new(g.rate, g.symlen, g.base_hz(), g.spacing_hz, NUMTONES);
        Ok(mfsk.modulate(&tones))
    }
}

pub struct IfkpDemod {
    demod: IfkDemod,
    framer: Framer,
}

impl IfkpDemod {
    pub fn new(speed: IfkpSpeed, center_hz: f32) -> Self {
        IfkpDemod {
            demod: IfkDemod::new(geom(speed, center_hz), OFFSET),
            framer: Framer::new(),
        }
    }

    fn frame_syms(&mut self, syms: &[u8], out: &mut Vec<Frame>) {
        for &s in syms {
            if let Some(ch) = self.framer.push(s) {
                push_char(out, ch);
            }
        }
    }
}

fn push_char(out: &mut Vec<Frame>, ch: u8) {
    out.push(Frame {
        payload: FramePayload::Text((ch as char).to_string()),
        meta: FrameMeta { crc_ok: true, decoder: Some("ifkp".into()), ..Default::default() },
    });
}

impl Demodulator for IfkpDemod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.demod.push_samples(samples);
        let syms = self.demod.drain();
        let mut out = Vec::new();
        self.frame_syms(&syms, &mut out);
        out
    }

    fn flush(&mut self) -> Vec<Frame> {
        let syms = self.demod.drain();
        let mut out = Vec::new();
        self.frame_syms(&syms, &mut out);
        // Complete the trailing single-symbol character still in the framer:
        // a synthetic idle symbol (0) completes it, exactly as fldigi's
        // continuous idle stream does. ref: ifkp.cxx:712-715.
        if let Some(ch) = self.framer.push(0) {
            push_char(&mut out, ch);
        }
        out
    }

    fn reset(&mut self) {
        self.demod.reset();
        self.framer.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- bit-exact KATs mirrored from the fldigi golden vector -------------
    // Run in plain `cargo test`; the reference-vector KAT in tests/kat.rs asserts
    // the same values. Provenance: tests/vectors/ifkp_varicode.json.

    #[test]
    fn tones_match_reference() {
        // ref: ifkp_varicode.json message "hello world" tones.
        let want: Vec<u32> = vec![9, 15, 28, 8, 24, 20, 11, 27, 13, 26, 31];
        assert_eq!(text_tones("hello world"), want);
    }

    #[test]
    fn syms_match_reference_cq() {
        // ref: ifkp_varicode.json message "CQ CQ CQ de K1ABC" syms.
        let want: Vec<u8> = vec![
            3, 29, 17, 29, 28, 3, 29, 17, 29, 28, 3, 29, 17, 29, 28, 4, 5, 28, 11, 29, 1, 30, 1, 29,
            2, 29, 3, 29,
        ];
        assert_eq!(text_syms("CQ CQ CQ de K1ABC"), want);
    }

    #[test]
    fn params_derive_spacing_and_baud() {
        assert!((tone_spacing() - 11.71875).abs() < 1e-4);
        assert!((IfkpSpeed::Normal.baud() - 3.90625).abs() < 1e-4);
        assert!((IfkpSpeed::Slow.baud() - 1.953125).abs() < 1e-4);
        assert!((IfkpSpeed::Fast.baud() - 7.8125).abs() < 1e-4);
    }

    #[test]
    fn labels_round_trip() {
        for &s in IfkpSpeed::all() {
            assert_eq!(IfkpSpeed::from_label(s.label()), Some(s));
        }
        assert_eq!(IfkpSpeed::from_label("ifkp-normal"), Some(IfkpSpeed::Normal));
        assert_eq!(IfkpSpeed::from_label("nope"), None);
    }

    // ---- loopback gates -----------------------------------------------------

    fn loopback(speed: IfkpSpeed, msg: &str) -> String {
        let mut tx = IfkpMod::new(speed, 1500.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = IfkpDemod::new(speed, 1500.0);
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
    fn loopback_recovers_text_all_speeds() {
        let msg = "CQ CQ CQ de K1ABC/7 2026";
        for &s in IfkpSpeed::all() {
            assert_eq!(loopback(s, msg), msg, "speed {}", s.label());
        }
    }

    #[test]
    fn loopback_recovers_punctuation_and_case() {
        let msg = "The quick brown fox! (73) $5 @ 90%";
        assert_eq!(loopback(IfkpSpeed::Normal, msg), msg);
    }
}
