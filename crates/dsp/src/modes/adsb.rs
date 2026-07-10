//! ADS-B (Mode S) mode: 1090 MHz extended squitter demod + loopback modulator.
//!
//! Unlike the audio modes, ADS-B is a wideband SDR mode. It consumes a
//! **magnitude** stream at a 2 MHz `native_rate` — the daemon computes
//! `|I + jQ|` from the rtl_tcp capture and feeds it here, no channelization.
//! `feed` emits each decoded Mode S frame as a `Packet` payload carrying the
//! raw 7/14 message bytes; the daemon decodes those into typed aircraft state
//! (see [`message`]) for the event stream and the TUI flights table.
//!
//! Transmit is loopback/self-test only: 1090 MHz is protected aeronautical
//! spectrum, so the modulator exists to exercise the demodulator end-to-end and
//! to render offline test vectors, never to key a radio.
//!
//! Building blocks live in submodules: [`crc`] (Mode S parity), [`ppm`] (the
//! magnitude PPM mod/demod), and [`message`] (field/CPR/altitude decode + DF17
//! frame construction).

mod crc;
mod message;
mod ppm;
mod track;

#[cfg(test)]
mod tests;

pub use message::{cpr_decode_airborne, encode_identification, AirbornePosition, ModeS};
pub use ppm::RawFrame;
pub use track::{Aircraft, AircraftTracker};

use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

/// ADS-B working rate: 2 Msps (dump1090's 2 MHz convention).
pub const ADSB_RATE: u32 = 2_000_000;
/// Samples per microsecond at [`ADSB_RATE`].
const SAMPLES_PER_US: usize = (ADSB_RATE / 1_000_000) as usize;
/// Microseconds of quiet padding around a modulated frame.
const PAD_US: usize = 8;

/// Short Mode S frame length in bits (DF 0/4/5/11/…).
pub const SHORT_FRAME_BITS: usize = 56;
/// Long (extended squitter) frame length in bits (DF 16/17/18/…).
pub const LONG_FRAME_BITS: usize = 112;

/// Half-microsecond slots in the 8 µs preamble.
pub const PREAMBLE_SLOTS: usize = 16;
/// Half-microsecond slots per data bit (one pulse-position pair).
pub const DATA_SLOTS_PER_BIT: usize = 2;

/// Preamble slots that carry a pulse — pulses at 0.0, 1.0, 3.5, 4.5 µs.
pub const PREAMBLE_HIGH_SLOTS: [usize; 4] = [0, 2, 7, 9];
/// Preamble slots that must be quiet.
pub const PREAMBLE_LOW_SLOTS: [usize; 12] = [1, 3, 4, 5, 6, 8, 10, 11, 12, 13, 14, 15];

/// True when downlink format `df` denotes a 112-bit (long) frame.
pub fn long_frame_df(df: u8) -> bool {
    matches!(df, 16 | 17 | 18 | 19 | 20 | 21 | 24 | 25 | 26 | 27 | 28 | 29 | 30 | 31)
}

fn payload_kind(p: &FramePayload) -> &'static str {
    match p {
        FramePayload::Packet(_) => "packet",
        FramePayload::Text(_) => "text",
        FramePayload::Message77(_) => "message77",
        FramePayload::Vocoder(_) => "vocoder",
        FramePayload::Image { .. } => "image",
    }
}

/// Streaming ADS-B demodulator. Buffers magnitude samples across `feed` calls so
/// a frame straddling a chunk boundary is not lost.
pub struct AdsbDemod {
    demod: ppm::PpmDemodulator,
    buf: Vec<f32>,
    /// Absolute sample index of `buf[0]` in the fed stream.
    base: u64,
}

impl AdsbDemod {
    pub fn new() -> Self {
        AdsbDemod { demod: ppm::PpmDemodulator::new(SAMPLES_PER_US), buf: Vec::new(), base: 0 }
    }

    fn emit(&self, raw: ppm::RawFrame) -> Frame {
        let crc_ok = raw.crc_ok();
        Frame {
            payload: FramePayload::Packet(raw.bytes),
            meta: FrameMeta {
                crc_ok,
                sample_offset: self.base + raw.offset as u64,
                decoder: Some("adsb".to_string()),
                ..Default::default()
            },
        }
    }

    fn drain(&mut self, frames: Vec<ppm::RawFrame>, consumed: usize) -> Vec<Frame> {
        let out: Vec<Frame> = frames.into_iter().map(|r| self.emit(r)).collect();
        self.buf.drain(..consumed);
        self.base += consumed as u64;
        out
    }
}

impl Default for AdsbDemod {
    fn default() -> Self {
        Self::new()
    }
}

impl Demodulator for AdsbDemod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: ADSB_RATE,
            bandwidth_hz: 2_000_000.0,
            tx: false,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.buf.extend_from_slice(samples);
        let (frames, consumed) = self.demod.scan(&self.buf, false);
        self.drain(frames, consumed)
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.base = 0;
    }

    fn flush(&mut self) -> Vec<Frame> {
        let (frames, consumed) = self.demod.scan(&self.buf, true);
        self.drain(frames, consumed)
    }
}

/// Loopback ADS-B modulator: a `Packet` payload (7 or 14 Mode S bytes, parity
/// already present) becomes a magnitude PPM waveform at [`ADSB_RATE`].
pub struct AdsbMod {
    modu: ppm::PpmModulator,
}

impl AdsbMod {
    pub fn new() -> Self {
        AdsbMod { modu: ppm::PpmModulator::new(SAMPLES_PER_US) }
    }
}

impl Default for AdsbMod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for AdsbMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: ADSB_RATE,
            bandwidth_hz: 2_000_000.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let bytes = match &frame.payload {
            FramePayload::Packet(b) => b,
            other => return Err(ModError::UnsupportedPayload(payload_kind(other))),
        };
        if bytes.len() != 7 && bytes.len() != 14 {
            return Err(ModError::Encode(format!(
                "adsb frame must be 7 or 14 bytes, got {}",
                bytes.len()
            )));
        }
        Ok(self.modu.modulate_padded(bytes, PAD_US, PAD_US))
    }
}
