//! Olivia mode assembly: an MFSK tone bank where each character is encoded as a
//! 64-chip Walsh block (FHT soft-decoded), configurable tones×bandwidth (default
//! 32 tones / 1000 Hz). Very robust at low SNR. Building blocks:
//! `frontend::modulate::MFsk`, `fec::fht::{walsh_encode,walsh_soft_decode}`, and
//! the shared `modes::fsk_util` tone detector. Reference: fldigi `olivia.cxx`.
//!
//! Each 6-bit character selects one of 64 Walsh sequences; the 64 ±1 chips are
//! packed `log2(tones)` per MFSK symbol (a chip's sign is one tone bit). The
//! streaming demod assumes symbol alignment to the fed buffer (sync/timing
//! recovery and the PRBS whitening of real Olivia are deferred — the loopback is
//! the gate; cross-decode against fldigi is the `#[ignore]` nightly gate).

use crate::fec::fht::{walsh_encode, walsh_soft_decode};
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::tone_powers;
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const OLIVIA_RATE: u32 = 8_000;
const WALSH_N: usize = 64; // 6-bit symbol → 64-chip Walsh block
const OLIVIA_BASE_HZ: f32 = 500.0;

/// 64-entry character set (one 6-bit Walsh symbol per character). Self-consistent
/// and invertible; real Olivia's MTEXT charset is the cross-decode concern.
const CHARSET: &[u8; 64] =
    b" ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789.";

fn char_to_symbol(ch: u8) -> usize {
    CHARSET.iter().position(|&c| c == ch).unwrap_or(0)
}

fn symbol_to_char(sym: usize) -> char {
    CHARSET[sym & 0x3F] as char
}

fn chips_per_symbol(tones: u32) -> usize {
    tones.trailing_zeros() as usize
}

/// Symbols needed to carry one 64-chip Walsh block (padded up).
fn symbols_per_char(tones: u32) -> usize {
    WALSH_N.div_ceil(chips_per_symbol(tones))
}

fn caps(bandwidth_hz: f32) -> ModeCaps {
    ModeCaps {
        native_rate: OLIVIA_RATE,
        bandwidth_hz,
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

pub struct OliviaMod {
    tones: u32,
    bandwidth_hz: f32,
}

impl OliviaMod {
    pub fn new(tones: u16, bandwidth_hz: u16) -> Self {
        OliviaMod { tones: tones as u32, bandwidth_hz: bandwidth_hz as f32 }
    }

    fn baud(&self) -> f32 {
        self.bandwidth_hz / self.tones as f32
    }

    fn sps(&self) -> usize {
        (OLIVIA_RATE as f32 / self.baud()).round() as usize
    }
}

impl Modulator for OliviaMod {
    fn caps(&self) -> ModeCaps {
        caps(self.bandwidth_hz)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("olivia needs text")),
        };
        let bits_per = chips_per_symbol(self.tones);
        let mut symbols = Vec::new();
        for ch in text.bytes() {
            let chips = walsh_encode(char_to_symbol(ch), WALSH_N); // 64 ±1 chips
            // Pack `bits_per` chips per symbol (a chip<0 ⇒ bit 1), MSB first.
            for chunk in chips.chunks(bits_per) {
                let mut sym = 0u32;
                for j in 0..bits_per {
                    let bit = chunk.get(j).map(|&c| u32::from(c < 0)).unwrap_or(0);
                    sym = (sym << 1) | bit;
                }
                symbols.push(sym);
            }
        }
        let mfsk = MFsk::new(OLIVIA_RATE as f32, self.sps(), OLIVIA_BASE_HZ, self.baud(), self.tones);
        Ok(mfsk.modulate(&symbols))
    }
}

pub struct OliviaDemod {
    tones: u32,
    bandwidth_hz: f32,
    buf: Vec<Sample>,
}

impl OliviaDemod {
    pub fn new(tones: u16, bandwidth_hz: u16) -> Self {
        OliviaDemod { tones: tones as u32, bandwidth_hz: bandwidth_hz as f32, buf: Vec::new() }
    }

    fn baud(&self) -> f32 {
        self.bandwidth_hz / self.tones as f32
    }

    fn sps(&self) -> usize {
        (OLIVIA_RATE as f32 / self.baud()).round() as usize
    }
}

impl crate::mode::Demodulator for OliviaDemod {
    fn caps(&self) -> ModeCaps {
        caps(self.bandwidth_hz)
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.buf.extend_from_slice(samples);
        let sps = self.sps();
        let bits_per = chips_per_symbol(self.tones);
        let syms_per_char = symbols_per_char(self.tones);
        let need = sps * syms_per_char;
        let mut out = Vec::new();
        // Decode whole characters (aligned to the buffer start) as they arrive.
        while self.buf.len() >= need {
            let mut chips: Vec<f32> = Vec::with_capacity(syms_per_char * bits_per);
            for s in 0..syms_per_char {
                let seg = &self.buf[s * sps..(s + 1) * sps];
                let powers = tone_powers(seg, OLIVIA_RATE as f32, OLIVIA_BASE_HZ, self.baud(), self.tones);
                let tone = crate::modes::fsk_util::argmax(&powers) as u32;
                for j in 0..bits_per {
                    let bit = (tone >> (bits_per - 1 - j)) & 1;
                    chips.push(if bit == 0 { 1.0 } else { -1.0 });
                }
            }
            chips.truncate(WALSH_N);
            let (sym, _mag) = walsh_soft_decode(&chips);
            out.push(Frame {
                payload: FramePayload::Text(symbol_to_char(sym).to_string()),
                meta: FrameMeta { crc_ok: true, decoder: Some("olivia".into()), ..Default::default() },
            });
            self.buf.drain(..need);
        }
        out
    }

    fn reset(&mut self) {
        self.buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Demodulator;

    #[test]
    fn caps_reflect_tone_bandwidth() {
        let m = OliviaMod::new(32, 1000);
        assert_eq!(m.caps().bandwidth_hz, 1000.0);
        assert!((m.baud() - 31.25).abs() < 0.01);
    }

    #[test]
    fn loopback_recovers_text() {
        let msg = "TEST OLIVIA";
        let mut tx = OliviaMod::new(32, 1000);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = OliviaDemod::new(32, 1000);
        let text: String = rx
            .feed(&samples)
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, msg, "got {text:?}");
    }
}
