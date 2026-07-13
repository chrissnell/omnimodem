//! Native (local USB) RTL-SDR transport: open + claim a dongle over `nusb` and
//! talk to the RTL2832U with vendor control transfers.
//!
//! This module owns the USB reality the shared [`pipeline`](super::pipeline) never
//! sees: matching a [`RtlKey`] to a present device, claiming interface 0 (detaching
//! the kernel DVB driver on Linux first), and the `read_reg`/`write_reg` control
//! primitives every later stage (baseband init, tuner, streaming) is built from.
//!
//! Register I/O runs over a small [`UsbControl`] seam so the exact SETUP packets are
//! unit-tested against a `FakeUsb` with no hardware. The register semantics —
//! request codes, the `(block << 8) | 0x10` write index, the little-endian read
//! assembly — are transcribed from librtlsdr (`rtlsdr_read_reg` / `rtlsdr_write_reg`),
//! the byte-level source of truth.

use super::AudioError;
use crate::ids::{DeviceId, RtlKey};
use nusb::transfer::{Control, ControlType, Recipient};
use std::time::Duration;

/// `bmRequestType` for an RTL2832U register **read** (`LIBUSB_ENDPOINT_IN |
/// LIBUSB_REQUEST_TYPE_VENDOR`, recipient device). librtlsdr `CTRL_IN`.
const CTRL_IN: u8 = 0xC0;
/// `bmRequestType` for an RTL2832U register **write** (`LIBUSB_ENDPOINT_OUT |
/// LIBUSB_REQUEST_TYPE_VENDOR`, recipient device). librtlsdr `CTRL_OUT`.
const CTRL_OUT: u8 = 0x40;
/// Per-transfer control timeout. librtlsdr `CTRL_TIMEOUT` (300 ms).
const CTRL_TIMEOUT: Duration = Duration::from_millis(300);
/// The RTL2832U exposes its vendor functions on interface 0.
const RTL_INTERFACE: u8 = 0;

/// Realtek RTL2832U register blocks, selected in the high byte of the control
/// `wIndex`. Transcribed from librtlsdr's block enum; only some are used before
/// the tuner and streaming stages land.
#[allow(dead_code)] // Rom/Ir/Iic are addressed by later bring-up phases (P2-B..E).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Block {
    /// Demodulator core.
    Demod = 0,
    /// USB controller block.
    Usb = 1,
    /// System block (GPIO, resets, DEMOD_CTL).
    Sys = 2,
    /// Tuner pass-through (I2C to the R82xx et al.).
    Tuner = 3,
    /// EEPROM.
    Rom = 4,
    /// IR receiver block.
    Ir = 5,
    /// Raw I2C bus.
    Iic = 6,
}

/// A vendor control-transfer SETUP packet, minus the data stage. Kept as raw wire
/// fields so tests can assert the exact bytes librtlsdr would put on the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Setup {
    /// `bmRequestType`.
    pub request_type: u8,
    /// `bRequest`.
    pub request: u8,
    /// `wValue`.
    pub value: u16,
    /// `wIndex`.
    pub index: u16,
}

/// The single-transfer seam register I/O is written against, so `read_reg` /
/// `write_reg` are exercised with a fake in unit tests. [`NusbControl`] is the real
/// implementation over a claimed interface.
pub(crate) trait UsbControl {
    /// Vendor control **OUT**: send `data` with the given SETUP.
    fn control_out(&self, setup: Setup, data: &[u8]) -> Result<(), AudioError>;
    /// Vendor control **IN**: read into `buf`, returning the byte count.
    fn control_in(&self, setup: Setup, buf: &mut [u8]) -> Result<usize, AudioError>;
}

/// `UsbControl` over a claimed `nusb::Interface`. The RTL2832U's register requests
/// are all vendor / device-recipient, so the direction alone distinguishes them;
/// nusb derives `bmRequestType` from the typed fields (matching [`CTRL_IN`] /
/// [`CTRL_OUT`], asserted in debug builds).
struct NusbControl {
    iface: nusb::Interface,
}

impl UsbControl for NusbControl {
    fn control_out(&self, setup: Setup, data: &[u8]) -> Result<(), AudioError> {
        debug_assert_eq!(setup.request_type, CTRL_OUT);
        let control = Control {
            control_type: ControlType::Vendor,
            recipient: Recipient::Device,
            request: setup.request,
            value: setup.value,
            index: setup.index,
        };
        self.iface
            .control_out_blocking(control, data, CTRL_TIMEOUT)
            .map(|_| ())
            .map_err(|e| AudioError::Usb(format!("control out (index {:#06x}): {e}", setup.index)))
    }

    fn control_in(&self, setup: Setup, buf: &mut [u8]) -> Result<usize, AudioError> {
        debug_assert_eq!(setup.request_type, CTRL_IN);
        let control = Control {
            control_type: ControlType::Vendor,
            recipient: Recipient::Device,
            request: setup.request,
            value: setup.value,
            index: setup.index,
        };
        self.iface
            .control_in_blocking(control, buf, CTRL_TIMEOUT)
            .map_err(|e| AudioError::Usb(format!("control in (index {:#06x}): {e}", setup.index)))
    }
}

/// Write a `len`-byte (1 or 2) RTL2832U register. Big-endian on the wire for a
/// 2-byte value, low byte only for a 1-byte value; the write index carries the
/// block in the high byte with the `0x10` write flag. Transcribed from librtlsdr
/// `rtlsdr_write_reg`.
#[allow(dead_code)] // Register writes are wired into baseband/tuner init in P2-B/P2-C.
pub(crate) fn write_reg(
    usb: &impl UsbControl,
    block: Block,
    addr: u16,
    val: u16,
    len: u8,
) -> Result<(), AudioError> {
    let mut data = [0u8; 2];
    if len == 1 {
        data[0] = (val & 0xff) as u8;
    } else {
        data[0] = (val >> 8) as u8;
    }
    data[1] = (val & 0xff) as u8;

    let setup = Setup {
        request_type: CTRL_OUT,
        request: 0,
        value: addr,
        index: ((block as u16) << 8) | 0x10,
    };
    usb.control_out(setup, &data[..len as usize])
}

/// Read a `len`-byte (1 or 2) RTL2832U register. The block sits in the high byte of
/// the read index (no write flag); the returned value is assembled little-endian
/// from the data stage. Transcribed from librtlsdr `rtlsdr_read_reg`.
#[allow(dead_code)] // Register reads are wired into tuner probe/init in P2-B/P2-C.
pub(crate) fn read_reg(
    usb: &impl UsbControl,
    block: Block,
    addr: u16,
    len: u8,
) -> Result<u16, AudioError> {
    let mut data = [0u8; 2];
    let setup = Setup {
        request_type: CTRL_IN,
        request: 0,
        value: addr,
        index: (block as u16) << 8,
    };
    usb.control_in(setup, &mut data[..len as usize])?;
    Ok(((data[1] as u16) << 8) | data[0] as u16)
}

/// A locally-attached RTL-SDR dongle, opened and claimed for exclusive use.
///
/// P2-A establishes the USB seam only: identity match, claim (with Linux
/// kernel-driver detach), and the register primitives. Baseband init, tuner
/// probe/tune, streaming, and the [`SdrTransport`](super::SdrTransport) impl arrive
/// in the following phases.
pub struct RtlUsbTransport {
    usb: NusbControl,
    /// The claimed interface number, kept for endpoint addressing in P2-E streaming.
    #[allow(dead_code)]
    iface: u8,
}

impl RtlUsbTransport {
    /// List USB devices, match `key` to a present RTL dongle, open it, and claim
    /// interface 0. On Linux the kernel DVB driver (`dvb_usb_rtl28xxu`) is detached
    /// as part of the claim; macOS/Windows claim directly. A claim that fails
    /// (driver still bound, no permission) maps to [`AudioError::UsbClaim`] so the
    /// caller can surface `needs_setup`.
    pub fn open(key: &RtlKey) -> Result<Self, AudioError> {
        let id = DeviceId::Rtl { key: key.clone() }.to_canonical_string();

        let info = nusb::list_devices()
            .map_err(|e| AudioError::Usb(format!("enumerate usb: {e}")))?
            .find(|d| is_rtl_dongle(d.vendor_id(), d.product_id()) && key_matches(d, key))
            .ok_or_else(|| AudioError::DeviceNotFound(id.clone()))?;

        let dev = info
            .open()
            .map_err(|e| AudioError::UsbClaim(id.clone(), format!("open device: {e}")))?;
        let iface = claim_interface(&dev, RTL_INTERFACE, &id)?;

        Ok(Self { usb: NusbControl { iface }, iface: RTL_INTERFACE })
    }

    /// Write an RTL2832U register (see [`write_reg`]).
    #[allow(dead_code)] // Consumed by baseband/tuner init in P2-B/P2-C.
    pub(crate) fn write_reg(&self, block: Block, addr: u16, val: u16, len: u8) -> Result<(), AudioError> {
        write_reg(&self.usb, block, addr, val, len)
    }

    /// Read an RTL2832U register (see [`read_reg`]).
    #[allow(dead_code)] // Consumed by tuner probe/init in P2-B/P2-C.
    pub(crate) fn read_reg(&self, block: Block, addr: u16, len: u8) -> Result<u16, AudioError> {
        read_reg(&self.usb, block, addr, len)
    }
}

/// Claim interface 0, detaching the kernel driver first on Linux.
#[cfg(target_os = "linux")]
fn claim_interface(dev: &nusb::Device, iface: u8, id: &str) -> Result<nusb::Interface, AudioError> {
    dev.detach_and_claim_interface(iface).map_err(|e| {
        AudioError::UsbClaim(id.to_string(), format!("detach + claim interface {iface}: {e}"))
    })
}

/// Claim interface 0 directly — macOS/Windows have no in-kernel DVB driver to
/// detach (the OS binds a generic/WinUSB driver instead).
#[cfg(not(target_os = "linux"))]
fn claim_interface(dev: &nusb::Device, iface: u8, id: &str) -> Result<nusb::Interface, AudioError> {
    dev.claim_interface(iface)
        .map_err(|e| AudioError::UsbClaim(id.to_string(), format!("claim interface {iface}: {e}")))
}

/// Does a present device match the requested [`RtlKey`]? Serial keys compare the
/// USB serial string; topology keys compare the bus plus the port chain (Linux
/// sysfs). A topology key never matches on a platform without a portable port
/// chain — cross-platform topology lands with the discovery work (P1-C / P3-B).
fn key_matches(info: &nusb::DeviceInfo, key: &RtlKey) -> bool {
    match key {
        RtlKey::Serial(want) => info.serial_number() == Some(want.as_str()),
        RtlKey::Topo { bus, ports } => {
            info.bus_number() == *bus && device_port_chain(info).as_deref() == Some(ports.as_str())
        }
    }
}

/// The USB port chain (e.g. `4.2`) for a device, from the Linux sysfs node name
/// `<bus>-<ports>`. `None` where no portable source exists.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn device_port_chain(info: &nusb::DeviceInfo) -> Option<String> {
    let name = info.sysfs_path().file_name()?.to_str()?;
    parse_sysfs_port_chain(name, info.bus_number())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn device_port_chain(_info: &nusb::DeviceInfo) -> Option<String> {
    None
}

/// Split a `<bus>-<ports>` sysfs USB device name into its port chain, requiring the
/// bus prefix to match. Root hubs (`usb1`, no `-`) and mismatched buses yield
/// `None`. Pure so the topology match is testable without hardware.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
fn parse_sysfs_port_chain(name: &str, bus: u8) -> Option<String> {
    let (bus_str, ports) = name.split_once('-')?;
    if bus_str.parse::<u8>().ok()? != bus || ports.is_empty() {
        return None;
    }
    Some(ports.to_string())
}

/// Realtek RTL2832U dongle USB IDs — librtlsdr's `known_devices` table. Matching
/// only these avoids opening unrelated devices during discovery.
const KNOWN_RTL: &[(u16, u16)] = &[
    (0x0bda, 0x2832), // Generic RTL2832U
    (0x0bda, 0x2838), // Generic RTL2832U OEM
    (0x0413, 0x6680), // DigitalNow Quad DVB-T PCI-E card
    (0x0413, 0x6f0f), // Leadtek WinFast DTV Dongle mini D
    (0x0458, 0x707f), // Genius TVGo DVB-T03 USB dongle (Ver. B)
    (0x0ccd, 0x00a9), // Terratec Cinergy T Stick Black (rev 1)
    (0x0ccd, 0x00b3), // Terratec NOXON DAB/DAB+ USB dongle (rev 1)
    (0x0ccd, 0x00b4), // Terratec Deutschlandradio DAB Stick
    (0x0ccd, 0x00b5), // Terratec NOXON DAB Stick - Radio Energy
    (0x0ccd, 0x00b7), // Terratec Media Broadcast DAB Stick
    (0x0ccd, 0x00b8), // Terratec BR DAB Stick
    (0x0ccd, 0x00b9), // Terratec WDR DAB Stick
    (0x0ccd, 0x00c0), // Terratec MuellerVerlag DAB Stick
    (0x0ccd, 0x00c6), // Terratec Fraunhofer DAB Stick
    (0x0ccd, 0x00d3), // Terratec Cinergy T Stick RC (Rev.3)
    (0x0ccd, 0x00d7), // Terratec T Stick PLUS
    (0x0ccd, 0x00e0), // Terratec NOXON DAB/DAB+ USB dongle (rev 2)
    (0x1209, 0x2832), // Generic RTL2832U
    (0x1554, 0x5020), // PixelView PV-DT235U(RN)
    (0x15f4, 0x0131), // Astrometa DVB-T/DVB-T2
    (0x15f4, 0x0133), // HanfTek DAB+FM+DVB-T
    (0x185b, 0x0620), // Compro Videomate U620F
    (0x185b, 0x0650), // Compro Videomate U650F
    (0x185b, 0x0680), // Compro Videomate U680F
    (0x1b80, 0xd393), // GIGABYTE GT-U7300
    (0x1b80, 0xd394), // DIKOM USB-DVBT HD
    (0x1b80, 0xd395), // Peak 102569AGPK
    (0x1b80, 0xd397), // KWorld KW-UB450-T USB DVB-T Pico TV
    (0x1b80, 0xd398), // Zaapa ZT-MINDVBZP
    (0x1b80, 0xd39d), // SVEON STV20 DVB-T USB & FM
    (0x1b80, 0xd3a4), // Twintech UT-40
    (0x1b80, 0xd3a8), // ASUS U3100MINI_PLUS_V2
    (0x1b80, 0xd3af), // SVEON STV27 DVB-T USB & FM
    (0x1b80, 0xd3b0), // SVEON STV21 DVB-T USB & FM
    (0x1d19, 0x1101), // Dexatek DK DVB-T Dongle (Logilink VG0002A)
    (0x1d19, 0x1102), // Dexatek DK DVB-T Dongle (MSI DigiVox mini II V3.0)
    (0x1d19, 0x1103), // Dexatek Technology Ltd. DK 5217 DVB-T Dongle
    (0x1d19, 0x1104), // MSI DigiVox Micro HD
    (0x1f4d, 0xa803), // Sweex DVB-T USB
    (0x1f4d, 0xb803), // GTek T803
    (0x1f4d, 0xc803), // Lifeview LV5TDeluxe
    (0x1f4d, 0xd286), // MyGica TD312
    (0x1f4d, 0xd803), // PROlectrix DV107669
];

/// Whether a `(vid, pid)` is a known RTL2832U dongle.
fn is_rtl_dongle(vid: u16, pid: u16) -> bool {
    KNOWN_RTL.contains(&(vid, pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records every OUT SETUP+payload and serves canned IN payloads, so a test can
    /// assert the exact control transfers register I/O emits.
    #[derive(Default)]
    struct FakeUsb {
        writes: RefCell<Vec<(Setup, Vec<u8>)>>,
        last_in: RefCell<Option<Setup>>,
        in_queue: RefCell<Vec<Vec<u8>>>,
    }

    impl UsbControl for FakeUsb {
        fn control_out(&self, setup: Setup, data: &[u8]) -> Result<(), AudioError> {
            self.writes.borrow_mut().push((setup, data.to_vec()));
            Ok(())
        }
        fn control_in(&self, setup: Setup, buf: &mut [u8]) -> Result<usize, AudioError> {
            *self.last_in.borrow_mut() = Some(setup);
            let canned = self.in_queue.borrow_mut().remove(0);
            let n = canned.len().min(buf.len());
            buf[..n].copy_from_slice(&canned[..n]);
            Ok(n)
        }
    }

    #[test]
    fn write_reg_one_byte_setup_packet() {
        // librtlsdr's first baseband write: rtlsdr_write_reg(dev, USBB, USB_SYSCTL, 0x09, 1).
        let usb = FakeUsb::default();
        write_reg(&usb, Block::Usb, 0x2000, 0x09, 1).unwrap();

        let writes = usb.writes.borrow();
        assert_eq!(writes.len(), 1);
        let (setup, data) = &writes[0];
        assert_eq!(
            *setup,
            Setup {
                request_type: 0x40,           // vendor | out | device
                request: 0,
                value: 0x2000,                // wValue = addr
                index: (1 << 8) | 0x10,       // block USBB in high byte + write flag
            }
        );
        assert_eq!(data, &[0x09]); // 1-byte value → low byte only
    }

    #[test]
    fn write_reg_two_byte_is_big_endian() {
        let usb = FakeUsb::default();
        write_reg(&usb, Block::Sys, 0x3000, 0x1234, 2).unwrap();

        let writes = usb.writes.borrow();
        let (setup, data) = &writes[0];
        assert_eq!(setup.request_type, 0x40);
        assert_eq!(setup.value, 0x3000);
        assert_eq!(setup.index, (2 << 8) | 0x10);
        assert_eq!(data, &[0x12, 0x34]); // high byte first on the wire
    }

    #[test]
    fn read_reg_setup_and_little_endian_assembly() {
        let usb = FakeUsb::default();
        usb.in_queue.borrow_mut().push(vec![0xcd, 0xab]); // data[0]=0xcd, data[1]=0xab
        let val = read_reg(&usb, Block::Sys, 0x0005, 2).unwrap();

        assert_eq!(
            usb.last_in.borrow().unwrap(),
            Setup {
                request_type: 0xC0,   // vendor | in | device
                request: 0,
                value: 0x0005,        // wValue = addr
                index: 2 << 8,        // block SYSB in high byte, no write flag
            }
        );
        assert_eq!(val, 0xabcd); // (data[1] << 8) | data[0]
    }

    #[test]
    fn known_dongle_ids_recognized() {
        assert!(is_rtl_dongle(0x0bda, 0x2838)); // generic RTL2832U OEM
        assert!(is_rtl_dongle(0x1d19, 0x1104)); // MSI DigiVox Micro HD (rebrand)
        assert!(!is_rtl_dongle(0x0bda, 0x0000)); // Realtek VID, non-RTL product
        assert!(!is_rtl_dongle(0x1234, 0x5678)); // unrelated device
    }

    #[test]
    fn sysfs_port_chain_parsing() {
        assert_eq!(parse_sysfs_port_chain("1-4.2", 1).as_deref(), Some("4.2"));
        assert_eq!(parse_sysfs_port_chain("2-1", 2).as_deref(), Some("1"));
        assert_eq!(parse_sysfs_port_chain("1-4.2", 2), None); // bus mismatch
        assert_eq!(parse_sysfs_port_chain("usb1", 1), None); // root hub, no port chain
    }
}
