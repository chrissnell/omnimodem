//! Audio subsystem: a pluggable `AudioBackend` over cpal / file / stdin, a
//! durable-identity device layer, resampling, and capture fan-out. No DSP.

pub mod alsa;
pub mod backend;
pub mod file;
pub mod fanout;
pub mod resample;
pub mod sdr;
pub mod stdin;

/// Back-compat path for the SDR backend, which moved from a single `rtlsdr.rs`
/// into the `sdr` module (transport seam + shared DSP pipeline). Kept so existing
/// callers and the `rtl_tcp` integration tests reach it unchanged.
pub use self::sdr as rtlsdr;

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
    /// A matched USB dongle could not be claimed for exclusive use — the kernel
    /// DVB driver is still bound, another process holds it, or the udev rules do
    /// not grant access. Surfaced as `needs_setup` so the UI can prompt the fix.
    #[error("cannot claim USB interface for {0}: {1}")]
    UsbClaim(String, String),
    /// A USB control/bulk transfer failed after the device was claimed.
    #[error("usb transfer error: {0}")]
    Usb(String),
}
