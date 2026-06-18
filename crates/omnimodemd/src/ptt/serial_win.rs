//! Windows serial RTS/DTR PTT adapter. Implements the `ModemControlLines`
//! seam by opening a COM port with `CreateFileW` in shared read+write mode
//! (so rigctld / fldigi can still access the device) and toggling RTS/DTR via
//! `EscapeCommFunction(SETRTS/CLRRTS/SETDTR/CLRDTR)`. There is no termios
//! analog on Windows, so no equivalent of the Unix startup-unkey concern about
//! line bouncing during open. Lifted from Graywolf `tx/ptt_win.rs`.
//!
//! Manual gate: this file only builds and runs on Windows. The error mapper
//! cannot be exercised without the Win32 API, so the `#[cfg(test)]` module
//! covers only the OS-independent `dos_device_path` helper. The
//! `CreateFileW` / `EscapeCommFunction` paths are verified by manual on-target
//! testing.
#![cfg(windows)]

use windows::core::HSTRING;
use windows::Win32::Devices::Communication::{
    EscapeCommFunction, CLRDTR, CLRRTS, SETDTR, SETRTS,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
    ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use crate::ptt::serial::ModemControlLines;
use crate::ptt::PttError;

/// Real Windows adapter: holds an open COM-port handle and drives its modem
/// control lines.
pub struct WinSerialLines {
    handle: HANDLE,
    device: String,
}

// SAFETY: a Windows HANDLE is not marked Send by the `windows` crate because
// raw handles are process-wide and the crate can't know whether a given one is
// shared. The PTT layer serialises all access to a single WinSerialLines
// instance, so concurrent use is impossible by construction. Matches the
// `ModemControlLines: Send` bound and Graywolf's WinSerialLines.
unsafe impl Send for WinSerialLines {}

impl WinSerialLines {
    pub fn open(path: &str) -> Result<Self, PttError> {
        // CreateFileW rejects bare "COM10".."COM999" with ERROR_FILE_NOT_FOUND;
        // only the "\\.\COMn" DOS-device form resolves them. Low-numbered ports
        // work either way, so we always prepend the prefix for consistency.
        // Paths already in DOS-device or extended-length form pass through.
        let dos = dos_device_path(path);
        let wide: HSTRING = dos.as_str().into();
        // SAFETY: `wide` is a valid NUL-terminated UTF-16 buffer that outlives
        // the call; all other pointer arguments are default.
        let handle = unsafe {
            CreateFileW(
                &wide,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|e| map_win_err(path, e))?;
        Ok(Self { handle, device: path.to_string() })
    }
}

/// Normalise a port name to the DOS-device form CreateFileW needs.
fn dos_device_path(path: &str) -> String {
    if path.starts_with(r"\\.\") || path.starts_with(r"\\?\") {
        path.to_string()
    } else {
        format!(r"\\.\{}", path)
    }
}

/// Map a `windows::core::Error` to a structured `PttError` by Win32 code.
fn map_win_err(device: &str, e: windows::core::Error) -> PttError {
    let code = e.code();
    if code == ERROR_ACCESS_DENIED.to_hresult() {
        PttError::PermissionDenied { device: device.into() }
    } else if code == ERROR_SHARING_VIOLATION.to_hresult() {
        PttError::Busy { device: device.into() }
    } else if code == ERROR_FILE_NOT_FOUND.to_hresult()
        || code == ERROR_PATH_NOT_FOUND.to_hresult()
    {
        PttError::DeviceGone { device: device.into() }
    } else {
        PttError::Io(format!("{device}: {e}"))
    }
}

impl ModemControlLines for WinSerialLines {
    fn write_rts(&mut self, high: bool) -> Result<(), PttError> {
        let code = if high { SETRTS } else { CLRRTS };
        // SAFETY: handle is valid for the lifetime of &mut self.
        unsafe { EscapeCommFunction(self.handle, code) }
            .map_err(|e| map_win_err(&self.device, e))
    }

    fn write_dtr(&mut self, high: bool) -> Result<(), PttError> {
        let code = if high { SETDTR } else { CLRDTR };
        // SAFETY: same as write_rts.
        unsafe { EscapeCommFunction(self.handle, code) }
            .map_err(|e| map_win_err(&self.device, e))
    }
}

impl Drop for WinSerialLines {
    fn drop(&mut self) {
        // SAFETY: handle was obtained from CreateFileW and hasn't been closed.
        // Ignore the return — we can't recover from a close failure in Drop.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::dos_device_path;

    #[test]
    fn bare_com_port_gets_dos_device_prefix() {
        assert_eq!(dos_device_path("COM12"), r"\\.\COM12");
        assert_eq!(dos_device_path("COM3"), r"\\.\COM3");
    }

    #[test]
    fn already_prefixed_paths_are_passed_through() {
        assert_eq!(dos_device_path(r"\\.\COM12"), r"\\.\COM12");
        assert_eq!(dos_device_path(r"\\?\COM12"), r"\\?\COM12");
    }
}
