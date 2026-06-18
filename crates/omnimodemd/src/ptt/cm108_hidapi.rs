//! CM108/CM119 HID GPIO PTT adapter for non-Linux desktop targets (macOS,
//! Windows). Implements the `Cm108Hid` seam via the `hidapi` crate, which
//! wraps IOKit's HID Manager on macOS and HID.dll / SetupAPI on Windows.
//! Lifted from Graywolf `tx/ptt_cm108_macos.rs` (the same `hidapi` path works
//! on both OSes).
//!
//! Linux uses the dedicated `cm108::unix::UnixCm108Hid` adapter (direct
//! /dev/hidrawN write), so this file is gated off Linux/Android.
//!
//! REPORT-ID / WRITE-LENGTH FINDING:
//! `hidapi::HidDevice::write(&[u8])` follows the hidapi C convention: the FIRST
//! byte of the buffer is the HID report ID, and the remaining bytes are the
//! report payload. CM108 GPIO uses report id 0. The seam's 5-byte report is
//! `[0x00, 0x00, value, mask, 0x00]` (see ptt/cm108.rs), where byte 0 (0x00)
//! is already the report-id byte and bytes 1..=4 are the CM108 GPIO payload
//! (HID_OR0..HID_OR3). We therefore write the 5-byte buffer to `write()`
//! verbatim — no extra leading 0x00 is prepended. This matches Graywolf's
//! macOS/Windows CM108 adapters, which write exactly the same 5-byte array.
//!
//! Manual gate: this file only builds on macOS/Windows. Error mapping needs a
//! live HidApi and cannot be unit-tested here; it is verified by manual
//! on-target testing. The `#[cfg(test)]` module only smoke-tests the seam wiring.
#![cfg(all(not(target_os = "linux"), not(target_os = "android")))]

use std::ffi::CString;

use hidapi::{HidApi, HidError};

use crate::ptt::cm108::Cm108Hid;
use crate::ptt::PttError;

/// Real macOS/Windows adapter: open a CM108 HID device by path and write GPIO
/// output reports through hidapi.
pub struct HidApiCm108 {
    // Field order matters for Drop: `device` (hid_close) must drop before
    // `_api` (hid_exit). Rust drops fields in declaration order, so `device`
    // is declared first. Matches Graywolf's MacCm108Gpio.
    device: hidapi::HidDevice,
    _api: HidApi,
    device_path: String,
}

// SAFETY: HidApi holds raw pointers from the C library and HidDevice wraps a
// raw OS HID handle; neither is Send by default. The PTT layer serialises all
// access to a single instance, so concurrent use is impossible by
// construction. Required to satisfy the `Cm108Hid: Send` bound. Matches
// Graywolf's MacCm108Gpio.
unsafe impl Send for HidApiCm108 {}

impl HidApiCm108 {
    pub fn open(path: &str) -> Result<Self, PttError> {
        let api = HidApi::new().map_err(|e| map_hid_err(path, e))?;
        let cpath = CString::new(path)
            .map_err(|_| PttError::Config(format!("invalid cm108 device path: {path}")))?;
        let device = api.open_path(&cpath).map_err(|e| map_hid_err(path, e))?;
        Ok(Self { device, _api: api, device_path: path.to_string() })
    }
}

/// Map a `hidapi::HidError` to a structured `PttError`. hidapi's structured
/// variants don't cleanly separate permission vs. not-found (the underlying C
/// library surfaces a wide-string message), so we key off the IoError kind
/// when present and fall back to substring matching on the message.
fn map_hid_err(device: &str, e: HidError) -> PttError {
    if let HidError::IoError { error } = &e {
        return match error.kind() {
            std::io::ErrorKind::PermissionDenied => {
                PttError::PermissionDenied { device: device.into() }
            }
            std::io::ErrorKind::NotFound => PttError::DeviceGone { device: device.into() },
            _ => PttError::Io(format!("{device}: {e}")),
        };
    }
    let msg = e.to_string().to_ascii_lowercase();
    if msg.contains("permission") || msg.contains("access") || msg.contains("denied") {
        PttError::PermissionDenied { device: device.into() }
    } else if msg.contains("not found")
        || msg.contains("no such")
        || msg.contains("disconnect")
        || msg.contains("unable to open")
    {
        PttError::DeviceGone { device: device.into() }
    } else {
        PttError::Io(format!("{device}: {e}"))
    }
}

impl Cm108Hid for HidApiCm108 {
    fn write_report(&mut self, report: [u8; 5]) -> Result<(), PttError> {
        // `report` already carries the report-id byte at index 0 (see the
        // module-level finding); write it verbatim.
        match self.device.write(&report) {
            Ok(_) => Ok(()),
            Err(e) => Err(map_hid_err(&self.device_path, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    // The hidapi-backed adapter can't be constructed without a live device, so
    // there is no OS-independent unit to exercise here. This smoke test only
    // pins the seam wiring: a fake implementing the same trait round-trips a
    // report, guarding against the trait signature drifting out from under
    // this adapter.
    use crate::ptt::cm108::Cm108Hid;
    use crate::ptt::PttError;

    struct Fake(Option<[u8; 5]>);
    impl Cm108Hid for Fake {
        fn write_report(&mut self, report: [u8; 5]) -> Result<(), PttError> {
            self.0 = Some(report);
            Ok(())
        }
    }

    #[test]
    fn seam_round_trips_a_report() {
        let mut f = Fake(None);
        f.write_report([0x00, 0x00, 0x04, 0x04, 0x00]).unwrap();
        assert_eq!(f.0.unwrap(), [0x00, 0x00, 0x04, 0x04, 0x00]);
    }
}
