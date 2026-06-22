//! JT65 mode assembly: 65-FSK (1 sync tone + 64 data tones), RS(63,12) over
//! GF(2⁶), 72-bit legacy message, 60 s transmissions on the minute. Building
//! blocks: `frontend::modulate::MFsk`, `fec::rs_gf64::RsGf64`,
//! `framing::message77::legacy::{pack72,unpack72}`, and the shared
//! `modes::fsk_util` tone detector. Reference: WSJT-X `jt65`.
//!
//! Frame layout (self-consistent; exact WSJT-X sync-vector interleave is the
//! `#[ignore]` cross-decode gate): the 63 RS codeword symbols are interleaved
//! with the sync tone — `[sync, data0, sync, data1, ...]` — giving 126 symbols.
//! Sync is tone 0; data symbol `s` (0..63) rides tone `s + 1`.

use crate::fec::rs_gf64::RsGf64;
use crate::framing::message77::legacy::{pack72, unpack72};
use crate::frontend::modulate::MFsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::detect_symbols;
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const JT65_RATE: u32 = 11_025;
pub const JT65_SPS: usize = 4096; // ~2.69 baud
pub const JT65_BAUD: f32 = JT65_RATE as f32 / JT65_SPS as f32;
pub const JT65_TONES: u32 = 65; // tone 0 = sync, tones 1..64 = data
pub const JT65_SYMBOLS: usize = 126; // 63 data interleaved with 63 sync
pub const JT65_WINDOW_S: f32 = 60.0;
const JT65_BASE_HZ: f32 = 1000.0;

fn caps() -> ModeCaps {
    ModeCaps {
        native_rate: JT65_RATE,
        bandwidth_hz: JT65_TONES as f32 * JT65_BAUD,
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Windowed { window_s: JT65_WINDOW_S, period_s: JT65_WINDOW_S },
    }
}

/// Pack 72 message bits (MSB first) into 12 six-bit RS symbols.
fn bits72_to_rs_symbols(bits: &[u8; 72]) -> [u8; 12] {
    let mut out = [0u8; 12];
    for (i, slot) in out.iter_mut().enumerate() {
        let mut s = 0u8;
        for j in 0..6 {
            s = (s << 1) | (bits[i * 6 + j] & 1);
        }
        *slot = s;
    }
    out
}

/// Inverse of [`bits72_to_rs_symbols`].
fn rs_symbols_to_bits72(syms: &[u8; 12]) -> [u8; 72] {
    let mut out = [0u8; 72];
    for (i, &s) in syms.iter().enumerate() {
        for j in 0..6 {
            out[i * 6 + j] = (s >> (5 - j)) & 1;
        }
    }
    out
}

pub struct Jt65Mod {
    rs: RsGf64,
}

impl Jt65Mod {
    pub fn new() -> Self {
        Jt65Mod { rs: RsGf64::jt65() }
    }
}

impl Default for Jt65Mod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for Jt65Mod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("jt65 needs text")),
        };
        let bits = pack72(&text).ok_or_else(|| ModError::TooLong(text.clone()))?;
        let data_syms = bits72_to_rs_symbols(&bits);
        let cw = self.rs.encode(&data_syms); // 63 codeword symbols
        let mut symbols = Vec::with_capacity(JT65_SYMBOLS);
        for &s in &cw {
            symbols.push(0); // sync tone
            symbols.push(s as u32 + 1); // data tone offset past the sync tone
        }
        let mfsk = MFsk::new(JT65_RATE as f32, JT65_SPS, JT65_BASE_HZ, JT65_BAUD, JT65_TONES);
        Ok(mfsk.modulate(&symbols))
    }
}

pub struct Jt65Demod {
    rs: RsGf64,
}

impl Jt65Demod {
    pub fn new() -> Self {
        Jt65Demod { rs: RsGf64::jt65() }
    }
}

impl Default for Jt65Demod {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockDemodulator for Jt65Demod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn decode_window(&mut self, window: &[Sample], _start_ns: u64) -> Vec<Frame> {
        if window.len() < JT65_SYMBOLS * JT65_SPS {
            return Vec::new();
        }
        let syms = detect_symbols(
            window, 0, JT65_SPS, JT65_SYMBOLS, JT65_RATE as f32, JT65_BASE_HZ, JT65_BAUD,
            JT65_TONES,
        );
        // Data symbols are the odd positions; map tone back to RS symbol value.
        let mut cw = [0u8; 63];
        for (i, slot) in cw.iter_mut().enumerate() {
            let tone = syms[i * 2 + 1];
            *slot = tone.saturating_sub(1).min(63) as u8;
        }
        let Some(data) = self.rs.decode(&cw) else {
            return Vec::new();
        };
        let bits = rs_symbols_to_bits72(&data);
        match unpack72(&bits) {
            Some(text) => vec![Frame {
                payload: FramePayload::Text(text),
                meta: FrameMeta {
                    crc_ok: true,
                    decoder: Some("jt65".into()),
                    ..Default::default()
                },
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
            Jt65Mod::new().caps().shape,
            DemodShape::Windowed { window_s, .. } if (window_s - 60.0).abs() < 0.1
        ));
    }

    #[test]
    fn bits_rs_symbols_round_trip() {
        let bits: [u8; 72] = std::array::from_fn(|i| ((i * 5 + 1) % 2) as u8);
        let syms = bits72_to_rs_symbols(&bits);
        assert_eq!(rs_symbols_to_bits72(&syms), bits);
    }

    #[test]
    fn loopback_decodes_message() {
        let msg = "K1ABC W9XYZ EN37";
        let mut tx = Jt65Mod::new();
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let n = (JT65_RATE as f32 * JT65_WINDOW_S) as usize;
        let mut window = samples.clone();
        window.resize(n, 0.0);
        let mut rx = Jt65Demod::new();
        let decodes = rx.decode_window(&window, 0);
        assert!(
            decodes.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t == msg)),
            "no JT65 decode: {decodes:?}"
        );
    }
}
