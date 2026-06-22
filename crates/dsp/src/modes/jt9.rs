//! JT9 mode assembly: 9-FSK, K=32 r=1/2 Fano-decoded convolutional code, 72-bit
//! legacy message, 60 s on the minute. The narrowest WSJT-X JT-family mode.
//! Building blocks: `frontend::modulate::MFsk`, `fec::fano::FanoCode`,
//! `framing::message77::legacy::{pack72,unpack72}`, and the shared
//! `modes::fsk_util` tone detector. Reference: WSJT-X `jt9`.
//!
//! Frame layout (self-consistent; exact WSJT-X sync/tone layout is the
//! `#[ignore]` cross-decode gate): one leading sync symbol on tone 8, then the
//! Fano-coded message bits packed 3-per-symbol onto data tones 0..7.

use crate::fec::fano::FanoCode;
use crate::framing::message77::legacy::{pack72, unpack72};
use crate::frontend::modulate::MFsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::{detect_symbols, symbol_to_llrs};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const JT9_RATE: u32 = 12_000;
pub const JT9_SPS: usize = 6912; // ~1.736 baud
pub const JT9_BAUD: f32 = JT9_RATE as f32 / JT9_SPS as f32;
pub const JT9_TONES: u32 = 9; // 8 data tones + 1 sync tone (index 8)
pub const JT9_SYNC_TONE: u32 = 8;
pub const JT9_WINDOW_S: f32 = 60.0;
const MSG_BITS: usize = 72;
const CODED_BITS: usize = (MSG_BITS + 31) * 2; // Fano: 2 out per (data + 31 tail)
const DATA_SYMBOLS: usize = CODED_BITS.div_ceil(3); // 3 bits/symbol on 8 tones
const TOTAL_SYMBOLS: usize = DATA_SYMBOLS + 1; // + leading sync symbol

fn caps() -> ModeCaps {
    ModeCaps {
        native_rate: JT9_RATE,
        bandwidth_hz: JT9_TONES as f32 * JT9_BAUD,
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Windowed { window_s: JT9_WINDOW_S, period_s: JT9_WINDOW_S },
    }
}

#[derive(Default)]
pub struct Jt9Mod {
    fano: FanoCode,
}

impl Jt9Mod {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Modulator for Jt9Mod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("jt9 needs text")),
        };
        let bits = pack72(&text).ok_or_else(|| ModError::TooLong(text.clone()))?;
        let coded = self.fano.encode(&bits); // CODED_BITS bits
        // Pack 3 coded bits per data symbol (MSB first), zero-padded.
        let mut symbols = Vec::with_capacity(TOTAL_SYMBOLS);
        symbols.push(JT9_SYNC_TONE);
        for chunk in coded.chunks(3) {
            let mut s = 0u32;
            for j in 0..3 {
                s = (s << 1) | (*chunk.get(j).unwrap_or(&0) as u32 & 1);
            }
            symbols.push(s);
        }
        let mfsk = MFsk::new(JT9_RATE as f32, JT9_SPS, 1000.0, JT9_BAUD, JT9_TONES);
        Ok(mfsk.modulate(&symbols))
    }
}

#[derive(Default)]
pub struct Jt9Demod {
    fano: FanoCode,
}

impl Jt9Demod {
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlockDemodulator for Jt9Demod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn decode_window(&mut self, window: &[Sample], _start_ns: u64) -> Vec<Frame> {
        if window.len() < TOTAL_SYMBOLS * JT9_SPS {
            return Vec::new();
        }
        let syms = detect_symbols(
            window, 0, JT9_SPS, TOTAL_SYMBOLS, JT9_RATE as f32, 1000.0, JT9_BAUD, JT9_TONES,
        );
        // Strip the leading sync symbol; unpack 3 bits/symbol back to LLRs.
        let mut llrs = Vec::with_capacity(DATA_SYMBOLS * 3);
        for &s in &syms[1..] {
            llrs.extend(symbol_to_llrs(s, 3, 6.0));
        }
        llrs.truncate(CODED_BITS);
        let Some(bits) = self.fano.fano_decode(&llrs, MSG_BITS, 0.5) else {
            return Vec::new();
        };
        let bits72: [u8; 72] = match bits.try_into() {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        match unpack72(&bits72) {
            Some(text) => vec![Frame {
                payload: FramePayload::Text(text),
                meta: FrameMeta { crc_ok: true, decoder: Some("jt9".into()), ..Default::default() },
            }],
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_are_windowed_60s() {
        assert!(matches!(
            Jt9Mod::new().caps().shape,
            DemodShape::Windowed { window_s, .. } if (window_s - 60.0).abs() < 0.1
        ));
    }

    #[test]
    fn loopback_decodes_message() {
        let msg = "K1ABC W9XYZ EN37";
        let mut tx = Jt9Mod::new();
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let n = (JT9_RATE as f32 * JT9_WINDOW_S) as usize;
        let mut window = samples.clone();
        window.resize(n, 0.0);
        let mut rx = Jt9Demod::new();
        let decodes = rx.decode_window(&window, 0);
        assert!(
            decodes.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t == msg)),
            "no JT9 decode: {decodes:?}"
        );
    }
}
