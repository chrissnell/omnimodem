//! MT63 family: 64-carrier overlapping-Walsh OFDM with deep interleave, ported
//! from fldigi (`fldigi/src/mt63/{mt63.cxx,mt63base.cxx,dsp.cxx}`, upstream
//! 4.1.23 @ 61b97f413). Six submodes — bandwidth ∈ {500,1000,2000} Hz ×
//! interleave ∈ {Short=32, Long=64} — all parametric over the OFDM core in
//! `frontend::ofdm`.
//!
//! The wire-determining integer domain (Walsh spread + block interleave →
//! `MT63encoder.Output`, and the per-carrier `TxVect` DBPSK phase indices) is
//! asserted bit-exact vs fldigi in `frontend::ofdm` (golden vector
//! `tests/vectors/mt63.json`). The windowed OFDM audio is loopback-gated, never
//! sample-exact (Doctrine §3), matching the DominoEX/Olivia/PSK precedent.
//!
//! TX frames a message the way fldigi's `mt63::tx_process` does: `DataInterleave`
//! leading NUL flush characters, the text, then a `DataInterleave` NUL flush
//! tail so the encoder interleaver drains — fldigi's exact on-air length, not
//! the receiver's extra deinterleaver delay (`flush_tail`). The streaming demod
//! assumes symbol alignment (the
//! ±carrier FEC-scan synchroniser/AFC is deferred with the sync tracker); the
//! fldigi cross-decode is the `#[ignore]` nightly gate. ref: mt63.cxx:60-165.
//!
//! Scope: 7-bit ASCII only. fldigi's optional 8-bit mode (the `c==127` escape →
//! `c+128`; mt63.cxx:133-148, 210-221, gated on `progdefaults.mt63_8bit`) is
//! intentionally not ported — RX emits printable + whitespace text and drops the
//! flush NULs / control codes.

use crate::frontend::ofdm::{Interleave, Mt63Modem, Mt63Rx, AUDIO_RATE};
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

/// The six MT63 submodes. Each fixes bandwidth + interleave depth. ref:
/// mt63.cxx:347-375.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mt63Variant {
    B500Short,
    B500Long,
    B1000Short,
    B1000Long,
    B2000Short,
    B2000Long,
}

impl Mt63Variant {
    pub fn bandwidth(self) -> u32 {
        use Mt63Variant::*;
        match self {
            B500Short | B500Long => 500,
            B1000Short | B1000Long => 1000,
            B2000Short | B2000Long => 2000,
        }
    }

    pub fn interleave(self) -> Interleave {
        use Mt63Variant::*;
        match self {
            B500Short | B1000Short | B2000Short => Interleave::Short,
            B500Long | B1000Long | B2000Long => Interleave::Long,
        }
    }

    pub fn from_label(s: &str) -> Option<Mt63Variant> {
        use Mt63Variant::*;
        Some(match s {
            "mt63_500s" => B500Short,
            "mt63_500l" => B500Long,
            "mt63_1000s" => B1000Short,
            "mt63_1000l" => B1000Long,
            "mt63_2000s" => B2000Short,
            "mt63_2000l" => B2000Long,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        use Mt63Variant::*;
        match self {
            B500Short => "mt63_500s",
            B500Long => "mt63_500l",
            B1000Short => "mt63_1000s",
            B1000Long => "mt63_1000l",
            B2000Short => "mt63_2000s",
            B2000Long => "mt63_2000l",
        }
    }

    /// Every ported submode, for table-driven tests and the TUI selector.
    pub fn all() -> &'static [Mt63Variant] {
        use Mt63Variant::*;
        &[B500Short, B500Long, B1000Short, B1000Long, B2000Short, B2000Long]
    }
}

fn caps(v: Mt63Variant) -> ModeCaps {
    ModeCaps {
        native_rate: AUDIO_RATE as u32,
        bandwidth_hz: v.bandwidth() as f32,
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

/// Trailing NUL symbols appended after the text, sized to fldigi's on-air flush.
/// The encoder's block interleaver delays each char by up to `DataInterleave`
/// symbols, so exactly `DataInterleave` trailing NULs push the last real char
/// out into the audio — this is fldigi's own tail (`flush = DataInterleave`
/// NULs + `SendJam`, mt63.cxx:104-120). The *decoder's* deinterleaver adds
/// another `DataInterleave` of delay, but that is drained by the trailing
/// channel samples a receiver keeps processing after the carrier drops — not by
/// keying PTT on `2·depth` of extra tone. The earlier `2·depth + 8` roughly
/// doubled the tail, so MT63-500L (200-sample symbols, ÷8 decimation ⇒ 0.2 s
/// each) held TX ~15 s longer than the reference. ref: mt63.cxx:95-120.
fn flush_tail(intlv: Interleave) -> usize {
    intlv.depth()
}

/// The framed character stream for a message: leading + trailing NUL flush.
fn framed(text: &str, intlv: Interleave) -> Vec<u8> {
    let d = intlv.depth();
    let mut v = vec![0u8; d];
    v.extend(text.bytes().map(|b| b & 0x7f)); // 7-bit ASCII (mt63.cxx:133)
    v.extend(std::iter::repeat_n(0u8, flush_tail(intlv)));
    v
}

pub struct Mt63Mod {
    modem: Mt63Modem,
}

impl Mt63Mod {
    pub fn new(v: Mt63Variant, center_hz: f32) -> Self {
        Mt63Mod { modem: Mt63Modem::new(v.bandwidth(), v.interleave(), center_hz) }
    }
}

impl Modulator for Mt63Mod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: AUDIO_RATE as u32,
            bandwidth_hz: self.modem.geometry().bandwidth as f32,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("mt63 needs text")),
        };
        Ok(self.modem.modulate_chars(&framed(text, self.modem.interleave())))
    }
}

pub struct Mt63Demod {
    v: Mt63Variant,
    rx: Mt63Rx,
}

impl Mt63Demod {
    pub fn new(v: Mt63Variant, center_hz: f32) -> Self {
        let modem = Mt63Modem::new(v.bandwidth(), v.interleave(), center_hz);
        Mt63Demod { v, rx: Mt63Rx::new(modem) }
    }
}

/// Map a decoded character code onto an output frame, dropping the NUL flush and
/// non-text control codes (fldigi's `put_rx_char` in 7-bit mode passes them to
/// the UI, which shows only printable + whitespace). ref: mt63.cxx:205-222.
fn push_code(out: &mut Vec<Frame>, code: u8, v: Mt63Variant) {
    let c = code as char;
    let keep = matches!(code, 0x20..=0x7e) || code == b'\n' || code == b'\r' || code == b'\t';
    if keep {
        out.push(Frame {
            payload: FramePayload::Text(c.to_string()),
            meta: FrameMeta { crc_ok: true, decoder: Some(v.label().into()), ..Default::default() },
        });
    }
}

impl Demodulator for Mt63Demod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        let codes = self.rx.feed(samples);
        let mut out = Vec::new();
        for c in codes {
            push_code(&mut out, c, self.v);
        }
        out
    }

    fn reset(&mut self) {
        self.rx.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_round_trip() {
        for &v in Mt63Variant::all() {
            assert_eq!(Mt63Variant::from_label(v.label()), Some(v));
        }
        assert_eq!(Mt63Variant::from_label("mt63_9000x"), None);
        assert_eq!(Mt63Variant::all().len(), 6);
    }

    #[test]
    fn params_map_bandwidth_and_interleave() {
        assert_eq!(Mt63Variant::B1000Long.bandwidth(), 1000);
        assert_eq!(Mt63Variant::B1000Long.interleave(), Interleave::Long);
        assert_eq!(Mt63Variant::B500Short.interleave(), Interleave::Short);
        assert_eq!(Mt63Variant::B2000Short.bandwidth(), 2000);
    }

    fn loopback(v: Mt63Variant, msg: &str) -> String {
        let mut tx = Mt63Mod::new(v, 1500.0);
        let mut samples = tx.modulate(&Frame::text(msg)).unwrap();
        // The TX tail is fldigi's on-air length (drains the *encoder*). A real
        // receiver drains its *deinterleaver* from the channel samples that keep
        // arriving after the carrier drops; model that here with trailing silence
        // rather than keying extra tone. ref: flush_tail / mt63.cxx:95-120.
        let modem = Mt63Modem::new(v.bandwidth(), v.interleave(), 1500.0);
        samples.extend(std::iter::repeat_n(0.0f32, v.interleave().depth() * modem.sym_len + modem.win_len));
        let mut rx = Mt63Demod::new(v, 1500.0);
        // feed in irregular chunks to exercise the streaming buffer/drain
        let mut frames = Vec::new();
        for chunk in samples.chunks(777) {
            frames.extend(rx.feed(chunk));
        }
        let s: String = frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        s
    }

    #[test]
    fn streaming_loopback_recovers_message_all_submodes() {
        let msg = "CQ DE K1ABC/7 EM73";
        for &v in Mt63Variant::all() {
            let out = loopback(v, msg);
            assert!(out.contains(msg), "{}: got {out:?}", v.label());
        }
    }

    /// Regression: the TX flush is bounded to fldigi's on-air length — a
    /// `DataInterleave` leading + `DataInterleave` trailing NUL frame — not the
    /// earlier `2·depth + 8` tail, which roughly doubled MT63-500L's keyed time
    /// (~41 s → ~27 s for two characters). Asserts the modulated length equals
    /// the payload+flush character chain exactly, so any tail bloat regresses.
    /// ref: flush_tail / mt63.cxx:95-120.
    #[test]
    fn tx_flush_is_bounded_to_fldigi_length() {
        let msg = "CQ";
        for &v in Mt63Variant::all() {
            let modem = Mt63Modem::new(v.bandwidth(), v.interleave(), 1500.0);
            let n_chars = framed(msg, v.interleave()).len();
            let expect = n_chars * modem.sym_len + modem.win_len + modem.sym_len;
            let samples = Mt63Mod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap().len();
            assert_eq!(samples, expect, "{} tail length", v.label());
        }
        // The 500L case the user hit: two chars must stay near fldigi's floor
        // (leading+trailing flush is intrinsic to the mode), well under the old
        // ~41 s the `2·depth + 8` tail produced.
        let s500l = Mt63Mod::new(Mt63Variant::B500Long, 1500.0)
            .modulate(&Frame::text("CQ"))
            .unwrap()
            .len();
        let secs = s500l as f32 / AUDIO_RATE;
        assert!(secs < 30.0, "mt63_500l two-char TX {secs:.1}s should be ~fldigi length");
    }

    #[test]
    fn streaming_loopback_punctuation_and_digits() {
        let msg = "MT63 2000L @ 73! (test) 0123456789";
        assert!(loopback(Mt63Variant::B1000Short, msg).contains(msg));
    }
}
