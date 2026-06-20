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
pub use types::{Cplx, Frame, FrameMeta, FramePayload, Llr, Sample, SoftBits};
