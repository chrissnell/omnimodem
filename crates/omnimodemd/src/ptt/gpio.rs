//! Linux gpiochip v2 PTT via gpiocdev. A `LineGone` on set means the chip was
//! unplugged -> map to `PttError::DeviceGone` so the registry evicts. Lifted
//! from Graywolf `tx/ppt_gpio_linux.rs` + its `GpioError`.

#![cfg(target_os = "linux")]

use super::{PttDriver, PttError};

/// Hardware seam: drive one gpiochip line active/inactive.
pub trait GpiochipLine: Send {
    fn set_active(&mut self, active: bool) -> Result<(), PttError>;
}

pub struct GpioPtt<L: GpiochipLine> {
    line: L,
    invert: bool,
}

impl<L: GpiochipLine> GpioPtt<L> {
    pub fn new(line: L, invert: bool) -> Result<Self, PttError> {
        let mut d = GpioPtt { line, invert };
        d.unkey()?;
        Ok(d)
    }
    fn set(&mut self, asserted: bool) -> Result<(), PttError> {
        self.line.set_active(asserted ^ self.invert)
    }
}

impl<L: GpiochipLine> PttDriver for GpioPtt<L> {
    fn key(&mut self) -> Result<(), PttError> {
        self.set(true)
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        self.set(false)
    }
}

impl<L: GpiochipLine> Drop for GpioPtt<L> {
    fn drop(&mut self) {
        let _ = self.set(false);
    }
}

/// Real adapter over gpiocdev.
pub mod linux {
    use super::*;
    use gpiocdev::line::Value;
    use gpiocdev::request::Request;

    pub struct LinuxGpiochip {
        request: Request,
        offset: u32,
        chip: String,
    }

    impl LinuxGpiochip {
        pub fn open(chip_path: &str, offset: u32) -> Result<Self, PttError> {
            let request = Request::builder()
                .on_chip(chip_path)
                .with_line(offset)
                .with_consumer("omnimodem-ptt")
                .as_output(Value::Inactive)
                .request()
                .map_err(|e| map_gpio_err(chip_path, offset, e))?;
            Ok(LinuxGpiochip { request, offset, chip: chip_path.to_string() })
        }
    }

    impl GpiochipLine for LinuxGpiochip {
        fn set_active(&mut self, active: bool) -> Result<(), PttError> {
            let v = if active { Value::Active } else { Value::Inactive };
            self.request
                .set_value(self.offset, v)
                .map_err(|e| map_gpio_err(&self.chip, self.offset, e))
        }
    }

    fn map_gpio_err(chip: &str, _line: u32, e: gpiocdev::Error) -> PttError {
        let s = e.to_string().to_lowercase();
        if s.contains("permission") {
            PttError::PermissionDenied { device: chip.into() }
        } else if s.contains("busy") {
            PttError::Busy { device: chip.into() }
        } else if s.contains("no such") || s.contains("nodev") || s.contains("gone") {
            PttError::DeviceGone { device: chip.into() }
        } else {
            PttError::Io(format!("{chip}: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct FakeLine {
        active: Arc<Mutex<bool>>,
        gone: Arc<Mutex<bool>>,
    }
    impl GpiochipLine for FakeLine {
        fn set_active(&mut self, active: bool) -> Result<(), PttError> {
            if *self.gone.lock().unwrap() {
                return Err(PttError::DeviceGone { device: "gpiochip0".into() });
            }
            *self.active.lock().unwrap() = active;
            Ok(())
        }
    }

    #[test]
    fn key_unkey_drives_the_line() {
        let line = FakeLine::default();
        let active = line.active.clone();
        let mut d = GpioPtt::new(line, false).unwrap();
        d.key().unwrap();
        assert!(*active.lock().unwrap());
        d.unkey().unwrap();
        assert!(!*active.lock().unwrap());
    }

    #[test]
    fn line_gone_surfaces_device_gone() {
        let line = FakeLine::default();
        let gone = line.gone.clone();
        let mut d = GpioPtt::new(line, false).unwrap();
        *gone.lock().unwrap() = true;
        assert!(matches!(d.key(), Err(PttError::DeviceGone { .. })));
    }
}
