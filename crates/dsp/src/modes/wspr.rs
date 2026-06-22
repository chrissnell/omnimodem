//! WSPR mode assembly: 4-FSK, 162 symbols, K=32 Fano FEC, bit-reversal
//! interleave, 50-bit `CALL GRID dBm` message, ~110 s transmissions starting on
//! even minutes. Building blocks: `frontend::modulate::MFsk`,
//! `fec::fano::FanoCode`, `fec::interleave::{bit_reversal_indices,permute,
//! unpermute}`, `framing::message77::legacy::{pack50,unpack50}`, and the shared
//! `modes::fsk_util` tone detector. Reference: WSJT-X `wsprsim`/`wsprd`.
//!
//! Each of the 162 symbols carries one interleaved Fano-coded data bit in the
//! high tone bit plus a fixed sync bit in the low tone bit: `tone = 2*data +
//! sync` (genuine 4-FSK). The sync vector here is a fixed deterministic pattern
//! (the exact WSJT-X sync vector is the `#[ignore]` cross-decode gate; decoding
//! does not depend on it — the data bit is recovered as `tone >> 1`).

use crate::fec::fano::FanoCode;
use crate::fec::interleave::{bit_reversal_indices, permute, unpermute};
use crate::framing::message77::legacy::{pack50, unpack50};
use crate::frontend::modulate::MFsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::detect_symbols;
use crate::types::{Frame, FrameMeta, FramePayload, Llr, Sample};

pub const WSPR_RATE: u32 = 12_000;
pub const WSPR_SPS: usize = 8192; // ~1.465 baud
pub const WSPR_BAUD: f32 = WSPR_RATE as f32 / WSPR_SPS as f32;
pub const WSPR_TONES: u32 = 4;
pub const WSPR_SYMBOLS: usize = 162;
pub const WSPR_WINDOW_S: f32 = 120.0;
const WSPR_BASE_HZ: f32 = 1400.0;
const MSG_BITS: usize = 50;

fn caps() -> ModeCaps {
    ModeCaps {
        native_rate: WSPR_RATE,
        bandwidth_hz: WSPR_TONES as f32 * WSPR_BAUD,
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Windowed { window_s: WSPR_WINDOW_S, period_s: 120.0 },
    }
}

/// Fixed deterministic 162-bit sync pattern (xorshift-seeded). Self-consistent;
/// the real WSJT-X sync vector is confirmed at cross-decode time.
fn sync_vector() -> Vec<u8> {
    let mut s: u32 = 0x5EED_1234;
    (0..WSPR_SYMBOLS)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s & 1) as u8
        })
        .collect()
}

fn parse_wspr_message(s: &str) -> Option<(String, String, u8)> {
    let mut it = s.split_whitespace();
    let call = it.next()?.to_string();
    let grid = it.next()?.to_string();
    let dbm = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((call, grid, dbm))
}

#[derive(Default)]
pub struct WsprMod {
    fano: FanoCode,
}

impl WsprMod {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Modulator for WsprMod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("wspr needs 'CALL GRID DBM'")),
        };
        let (call, grid, dbm) =
            parse_wspr_message(&text).ok_or_else(|| ModError::Encode(text.clone()))?;
        let bits = pack50(&call, &grid, dbm).ok_or_else(|| ModError::TooLong(text.clone()))?;
        let coded = self.fano.encode(&bits); // 162 coded bits
        debug_assert_eq!(coded.len(), WSPR_SYMBOLS);
        let idx = bit_reversal_indices(WSPR_SYMBOLS);
        let interleaved = permute(&coded, &idx);
        let sync = sync_vector();
        let symbols: Vec<u32> = interleaved
            .iter()
            .zip(sync.iter())
            .map(|(&d, &s)| 2 * d as u32 + s as u32)
            .collect();
        let mfsk = MFsk::new(WSPR_RATE as f32, WSPR_SPS, WSPR_BASE_HZ, WSPR_BAUD, WSPR_TONES);
        Ok(mfsk.modulate(&symbols))
    }
}

#[derive(Default)]
pub struct WsprDemod {
    fano: FanoCode,
}

impl WsprDemod {
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlockDemodulator for WsprDemod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn decode_window(&mut self, window: &[Sample], _start_ns: u64) -> Vec<Frame> {
        if window.len() < WSPR_SYMBOLS * WSPR_SPS {
            return Vec::new();
        }
        let tones = detect_symbols(
            window, 0, WSPR_SPS, WSPR_SYMBOLS, WSPR_RATE as f32, WSPR_BASE_HZ, WSPR_BAUD,
            WSPR_TONES,
        );
        // Recover the interleaved data bit (high tone bit), then de-interleave.
        let interleaved: Vec<u8> = tones.iter().map(|&t| (t >> 1) as u8 & 1).collect();
        let idx = bit_reversal_indices(WSPR_SYMBOLS);
        let coded = unpermute(&interleaved, &idx);
        let llrs: Vec<Llr> = coded.iter().map(|&b| if b == 0 { 6.0 } else { -6.0 }).collect();
        let Some(bits) = self.fano.fano_decode(&llrs, MSG_BITS, 0.5) else {
            return Vec::new();
        };
        let bits50: [u8; 50] = match bits.try_into() {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        match unpack50(&bits50) {
            Some((call, grid, dbm)) => vec![Frame {
                payload: FramePayload::Text(format!("{call} {grid} {dbm}")),
                meta: FrameMeta {
                    crc_ok: true,
                    decoder: Some("wspr".into()),
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
    fn caps_are_windowed_2min_slot() {
        assert!(matches!(
            WsprMod::new().caps().shape,
            DemodShape::Windowed { period_s, .. } if (period_s - 120.0).abs() < 0.1
        ));
    }

    #[test]
    fn parses_wspr_message() {
        assert_eq!(
            parse_wspr_message("K1ABC FN42 37"),
            Some(("K1ABC".into(), "FN42".into(), 37))
        );
        assert_eq!(parse_wspr_message("bad"), None);
    }

    #[test]
    fn loopback_decodes() {
        let msg = "K1ABC FN42 37";
        let mut tx = WsprMod::new();
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let n = (WSPR_RATE as f32 * WSPR_WINDOW_S) as usize;
        let mut window = samples.clone();
        window.resize(n, 0.0);
        let mut rx = WsprDemod::new();
        let d = rx.decode_window(&window, 0);
        assert!(
            d.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t == msg)),
            "no WSPR decode: {d:?}"
        );
    }
}
