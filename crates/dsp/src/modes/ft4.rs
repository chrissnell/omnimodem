//! FT4 mode assembly: 4-GFSK, (174,91) LDPC + CRC-14 + 77-bit message, 7.5 s
//! slots. Shares FT8's FEC/message spine (`fec::ldpc::Ldpc::ft8`, `fec::osd`,
//! `framing::message77`, the CRC-14); differs in the front-end waveform (4 tones
//! at BT=1.0 instead of 8). Reference: WSJT-X `ft4sim`/`ft4d`.
//!
//! This is a standalone assembly rather than a refactor of the FT8 decoder: it
//! reuses the public FEC/message building blocks directly, so FT8 stays
//! untouched. The 4×4 Costas-array sync of real FT4 is the `#[ignore]`
//! cross-decode concern; the loopback gate here aligns to the window start.

use crate::fec::crc::ftx_compute_crc;
use crate::fec::gray::gray_encode;
use crate::fec::ldpc::Ldpc;
use crate::fec::llr::demap_fsk;
use crate::fec::osd::osd_decode;
use crate::framing::message77::{pack77, unpack77};
use crate::frontend::modulate::Gfsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::tone_powers;
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const FT4_RATE: u32 = 12_000;
pub const FT4_SPS: usize = 512; // 23.4375 baud
pub const FT4_BAUD: f32 = FT4_RATE as f32 / FT4_SPS as f32;
pub const FT4_TONES: u32 = 4;
pub const FT4_BASE_HZ: f32 = 1000.0;
pub const FT4_WINDOW_S: f32 = 7.5;
const K91: usize = 91;
const N174: usize = 174;
const CRC_BITS: usize = 14;
const MSG_BITS: usize = 77;
const DATA_SYMBOLS: usize = N174 / 2; // 2 bits/symbol → 87 symbols

fn caps(tx: bool) -> ModeCaps {
    ModeCaps {
        native_rate: FT4_RATE,
        bandwidth_hz: FT4_TONES as f32 * FT4_BAUD,
        tx,
        duplex: Duplex::Half,
        shape: DemodShape::Windowed { window_s: FT4_WINDOW_S, period_s: FT4_WINDOW_S },
    }
}

/// CRC-14 over the 77-bit payload, byte-exact with ft8_lib `ftx_add_crc` (shared
/// with FT8: the 10-byte payload is CRCed over 82 bits).
fn payload_crc(payload: &[u8; 10]) -> u16 {
    let mut buf = [0u8; 11];
    buf[..10].copy_from_slice(payload);
    buf[9] &= 0xF8;
    ftx_compute_crc(&buf, 82)
}

/// text → 91 message+CRC bits → LDPC(174,91) codeword.
fn encode_codeword(message: &str) -> [u8; N174] {
    let payload = pack77(message);
    let cksum = payload_crc(&payload);
    let mut bits91 = vec![0u8; K91];
    for (i, b) in bits91.iter_mut().take(MSG_BITS).enumerate() {
        *b = (payload[i / 8] >> (7 - (i % 8))) & 1;
    }
    for i in 0..CRC_BITS {
        bits91[MSG_BITS + i] = ((cksum >> (CRC_BITS - 1 - i)) & 1) as u8;
    }
    Ldpc::ft8().encode(&bits91).try_into().expect("174-bit codeword")
}

#[derive(Default)]
pub struct Ft4Mod;

impl Ft4Mod {
    pub fn new() -> Self {
        Ft4Mod
    }
}

impl Modulator for Ft4Mod {
    fn caps(&self) -> ModeCaps {
        caps(true)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let message = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            FramePayload::Message77(m) => unpack77(m),
            _ => return Err(ModError::UnsupportedPayload("ft4 needs text/message77")),
        };
        let cw = encode_codeword(&message);
        // 2 bits/symbol (MSB first) → symbol value v → tone gray_encode(v).
        let symbols: Vec<u32> = (0..DATA_SYMBOLS)
            .map(|i| {
                let v = ((cw[2 * i] as u32) << 1) | cw[2 * i + 1] as u32;
                gray_encode(v)
            })
            .collect();
        let gfsk = Gfsk::new(FT4_RATE as f32, FT4_SPS, FT4_BASE_HZ, FT4_BAUD, 1.0);
        Ok(gfsk.modulate(&symbols))
    }
}

#[derive(Default)]
pub struct Ft4Demod;

impl Ft4Demod {
    pub fn new() -> Self {
        Ft4Demod
    }
}

impl BlockDemodulator for Ft4Demod {
    fn caps(&self) -> ModeCaps {
        caps(false)
    }

    fn decode_window(&mut self, window: &[Sample], _start_ns: u64) -> Vec<Frame> {
        if window.len() < DATA_SYMBOLS * FT4_SPS {
            return Vec::new();
        }
        // Per-symbol 4-tone powers, then a noise normalizer from the mean power.
        let rows: Vec<Vec<f32>> = (0..DATA_SYMBOLS)
            .map(|i| {
                let seg = &window[i * FT4_SPS..(i + 1) * FT4_SPS];
                tone_powers(seg, FT4_RATE as f32, FT4_BASE_HZ, FT4_BAUD, FT4_TONES)
            })
            .collect();
        let (sum, cnt) = rows.iter().flatten().fold((0.0f64, 0usize), |(s, c), &v| (s + v as f64, c + 1));
        let noise_var = ((sum / cnt.max(1) as f64) as f32).max(f32::MIN_POSITIVE);

        let mut llrs = Vec::with_capacity(N174);
        for row in &rows {
            llrs.extend(demap_fsk(row, noise_var));
        }
        if llrs.len() != N174 {
            return Vec::new();
        }

        let code = Ldpc::ft8();
        let (mut cw, perr) = code.decode_minsum(&llrs, 30);
        if perr != 0 {
            match osd_decode(&code, &llrs, 2) {
                Some(better) => cw = better,
                None => return Vec::new(),
            }
        }
        let mut payload = [0u8; 10];
        for (i, &b) in cw.iter().take(MSG_BITS).enumerate() {
            payload[i / 8] |= b << (7 - (i % 8));
        }
        let mut rx_crc = 0u16;
        for i in 0..CRC_BITS {
            rx_crc = (rx_crc << 1) | cw[MSG_BITS + i] as u16;
        }
        if payload_crc(&payload) != rx_crc {
            return Vec::new();
        }
        let text = unpack77(&payload);
        if text.is_empty() {
            return Vec::new();
        }
        vec![Frame {
            payload: FramePayload::Text(text),
            meta: FrameMeta { crc_ok: true, decoder: Some("ft4".into()), ..Default::default() },
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_are_windowed_75s_tx() {
        let c = Ft4Mod::new().caps();
        assert_eq!(c.native_rate, 12_000);
        assert!(c.tx);
        assert!(matches!(c.shape, DemodShape::Windowed { window_s, .. } if (window_s - 7.5).abs() < 0.01));
    }

    #[test]
    fn modulates_a_standard_message() {
        let s = Ft4Mod::new().modulate(&Frame::text("CQ K1ABC FN42")).unwrap();
        assert_eq!(s.len(), DATA_SYMBOLS * FT4_SPS);
    }

    #[test]
    fn loopback_decodes_message() {
        let msg = "CQ K1ABC FN42";
        let mut tx = Ft4Mod::new();
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let n = (FT4_RATE as f32 * FT4_WINDOW_S) as usize;
        let mut window = samples.clone();
        window.resize(n, 0.0);
        let decodes = Ft4Demod::new().decode_window(&window, 0);
        assert!(
            decodes.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t == msg)),
            "no FT4 decode: {decodes:?}"
        );
    }
}
