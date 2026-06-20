//! The mode framework: capability descriptor and the three demod/mod shapes.
//!
//! Two demod shapes are first-class (design §"Streaming AND block/windowed"):
//! `Demodulator` for continuous/HDLC modes (`feed(samples) -> Vec<Frame>`) and
//! `BlockDemodulator` for windowed multi-pass modes (FT8/WSPR). A mode
//! implements whichever fits; `ModeCaps::shape` declares which to the runtime.

use crate::types::{Frame, Sample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplex {
    Half,
    Full,
}

/// Which decode shape the runtime must drive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DemodShape {
    /// Feed samples as they arrive; frames come out when found.
    Streaming,
    /// Buffer a time-aligned window of `window_s`, decode every `period_s`.
    Windowed { window_s: f32, period_s: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeCaps {
    /// Native working sample rate in Hz (the resampler bridges to this).
    pub native_rate: u32,
    pub bandwidth_hz: f32,
    pub tx: bool,
    pub duplex: Duplex,
    pub shape: DemodShape,
}

/// Continuous/streaming demodulation.
pub trait Demodulator: Send {
    fn caps(&self) -> ModeCaps;
    /// Consume samples at `caps().native_rate`; return any frames found. Must
    /// not allocate per-sample (reuse owned scratch buffers).
    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame>;
    /// Drop all soft state (DPLL lock, AGC, partial frames).
    fn reset(&mut self);
}

/// Windowed/block multi-pass demodulation (FT8/JS8/WSPR).
pub trait BlockDemodulator: Send {
    fn caps(&self) -> ModeCaps;
    /// Decode one time-aligned window. `window_start_ns` is the wall-clock of
    /// the first sample. May return 0..N decodes (multi-pass internally).
    fn decode_window(&mut self, window: &[Sample], window_start_ns: u64) -> Vec<Frame>;
}

#[derive(Debug, thiserror::Error)]
pub enum ModError {
    #[error("payload not supported by this mode: {0}")]
    UnsupportedPayload(&'static str),
    #[error("message too long for mode: {0}")]
    TooLong(String),
    #[error("encode error: {0}")]
    Encode(String),
}

/// Symmetric transmit side: a frame's payload -> baseband audio at native rate.
pub trait Modulator: Send {
    fn caps(&self) -> ModeCaps;
    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windowed_shape_carries_grid() {
        let caps = ModeCaps {
            native_rate: 12_000,
            bandwidth_hz: 50.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Windowed { window_s: 15.0, period_s: 15.0 },
        };
        match caps.shape {
            DemodShape::Windowed { window_s, .. } => assert_eq!(window_s, 15.0),
            _ => panic!("expected windowed"),
        }
    }
}
