//! Contestia mode assembly: Olivia's faster sibling. Like `modes::olivia` it is
//! an MFSK tone bank where each character is spread across a Walsh/Hadamard block
//! (FHT soft-decoded), but the block is **32 chips** (a 5-bit symbol) rather than
//! Olivia's 64 — trading a smaller alphabet for higher throughput at the same
//! tone count. Parametric over the fldigi Contestia grid (tones × bandwidth).
//! Reference: fldigi `contestia.cxx` (which drives the shared Olivia engine with
//! `Tones = 2·2^idx`, `bandwidth = 125·2^bw`; globals.h:57-64 lists the grid).
//!
//! Building blocks: `frontend::modulate::MFsk`, `fec::fht::{walsh_encode,
//! walsh_soft_decode}` (reused *parametrically* at N=32, exactly as the plan
//! calls for), and the shared `modes::fsk_util` tone detector. As with the
//! `olivia` assembly, the charset here is self-consistent and invertible — real
//! Contestia's compressed MTEXT alphabet + PRBS whitening are the cross-decode
//! concern (the `#[ignore]` nightly gate); the loopback is the CI gate.

use crate::fec::fht::{walsh_encode, walsh_soft_decode};
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::{argmax, tone_powers};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const CONTESTIA_RATE: u32 = 8_000;
const WALSH_N: usize = 32; // 5-bit symbol → 32-chip Walsh block (vs Olivia's 64)
const CONTESTIA_BASE_HZ: f32 = 500.0;

/// 32-entry character set (one 5-bit Walsh symbol per character). Self-consistent
/// and invertible; real Contestia's MTEXT charset is the cross-decode concern.
const CHARSET: &[u8; 32] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ01234";

fn char_to_symbol(ch: u8) -> usize {
    // Case-fold to the uppercase alphabet; unknown bytes map to space.
    let up = ch.to_ascii_uppercase();
    CHARSET.iter().position(|&c| c == up).unwrap_or(0)
}

fn symbol_to_char(sym: usize) -> char {
    CHARSET[sym & (WALSH_N - 1)] as char
}

fn chips_per_symbol(tones: u32) -> usize {
    tones.trailing_zeros() as usize
}

/// Symbols needed to carry one 32-chip Walsh block (padded up).
fn symbols_per_char(tones: u32) -> usize {
    WALSH_N.div_ceil(chips_per_symbol(tones))
}

/// The fldigi Contestia submode grid: `tones` MFSK tones over `bandwidth_hz`.
/// ref: globals.h:57-64, contestia.cxx:365-455.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContestiaVariant {
    pub tones: u16,
    pub bandwidth_hz: u16,
}

impl ContestiaVariant {
    pub const fn new(tones: u16, bandwidth_hz: u16) -> Self {
        ContestiaVariant { tones, bandwidth_hz }
    }

    pub fn baud(self) -> f32 {
        self.bandwidth_hz as f32 / self.tones as f32
    }

    fn sps(self) -> usize {
        (CONTESTIA_RATE as f32 / self.baud()).round() as usize
    }

    pub fn label(self) -> String {
        format!("contestia{}_{}", self.tones, self.bandwidth_hz)
    }

    /// Parse a `contestia<tones>_<bw>` label if it names a submode in the grid.
    pub fn from_label(s: &str) -> Option<ContestiaVariant> {
        let rest = s.strip_prefix("contestia")?;
        let (t, b) = rest.split_once('_')?;
        let v = ContestiaVariant::new(t.parse().ok()?, b.parse().ok()?);
        Self::all().iter().copied().find(|&g| g == v)
    }

    /// Every submode in the fldigi Contestia grid. ref: globals.h:57-64.
    pub fn all() -> &'static [ContestiaVariant] {
        macro_rules! v {
            ($t:expr, $b:expr) => {
                ContestiaVariant { tones: $t, bandwidth_hz: $b }
            };
        }
        static ALL: [ContestiaVariant; 19] = [
            v!(4, 125), v!(4, 250), v!(4, 500), v!(4, 1000), v!(4, 2000),
            v!(8, 125), v!(8, 250), v!(8, 500), v!(8, 1000), v!(8, 2000),
            v!(16, 250), v!(16, 500), v!(16, 1000), v!(16, 2000),
            v!(32, 1000), v!(32, 2000),
            v!(64, 500), v!(64, 1000), v!(64, 2000),
        ];
        &ALL
    }
}

fn caps(v: ContestiaVariant) -> ModeCaps {
    ModeCaps {
        native_rate: CONTESTIA_RATE,
        bandwidth_hz: v.bandwidth_hz as f32,
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

pub struct ContestiaMod {
    v: ContestiaVariant,
}

impl ContestiaMod {
    pub fn new(tones: u16, bandwidth_hz: u16) -> Self {
        ContestiaMod { v: ContestiaVariant::new(tones, bandwidth_hz) }
    }
}

impl Modulator for ContestiaMod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("contestia needs text")),
        };
        let bits_per = chips_per_symbol(self.v.tones as u32);
        let mut symbols = Vec::new();
        for ch in text.bytes() {
            let chips = walsh_encode(char_to_symbol(ch), WALSH_N); // 32 ±1 chips
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
        let mfsk = MFsk::new(
            CONTESTIA_RATE as f32,
            self.v.sps(),
            CONTESTIA_BASE_HZ,
            self.v.baud(),
            self.v.tones as u32,
        );
        Ok(mfsk.modulate(&symbols))
    }
}

pub struct ContestiaDemod {
    v: ContestiaVariant,
    buf: Vec<Sample>,
}

impl ContestiaDemod {
    pub fn new(tones: u16, bandwidth_hz: u16) -> Self {
        ContestiaDemod { v: ContestiaVariant::new(tones, bandwidth_hz), buf: Vec::new() }
    }
}

impl Demodulator for ContestiaDemod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.buf.extend_from_slice(samples);
        let sps = self.v.sps();
        let bits_per = chips_per_symbol(self.v.tones as u32);
        let syms_per_char = symbols_per_char(self.v.tones as u32);
        let need = sps * syms_per_char;
        let mut out = Vec::new();
        while self.buf.len() >= need {
            let mut chips: Vec<f32> = Vec::with_capacity(syms_per_char * bits_per);
            for s in 0..syms_per_char {
                let seg = &self.buf[s * sps..(s + 1) * sps];
                let powers = tone_powers(
                    seg,
                    CONTESTIA_RATE as f32,
                    CONTESTIA_BASE_HZ,
                    self.v.baud(),
                    self.v.tones as u32,
                );
                let tone = argmax(&powers) as u32;
                for j in 0..bits_per {
                    let bit = (tone >> (bits_per - 1 - j)) & 1;
                    chips.push(if bit == 0 { 1.0 } else { -1.0 });
                }
            }
            chips.truncate(WALSH_N);
            let (sym, _mag) = walsh_soft_decode(&chips);
            out.push(Frame {
                payload: FramePayload::Text(symbol_to_char(sym).to_string()),
                meta: FrameMeta { crc_ok: true, decoder: Some("contestia".into()), ..Default::default() },
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

    #[test]
    fn grid_has_the_fldigi_submodes() {
        // ref: globals.h:57-64 — 5+5+4+2+3 = 19 submodes.
        assert_eq!(ContestiaVariant::all().len(), 19);
        assert!(ContestiaVariant::from_label("contestia8_500").is_some());
        assert!(ContestiaVariant::from_label("contestia32_1000").is_some());
        // Not in the grid (16/125, 32/500, 64/250 are absent).
        assert!(ContestiaVariant::from_label("contestia16_125").is_none());
        assert!(ContestiaVariant::from_label("contestia64_250").is_none());
        assert!(ContestiaVariant::from_label("olivia32_1000").is_none());
    }

    #[test]
    fn labels_round_trip() {
        for &v in ContestiaVariant::all() {
            assert_eq!(ContestiaVariant::from_label(&v.label()), Some(v));
        }
    }

    #[test]
    fn baud_derives_from_tones_and_bandwidth() {
        let v = ContestiaVariant::new(8, 500);
        assert!((v.baud() - 62.5).abs() < 0.01);
        assert_eq!(ContestiaMod::new(8, 500).caps().bandwidth_hz, 500.0);
    }

    fn loopback(v: ContestiaVariant, msg: &str) -> String {
        let mut tx = ContestiaMod::new(v.tones, v.bandwidth_hz);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = ContestiaDemod::new(v.tones, v.bandwidth_hz);
        rx.feed(&samples)
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn loopback_recovers_text_across_the_grid() {
        // Uppercase + space + digits 0-4 (the self-consistent 32-char alphabet).
        let msg = "CQ DE K1ABC 2024";
        for &v in ContestiaVariant::all() {
            assert_eq!(loopback(v, msg), msg, "submode {}", v.label());
        }
    }
}
