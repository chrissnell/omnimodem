//! Serial RTS/DTR PTT. The most common cheap interface. Logic (which line,
//! polarity, structured errors) sits above a `ModemControlLines` seam; the
//! Unix adapter drives `TIOCMSET` via nix. Lifted from Graywolf
//! `tx/ppt_unix.rs` (`UnixSerialLines`).

use super::{PttDriver, PttError};

/// Which control line keys the radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialLine {
    Rts,
    Dtr,
}

/// Hardware seam: set a control line high/low. The real impl issues ioctls; the
/// test impl records calls.
pub trait ModemControlLines: Send {
    fn write_rts(&mut self, high: bool) -> Result<(), PttError>;
    fn write_dtr(&mut self, high: bool) -> Result<(), PttError>;
}

/// Serial PTT driver over any `ModemControlLines`.
pub struct SerialLinePtt<L: ModemControlLines> {
    lines: L,
    line: SerialLine,
    invert: bool,
    #[allow(dead_code)]
    device: String,
}

impl<L: ModemControlLines> SerialLinePtt<L> {
    /// Build and immediately unkey: on Linux the TTY layer asserts DTR during
    /// open() regardless of intent, so an explicit deassert here narrows the
    /// spurious-TX window to microseconds (Graywolf startup-unkey).
    pub fn new(lines: L, line: SerialLine, invert: bool, device: String) -> Result<Self, PttError> {
        let mut d = SerialLinePtt { lines, line, invert, device };
        d.unkey()?;
        Ok(d)
    }

    fn set(&mut self, asserted: bool) -> Result<(), PttError> {
        let level = asserted ^ self.invert;
        match self.line {
            SerialLine::Rts => self.lines.write_rts(level),
            SerialLine::Dtr => self.lines.write_dtr(level),
        }
    }
}

impl<L: ModemControlLines> PttDriver for SerialLinePtt<L> {
    fn key(&mut self) -> Result<(), PttError> {
        self.set(true)
    }
    fn unkey(&mut self) -> Result<(), PttError> {
        self.set(false)
    }
}

impl<L: ModemControlLines> Drop for SerialLinePtt<L> {
    fn drop(&mut self) {
        let _ = self.set(false); // never leave a rig keyed
    }
}

/// The real Unix adapter: open the tty and drive TIOCMSET.
#[cfg(unix)]
pub mod unix {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    pub struct UnixSerialLines {
        fd: OwnedFd,
        device: String,
    }

    impl UnixSerialLines {
        pub fn open(path: &str) -> Result<Self, PttError> {
            use nix::fcntl::{open, OFlag};
            use nix::sys::stat::Mode;
            let raw = open(
                path,
                OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| map_open_err(path, e))?;
            // SAFETY: open returned a valid owned fd.
            let owned = unsafe { OwnedFd::from_raw_fd(raw) };
            Ok(UnixSerialLines { fd: owned, device: path.to_string() })
        }

        fn set_bit(&mut self, bit: i32, high: bool) -> Result<(), PttError> {
            // Read-modify-write the modem control bits via TIOCMGET/TIOCMSET.
            let mut status: i32 = 0;
            // SAFETY: ioctl with valid fd + int pointer.
            let r = unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCMGET, &mut status) };
            if r != 0 {
                return Err(self.io_or_gone());
            }
            if high {
                status |= bit;
            } else {
                status &= !bit;
            }
            let r = unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCMSET, &status) };
            if r != 0 {
                return Err(self.io_or_gone());
            }
            Ok(())
        }

        fn io_or_gone(&self) -> PttError {
            // ENODEV/ENXIO after a working open means the adapter was unplugged.
            match nix::errno::Errno::last() {
                nix::errno::Errno::ENODEV | nix::errno::Errno::ENXIO => {
                    PttError::DeviceGone { device: self.device.clone() }
                }
                e => PttError::Io(format!("{}: {e}", self.device)),
            }
        }
    }

    impl ModemControlLines for UnixSerialLines {
        fn write_rts(&mut self, high: bool) -> Result<(), PttError> {
            self.set_bit(libc::TIOCM_RTS, high)
        }
        fn write_dtr(&mut self, high: bool) -> Result<(), PttError> {
            self.set_bit(libc::TIOCM_DTR, high)
        }
    }

    fn map_open_err(path: &str, e: nix::errno::Errno) -> PttError {
        match e {
            nix::errno::Errno::EACCES => PttError::PermissionDenied { device: path.into() },
            nix::errno::Errno::EBUSY => PttError::Busy { device: path.into() },
            nix::errno::Errno::ENOENT | nix::errno::Errno::ENODEV | nix::errno::Errno::ENXIO => {
                PttError::DeviceGone { device: path.into() }
            }
            other => PttError::Io(format!("open {path}: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct FakeLines {
        rts: Arc<Mutex<bool>>,
        dtr: Arc<Mutex<bool>>,
    }
    impl ModemControlLines for FakeLines {
        fn write_rts(&mut self, high: bool) -> Result<(), PttError> {
            *self.rts.lock().unwrap() = high;
            Ok(())
        }
        fn write_dtr(&mut self, high: bool) -> Result<(), PttError> {
            *self.dtr.lock().unwrap() = high;
            Ok(())
        }
    }

    #[test]
    fn key_asserts_the_selected_line() {
        let lines = FakeLines::default();
        let rts = lines.rts.clone();
        let mut d = SerialLinePtt::new(lines, SerialLine::Rts, false, "/dev/ttyUSB0".into()).unwrap();
        d.key().unwrap();
        assert!(*rts.lock().unwrap());
        d.unkey().unwrap();
        assert!(!*rts.lock().unwrap());
    }

    #[test]
    fn invert_flips_polarity() {
        let lines = FakeLines::default();
        let dtr = lines.dtr.clone();
        let mut d = SerialLinePtt::new(lines, SerialLine::Dtr, true, "/dev/ttyUSB0".into()).unwrap();
        // new() unkeyed: inverted unkey => line HIGH.
        assert!(*dtr.lock().unwrap());
        d.key().unwrap();
        assert!(!*dtr.lock().unwrap()); // inverted key => LOW
    }

    #[test]
    fn drop_releases_the_line() {
        let lines = FakeLines::default();
        let rts = lines.rts.clone();
        {
            let mut d = SerialLinePtt::new(lines, SerialLine::Rts, false, "x".into()).unwrap();
            d.key().unwrap();
            assert!(*rts.lock().unwrap());
        }
        assert!(!*rts.lock().unwrap(), "drop must deassert");
    }
}
