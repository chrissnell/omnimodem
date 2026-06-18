//! PortRegistry: build PTT drivers from config and cache them by DeviceId, with
//! eviction on hotplug/disappearance — the gap Graywolf's path-keyed, never-
//! evicted serial cache left open.

use super::none::NonePtt;
use super::{PttDriver, PttError};
use crate::ids::DeviceId;
use std::collections::HashMap;

/// How a channel's PTT is wired. The `device_id` is the durable key config is
/// stored under; `node` is the resolved live path (e.g. /dev/ttyUSB0) supplied
/// by the device cache at build time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PttConfig {
    pub device_id: DeviceId,
    pub method: PttMethod,
    pub invert: bool,
}

/// Supported PTT methods. Non-Linux construction fails closed (`Unsupported`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PttMethod {
    None,
    Vox,
    SerialRts { node: String },
    SerialDtr { node: String },
    Cm108 { node: String, pin: u8 },
    Gpio { chip: String, line: u32 },
    /// Hamlib `rigctld` over TCP. `addr` is `host:port`. Portable (every OS).
    Rigctld { addr: String },
    /// Android: PTT actuated by the Kotlin USB layer; `method` is one of the
    /// `ptt::android` method-int consts (CP2102N_RTS, CM108_HID, …). Builds a
    /// driver only on Android (or the host test stub); elsewhere `Unsupported`.
    Android { method: i32 },
}

/// Opens a real driver for a config. Injectable so tests substitute MockPtt.
pub trait DriverOpener: Send {
    fn open(&self, cfg: &PttConfig) -> Result<Box<dyn PttDriver>, PttError>;
}

/// Caches one driver per DeviceId; evicts on hotplug.
pub struct PortRegistry {
    opener: Box<dyn DriverOpener>,
    cache: HashMap<DeviceId, ()>, // identity presence; drivers are owned by callers
}

impl PortRegistry {
    pub fn new(opener: Box<dyn DriverOpener>) -> Self {
        PortRegistry { opener, cache: HashMap::new() }
    }

    /// Build a driver, recording its DeviceId as live.
    pub fn build_driver(&mut self, cfg: &PttConfig) -> Result<Box<dyn PttDriver>, PttError> {
        let driver = self.opener.open(cfg)?;
        self.cache.insert(cfg.device_id.clone(), ());
        Ok(driver)
    }

    /// A device disappeared (hotplug `Departed`): forget it so the next
    /// build_driver re-opens from scratch rather than reusing a dead handle.
    pub fn evict(&mut self, id: &DeviceId) {
        self.cache.remove(id);
    }

    pub fn knows(&self, id: &DeviceId) -> bool {
        self.cache.contains_key(id)
    }
}

/// The production opener building the real Linux drivers from Task 11.
pub struct RealOpener;

impl DriverOpener for RealOpener {
    fn open(&self, cfg: &PttConfig) -> Result<Box<dyn PttDriver>, PttError> {
        match &cfg.method {
            PttMethod::None | PttMethod::Vox => Ok(Box::new(NonePtt)),

            // Portable, every OS: Hamlib rigctld over TCP.
            PttMethod::Rigctld { addr } => {
                use super::rigctld::RigctldPtt;
                Ok(Box::new(RigctldPtt::connect(addr)?))
            }

            // Serial RTS/DTR: unix drives TIOCMSET; Windows uses EscapeCommFunction.
            #[cfg(unix)]
            PttMethod::SerialRts { node } => {
                use super::serial::{unix::UnixSerialLines, SerialLine, SerialLinePtt};
                let lines = UnixSerialLines::open(node)?;
                Ok(Box::new(SerialLinePtt::new(lines, SerialLine::Rts, cfg.invert, node.clone())?))
            }
            #[cfg(unix)]
            PttMethod::SerialDtr { node } => {
                use super::serial::{unix::UnixSerialLines, SerialLine, SerialLinePtt};
                let lines = UnixSerialLines::open(node)?;
                Ok(Box::new(SerialLinePtt::new(lines, SerialLine::Dtr, cfg.invert, node.clone())?))
            }
            #[cfg(windows)]
            PttMethod::SerialRts { node } => {
                use super::serial::{SerialLine, SerialLinePtt};
                use super::serial_win::WinSerialLines;
                let lines = WinSerialLines::open(node)?;
                Ok(Box::new(SerialLinePtt::new(lines, SerialLine::Rts, cfg.invert, node.clone())?))
            }
            #[cfg(windows)]
            PttMethod::SerialDtr { node } => {
                use super::serial::{SerialLine, SerialLinePtt};
                use super::serial_win::WinSerialLines;
                let lines = WinSerialLines::open(node)?;
                Ok(Box::new(SerialLinePtt::new(lines, SerialLine::Dtr, cfg.invert, node.clone())?))
            }

            // CM108 HID: Linux writes /dev/hidrawN directly; macOS+Windows via hidapi.
            #[cfg(target_os = "linux")]
            PttMethod::Cm108 { node, pin } => {
                use super::cm108::{unix::UnixCm108Hid, Cm108Ptt};
                let hid = UnixCm108Hid::open(node)?;
                Ok(Box::new(Cm108Ptt::new(hid, *pin, cfg.invert)?))
            }
            #[cfg(all(not(target_os = "linux"), not(target_os = "android")))]
            PttMethod::Cm108 { node, pin } => {
                use super::cm108::Cm108Ptt;
                use super::cm108_hidapi::HidApiCm108;
                let hid = HidApiCm108::open(node)?;
                Ok(Box::new(Cm108Ptt::new(hid, *pin, cfg.invert)?))
            }

            #[cfg(target_os = "linux")]
            PttMethod::Gpio { chip, line } => {
                use super::gpio::{linux::LinuxGpiochip, GpioPtt};
                let gl = LinuxGpiochip::open(chip, *line)?;
                Ok(Box::new(GpioPtt::new(gl, cfg.invert)?))
            }

            // Android: actuated by the Kotlin USB layer over JNI.
            #[cfg(any(target_os = "android", feature = "android-test-stub"))]
            PttMethod::Android { method } => {
                use super::android::AndroidPtt;
                Ok(Box::new(AndroidPtt::new(*method)))
            }

            #[allow(unreachable_patterns)]
            _ => Err(PttError::Unsupported),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptt::none::MockPtt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountingOpener {
        opens: Arc<AtomicUsize>,
    }
    impl DriverOpener for CountingOpener {
        fn open(&self, _cfg: &PttConfig) -> Result<Box<dyn PttDriver>, PttError> {
            self.opens.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(MockPtt::new()))
        }
    }

    fn cfg(tag: &str) -> PttConfig {
        PttConfig {
            device_id: DeviceId::Serial { by_id: tag.into() },
            method: PttMethod::None,
            invert: false,
        }
    }

    #[test]
    fn build_records_identity_as_live() {
        let opens = Arc::new(AtomicUsize::new(0));
        let mut reg = PortRegistry::new(Box::new(CountingOpener { opens: opens.clone() }));
        let c = cfg("ftdi-A");
        let _d = reg.build_driver(&c).unwrap();
        assert!(reg.knows(&c.device_id));
    }

    #[test]
    fn eviction_forgets_the_identity() {
        let opens = Arc::new(AtomicUsize::new(0));
        let mut reg = PortRegistry::new(Box::new(CountingOpener { opens }));
        let c = cfg("ftdi-A");
        let _d = reg.build_driver(&c).unwrap();
        reg.evict(&c.device_id);
        assert!(!reg.knows(&c.device_id), "evicted identity must be forgotten");
    }

    #[test]
    fn rebuild_after_eviction_reopens() {
        let opens = Arc::new(AtomicUsize::new(0));
        let mut reg = PortRegistry::new(Box::new(CountingOpener { opens: opens.clone() }));
        let c = cfg("ftdi-A");
        let _d1 = reg.build_driver(&c).unwrap();
        reg.evict(&c.device_id);
        let _d2 = reg.build_driver(&c).unwrap();
        assert_eq!(opens.load(Ordering::Relaxed), 2, "reopen after hotplug");
    }
}
