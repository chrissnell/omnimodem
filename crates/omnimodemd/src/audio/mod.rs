//! Audio subsystem: a pluggable `AudioBackend` over cpal / file / stdin, a
//! durable-identity device layer, resampling, and capture fan-out. No DSP.

pub mod alsa;
pub mod backend;
pub mod file;
pub mod fanout;
pub mod resample;
pub mod stdin;

#[cfg(not(test))]
pub mod cpal_backend;

/// A block of mono audio samples. i16 throughout, matching Graywolf's pipeline
/// and the soundcard's native format on the cheap USB adapters we target.
pub type AudioChunk = Vec<i16>;

/// Bounded depth of a capture delivery channel, in chunks (~1 s at 48 kHz with
/// 20 ms chunks). Lifted from Graywolf `CHUNK_QUEUE_DEPTH`.
pub const CHUNK_QUEUE_DEPTH: usize = 64;

/// Never open a stream above this rate. The ALSA `plughw` PCM advertises
/// synthetic resample ranges (up to 192 kHz) the codec can't honor; opening
/// above the real ceiling desyncs bit timing so every future frame fails FCS.
/// Lifted from Graywolf `MODEM_MAX_SAMPLE_RATE`. Resampling (Task 7) is
/// additive and happens *after* this capped capture, never instead of it.
pub const MAX_SAMPLE_RATE: u32 = 48_000;

/// Errors from the audio subsystem.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio device matches {0}")]
    DeviceNotFound(String),
    #[error("device {device} supports no usable capture format")]
    NoUsableFormat { device: String },
    #[error("requested rate {requested} exceeds the {ceiling} Hz ceiling")]
    RateTooHigh { requested: u32, ceiling: u32 },
    #[error("backend i/o error: {0}")]
    Io(String),
    #[error("backend unsupported on this platform")]
    Unsupported,
}
