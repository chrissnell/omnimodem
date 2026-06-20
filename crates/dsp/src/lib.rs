//! omnimodem-dsp — mode-agnostic DSP, FEC and framing building blocks.
//!
//! Pure library: no dependency on the daemon. Every block is individually
//! testable and gated by known-answer vectors (see `tests/kat.rs`). The
//! soft-information (`Llr`) type in `types` is the spine between the
//! detector/demapper and the FEC decoder.

pub mod types;
pub mod mode;
pub mod ensemble;

pub mod frontend;
pub mod sync;
pub mod fec;
pub mod framing;
pub mod modes;

#[cfg(any(test, feature = "testutil"))]
pub mod testutil;

pub use ensemble::ParallelDemodulator;
pub use mode::{
    BlockDemodulator, DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator,
};
pub use modes::{
    afsk1200::{Afsk1200Demod, Afsk1200Ensemble, Afsk1200Mod},
    cw::{CwDemod, CwMod},
    ft8::{Ft8Demod, Ft8Mod},
    psk31::{Psk31Demod, Psk31Mod},
    rtty::{RttyDemod, RttyMod},
};
pub use types::{Cplx, Frame, FrameMeta, FramePayload, Llr, Sample, SoftBits};
