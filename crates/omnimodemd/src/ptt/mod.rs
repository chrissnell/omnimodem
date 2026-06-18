//! PTT subsystem: a `PttDriver` trait, structured errors, per-OS drivers
//! behind hardware seams, a `PortRegistry` with DeviceId-keyed hotplug
//! eviction, no-sleep TX sequencing, and the RX/TX interlock. No DSP.

pub mod interlock;
pub mod none;
pub mod registry;
pub mod sequence;
pub mod udev;

#[cfg(target_os = "linux")]
pub mod gpio;
#[cfg(unix)]
pub mod cm108;
#[cfg(unix)]
pub mod serial;

/// Structured PTT failure. Replaces Graywolf's `Result<(), String>` so callers
/// can react: `DeviceGone` triggers registry eviction (Task 12); `PermissionDenied`
/// and `Busy` are terminal config errors; `Unsupported` is a non-Linux driver.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PttError {
    #[error("ptt device {device} disappeared")]
    DeviceGone { device: String },
    #[error("permission denied opening {device}")]
    PermissionDenied { device: String },
    #[error("ptt device {device} is busy")]
    Busy { device: String },
    #[error("invalid ptt config: {0}")]
    Config(String),
    #[error("ptt method unsupported on this platform")]
    Unsupported,
    #[error("ptt i/o error: {0}")]
    Io(String),
}

/// Drives a transmitter's PTT line. `key` asserts (transmit), `unkey` releases.
/// Implementors MUST release PTT on `Drop` (a stuck transmitter is an FCC
/// hazard) — see the per-driver `Drop` impls.
pub trait PttDriver: Send {
    fn key(&mut self) -> Result<(), PttError>;
    fn unkey(&mut self) -> Result<(), PttError>;
}
