//! CM108/CM119 HID GPIO PTT. Writes a 5-byte HID output report to /dev/hidrawN.
//! Closing a hidraw fd does NOT reset GPIO, so unkey-on-Drop is mandatory.
//! Lifted from Graywolf `tx/ppt_cm108_unix.rs`.

use super::{PttDriver, PttError};

/// Hardware seam: write a raw HID output report.
pub trait Cm108Hid: Send {
    fn write_report(&mut self, report: [u8; 5]) -> Result<(), PttError>;
}

/// CM108 PTT on a 1..=8 GPIO pin.
pub struct Cm108Ptt<H: Cm108Hid> {
    hid: H,
    pin: u8,
    invert: bool,
}

impl<H: Cm108Hid> Cm108Ptt<H> {
    pub fn new(hid: H, pin: u8, invert: bool) -> Result<Self, PttError> {
        if !(1..=8).contains(&pin) {
            return Err(PttError::Config(format!("cm108 pin {pin} out of range 1..=8")));
        }
        let mut d = Cm108Ptt { hid, pin, invert };
        d.unkey()?;
        Ok(d)
    }

    fn set(&mut self, asserted: bool) -> Result<(), PttError> {
        let on = asserted ^ self.invert;
        let mask = 1u8 << (self.pin - 1);
        let value = if on { mask } else { 0 };
        // CM108 HID GPIO report: [0x00, 0x00, value, mask, 0x00].
        self.hid.write_report([0x00, 0x00, value, mask, 0x00])
    }
}

impl<H: Cm108Hid> PttDriver for Cm108Ptt<H> {
    fn key(&mut self) -> Result<(), PttError> {
        self.set(true)
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        self.set(false)
    }
}

impl<H: Cm108Hid> Drop for Cm108Ptt<H> {
    fn drop(&mut self) {
        let _ = self.set(false);
    }
}

/// Real Unix adapter: open /dev/hidrawN and write the report.
#[cfg(unix)]
pub mod unix {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};

    pub struct UnixCm108Hid {
        fd: OwnedFd,
        device: String,
    }

    impl UnixCm108Hid {
        pub fn open(path: &str) -> Result<Self, PttError> {
            use nix::fcntl::{open, OFlag};
            use nix::sys::stat::Mode;
            let raw = open(
                path,
                OFlag::O_RDWR | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| match e {
                nix::errno::Errno::EACCES => PttError::PermissionDenied { device: path.into() },
                nix::errno::Errno::ENOENT => PttError::DeviceGone { device: path.into() },
                o => PttError::Io(format!("open {path}: {o}")),
            })?;
            // SAFETY: open returned a valid fd.
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };
            Ok(UnixCm108Hid { fd, device: path.to_string() })
        }
    }

    impl Cm108Hid for UnixCm108Hid {
        fn write_report(&mut self, report: [u8; 5]) -> Result<(), PttError> {
            match nix::unistd::write(&self.fd, &report) {
                Ok(_) => Ok(()),
                Err(nix::errno::Errno::ENODEV) => {
                    Err(PttError::DeviceGone { device: self.device.clone() })
                }
                Err(e) => Err(PttError::Io(format!("{}: {e}", self.device))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct FakeHid {
        last: Arc<Mutex<Option<[u8; 5]>>>,
    }
    impl Cm108Hid for FakeHid {
        fn write_report(&mut self, report: [u8; 5]) -> Result<(), PttError> {
            *self.last.lock().unwrap() = Some(report);
            Ok(())
        }
    }

    #[test]
    fn rejects_pin_out_of_range() {
        assert!(matches!(
            Cm108Ptt::new(FakeHid::default(), 9, false),
            Err(PttError::Config(_))
        ));
    }

    #[test]
    fn key_sets_value_and_mask_for_pin3() {
        let hid = FakeHid::default();
        let last = hid.last.clone();
        let mut d = Cm108Ptt::new(hid, 3, false).unwrap();
        d.key().unwrap();
        // pin 3 => bit 2 => mask 0b100 = 0x04, value == mask when on.
        assert_eq!(last.lock().unwrap().unwrap(), [0x00, 0x00, 0x04, 0x04, 0x00]);
        d.unkey().unwrap();
        assert_eq!(last.lock().unwrap().unwrap(), [0x00, 0x00, 0x00, 0x04, 0x00]);
    }

    #[test]
    fn drop_writes_an_unkey_report() {
        let hid = FakeHid::default();
        let last = hid.last.clone();
        {
            let mut d = Cm108Ptt::new(hid, 1, false).unwrap();
            d.key().unwrap();
        }
        // Final report after drop is value=0.
        assert_eq!(last.lock().unwrap().unwrap()[2], 0x00);
    }
}
