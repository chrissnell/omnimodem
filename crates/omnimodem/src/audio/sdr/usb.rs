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

use super::{usb_regs, AudioError};
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
/// Demod-register flag OR'd into the low byte of `wValue` (the address rides the
/// high byte). librtlsdr `rtlsdr_demod_write_reg`.
const DEMOD_ADDR_FLAG: u16 = 0x20;
/// Write-page flag OR'd into `wIndex` for a demod write. librtlsdr `0x10 | page`.
const DEMOD_WRITE_FLAG: u16 = 0x10;

/// Realtek RTL2832U register blocks, selected in the high byte of the control
/// `wIndex`. Transcribed from librtlsdr's block enum; only some are used before
/// the tuner and streaming stages land.
#[allow(dead_code)] // Rom/Ir are addressed by later bring-up phases (P2-E onward).
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
    debug_assert!(len == 1 || len == 2, "register width must be 1 or 2 bytes, got {len}");
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
    debug_assert!(len == 1 || len == 2, "register width must be 1 or 2 bytes, got {len}");
    let mut data = [0u8; 2];
    let setup = Setup {
        request_type: CTRL_IN,
        request: 0,
        value: addr,
        index: (block as u16) << 8,
    };
    let n = usb.control_in(setup, &mut data[..len as usize])?;
    if n != len as usize {
        return Err(AudioError::Usb(format!(
            "short register read at {:#06x}: got {n} of {len} bytes",
            addr
        )));
    }
    Ok(((data[1] as u16) << 8) | data[0] as u16)
}

/// Write a demodulator register on `page`. The demod core uses a distinct encoding
/// from the block registers: the address rides the high byte of `wValue` (with the
/// `0x20` demod flag in the low byte), `page` sits in `wIndex` with the `0x10` write
/// flag, and a 2-byte value is big-endian. Every demod write is followed by a dummy
/// demod read that latches it — a hardware quirk replicated verbatim from librtlsdr
/// (`rtlsdr_demod_write_reg`); its result is discarded and a failed latch read never
/// fails the write.
pub(crate) fn demod_write_reg(
    usb: &impl UsbControl,
    page: u8,
    addr: u16,
    val: u16,
    len: u8,
) -> Result<(), AudioError> {
    debug_assert!(len == 1 || len == 2, "register width must be 1 or 2 bytes, got {len}");
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
        value: (addr << 8) | DEMOD_ADDR_FLAG,
        index: DEMOD_WRITE_FLAG | page as u16,
    };
    usb.control_out(setup, &data[..len as usize])?;

    // Latch read (librtlsdr reads demod page 0x0a / addr 0x01 after every write).
    let _ = demod_read_reg(usb, 0x0a, 0x01, 1);
    Ok(())
}

/// Read a demodulator register on `page`. Same address encoding as
/// [`demod_write_reg`], but `wIndex` carries only the page (no write flag) and the
/// value is assembled little-endian. Transcribed from librtlsdr
/// `rtlsdr_demod_read_reg`.
pub(crate) fn demod_read_reg(
    usb: &impl UsbControl,
    page: u8,
    addr: u16,
    len: u8,
) -> Result<u16, AudioError> {
    debug_assert!(len == 1 || len == 2, "register width must be 1 or 2 bytes, got {len}");
    let mut data = [0u8; 2];
    let setup = Setup {
        request_type: CTRL_IN,
        request: 0,
        value: (addr << 8) | DEMOD_ADDR_FLAG,
        index: page as u16,
    };
    let n = usb.control_in(setup, &mut data[..len as usize])?;
    if n != len as usize {
        return Err(AudioError::Usb(format!(
            "short demod read at page {page} addr {:#06x}: got {n} of {len} bytes",
            addr
        )));
    }
    Ok(((data[1] as u16) << 8) | data[0] as u16)
}

/// Issue an ordered [`usb_regs::RegWrite`] sequence, dispatching each entry to the
/// block ([`write_reg`]) or demod ([`demod_write_reg`]) control-transfer path.
pub(crate) fn apply_writes(
    usb: &impl UsbControl,
    ops: &[usb_regs::RegWrite],
) -> Result<(), AudioError> {
    for op in ops {
        match *op {
            usb_regs::RegWrite::Block { block, addr, val, len } => {
                write_reg(usb, block, addr, val, len)?;
            }
            usb_regs::RegWrite::Demod { page, addr, val, len } => {
                demod_write_reg(usb, page, addr, val, len)?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// I2C bus / R82xx tuner register I/O
// ---------------------------------------------------------------------------

/// Write one register on an I2C device hung off the RTL2832U's I2C bus. The RTL2832U
/// tunnels I2C through the `Iic` block: `wValue` carries the 7-bit device address and
/// the two-byte data stage is `[reg, val]`. Transcribed from librtlsdr
/// `rtlsdr_i2c_write` / `rtlsdr_write_array(IICB, ...)`.
fn i2c_write_reg(usb: &impl UsbControl, i2c_addr: u8, reg: u8, val: u8) -> Result<(), AudioError> {
    let setup = Setup {
        request_type: CTRL_OUT,
        request: 0,
        value: u16::from(i2c_addr),
        index: ((Block::Iic as u16) << 8) | 0x10,
    };
    usb.control_out(setup, &[reg, val])
}

/// Read one register from an I2C device: set the device's register pointer with a
/// one-byte write, then read the single byte back. Used only for the tuner-presence
/// probe. Transcribed from librtlsdr `rtlsdr_i2c_read_reg` (no bit reversal — that is
/// applied only by the R82xx status read).
fn i2c_read_reg(usb: &impl UsbControl, i2c_addr: u8, reg: u8) -> Result<u8, AudioError> {
    let write = Setup {
        request_type: CTRL_OUT,
        request: 0,
        value: u16::from(i2c_addr),
        index: ((Block::Iic as u16) << 8) | 0x10,
    };
    usb.control_out(write, &[reg])?;

    let read = Setup {
        request_type: CTRL_IN,
        request: 0,
        value: u16::from(i2c_addr),
        index: (Block::Iic as u16) << 8,
    };
    let mut data = [0u8; 1];
    let n = usb.control_in(read, &mut data)?;
    if n != 1 {
        return Err(AudioError::Usb(format!(
            "short i2c read at addr {i2c_addr:#04x} reg {reg:#04x}: got {n} bytes"
        )));
    }
    Ok(data[0])
}

/// Reverse the bit order of a byte. The R82xx returns its status registers MSB-first
/// per nibble, so [`r82xx_read`] un-reverses each byte. librtlsdr `r82xx_bitrev`.
fn r82xx_bitrev(byte: u8) -> u8 {
    const LUT: [u8; 16] = [
        0x0, 0x8, 0x4, 0xc, 0x2, 0xa, 0x6, 0xe, 0x1, 0x9, 0x5, 0xd, 0x3, 0xb, 0x7, 0xf,
    ];
    (LUT[(byte & 0xf) as usize] << 4) | LUT[(byte >> 4) as usize]
}

/// Read `buf.len()` R82xx status registers starting at register 0 and bit-reverse
/// each (the R82xx has no random-access read — a read always streams from reg 0).
/// Transcribed from librtlsdr `r82xx_read`.
fn r82xx_read(usb: &impl UsbControl, i2c_addr: u8, buf: &mut [u8]) -> Result<(), AudioError> {
    let setup = Setup {
        request_type: CTRL_IN,
        request: 0,
        value: u16::from(i2c_addr),
        index: (Block::Iic as u16) << 8,
    };
    let n = usb.control_in(setup, buf)?;
    if n != buf.len() {
        return Err(AudioError::Usb(format!(
            "short r82xx read at addr {:#04x}: got {n} of {} bytes",
            i2c_addr,
            buf.len()
        )));
    }
    for b in buf.iter_mut() {
        *b = r82xx_bitrev(*b);
    }
    Ok(())
}

/// Toggle the RTL2832U's I2C repeater, which gates control-transfer access to the
/// tuner bus. Transcribed from librtlsdr `rtlsdr_set_i2c_repeater`.
fn set_i2c_repeater(usb: &impl UsbControl, on: bool) -> Result<(), AudioError> {
    demod_write_reg(usb, 1, 0x01, if on { 0x18 } else { 0x10 }, 1)
}

/// Apply an ordered [`usb_regs::TunerOp`] sequence to the tuner over I2C, keeping
/// `shadow` (the driver's register-file mirror) in step so masked writes
/// read-modify-write against the last programmed value. Transcribed from librtlsdr
/// `r82xx_write_reg` / `r82xx_write_reg_mask` + the shadow in `r82xx_priv::regs`.
fn apply_tuner_ops(
    usb: &impl UsbControl,
    i2c_addr: u8,
    shadow: &mut [u8; usb_regs::NUM_REGS],
    ops: &[usb_regs::TunerOp],
) -> Result<(), AudioError> {
    for op in ops {
        let (reg, byte) = match *op {
            usb_regs::TunerOp::Write { reg, val } => (reg, val),
            usb_regs::TunerOp::Mask { reg, val, mask } => {
                let cur = shadow[reg as usize];
                (reg, (cur & !mask) | (val & mask))
            }
        };
        shadow[reg as usize] = byte;
        i2c_write_reg(usb, i2c_addr, reg, byte)?;
    }
    Ok(())
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
    /// The probed tuner, or `None` until [`probe_tuner`](Self::probe_tuner) runs.
    tuner: Option<usb_regs::TunerKind>,
    /// Mirror of the R82xx register file (indexed by absolute register), seeded by
    /// [`init_tuner`](Self::init_tuner) so masked writes read-modify-write correctly.
    tuner_shadow: [u8; usb_regs::NUM_REGS],
}

impl RtlUsbTransport {
    /// List USB devices, match `key` to a present RTL dongle, open it, and claim
    /// interface 0. On Linux the kernel DVB driver (`dvb_usb_rtl28xxu`) is detached
    /// as part of the claim; macOS/Windows claim directly. A claim that fails
    /// (driver still bound, no permission) maps to [`AudioError::UsbClaim`], which a
    /// later phase classifies into `needs_setup`.
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

        Ok(Self {
            usb: NusbControl { iface },
            iface: RTL_INTERFACE,
            tuner: None,
            tuner_shadow: [0u8; usb_regs::NUM_REGS],
        })
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

    /// Run the RTL2832U baseband bring-up ([`usb_regs::baseband_init`]) over the
    /// claimed control endpoint: USB block init, demod power-on/reset, FIR filter,
    /// and the SDR-mode/AGC/Zero-IF datapath configuration.
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn init_baseband(&self) -> Result<(), AudioError> {
        apply_writes(&self.usb, &usb_regs::baseband_init())
    }

    /// Program the RTL2832U resampler for `samp_rate`
    /// ([`usb_regs::sample_rate_writes`]), rejecting rates outside the resampler's
    /// usable window.
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn set_sample_rate(&self, samp_rate: u32) -> Result<(), AudioError> {
        apply_writes(&self.usb, &usb_regs::sample_rate_writes(samp_rate)?)
    }

    /// Probe the I2C bus for a supported tuner and record its kind. Walks the R820T
    /// then R828D addresses looking for the R82xx chip id, with the I2C repeater
    /// enabled for the duration. A non-R82xx dongle yields
    /// [`AudioError::UnsupportedTuner`]. Transcribed from librtlsdr's tuner-detect.
    #[allow(dead_code)] // Called from open()/apply_hardware once P2-D wires the transport.
    pub(crate) fn probe_tuner(&mut self) -> Result<usb_regs::TunerKind, AudioError> {
        set_i2c_repeater(&self.usb, true)?;
        let probe = (|| {
            for addr in [usb_regs::R820T_I2C_ADDR, usb_regs::R828D_I2C_ADDR] {
                let id = i2c_read_reg(&self.usb, addr, usb_regs::R82XX_CHECK_ADDR)?;
                if let Some(kind) = usb_regs::tuner_kind_from_probe(addr, id) {
                    return Ok(kind);
                }
            }
            Err(AudioError::UnsupportedTuner)
        })();
        // Best-effort release; the probe result is what matters.
        let _ = set_i2c_repeater(&self.usb, false);
        let kind = probe?;
        self.tuner = Some(kind);
        Ok(kind)
    }

    /// Load the R82xx power-on register image ([`usb_regs::r82xx_init_array`]) over
    /// I2C and seed the register shadow from it. Requires [`probe_tuner`](Self::probe_tuner)
    /// to have identified the tuner. Transcribed from librtlsdr `r82xx_init`
    /// (register-image write; IF-filter calibration is applied at first tune).
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn init_tuner(&mut self) -> Result<(), AudioError> {
        let tuner = self.tuner.ok_or(AudioError::UnsupportedTuner)?;
        let arr = usb_regs::r82xx_init_array();
        for (i, byte) in arr.iter().enumerate() {
            let reg = (usb_regs::REG_SHADOW_START + i) as u8;
            self.tuner_shadow[reg as usize] = *byte;
            i2c_write_reg(&self.usb, tuner.i2c_addr(), reg, *byte)?;
        }
        Ok(())
    }

    /// Tune the R82xx to RF frequency `rf_hz`: program the tracking-filter/mux band
    /// and solve + load the PLL for LO = `rf_hz + IF`. Transcribed from librtlsdr
    /// `r82xx_set_freq` → `r82xx_set_mux` + `r82xx_set_pll`.
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn set_tuner_freq(&mut self, rf_hz: u32) -> Result<(), AudioError> {
        let tuner = self.tuner.ok_or(AudioError::UnsupportedTuner)?;
        let lo_hz = rf_hz + usb_regs::R82XX_IF_FREQ;

        // vco_fine_tune lives in R4[5:4]; it nudges the PLL divider selector.
        let mut status = [0u8; 5];
        r82xx_read(&self.usb, tuner.i2c_addr(), &mut status)?;
        let vco_fine_tune = (status[4] & 0x30) >> 4;

        let mux = usb_regs::r82xx_mux_writes(lo_hz);
        apply_tuner_ops(&self.usb, tuner.i2c_addr(), &mut self.tuner_shadow, &mux)?;

        let pll = usb_regs::r82xx_pll(lo_hz, tuner, vco_fine_tune)?;
        let pll_ops = usb_regs::r82xx_pll_writes(&pll);
        apply_tuner_ops(&self.usb, tuner.i2c_addr(), &mut self.tuner_shadow, &pll_ops)
    }

    /// Switch RF gain to automatic (tuner AGC). Manual mode is entered by
    /// [`set_tuner_gain`](Self::set_tuner_gain), which flips the AGC off and loads the
    /// gain in one sequence. Transcribed from librtlsdr `r82xx_set_gain` (auto branch).
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn set_gain_mode(&mut self, auto: bool) -> Result<(), AudioError> {
        let tuner = self.tuner.ok_or(AudioError::UnsupportedTuner)?;
        // Only the auto path is a standalone mode flip; manual gain always carries a
        // level, so the manual AGC-off writes live in `set_tuner_gain`.
        if !auto {
            return Ok(());
        }
        let ops = usb_regs::r82xx_gain_auto_writes();
        apply_tuner_ops(&self.usb, tuner.i2c_addr(), &mut self.tuner_shadow, &ops)
    }

    /// Set a manual RF gain of `gain_tenths_db`, snapped to the R82xx gain table.
    /// Implies manual gain mode. Transcribed from librtlsdr `r82xx_set_gain`
    /// (manual branch).
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn set_tuner_gain(&mut self, gain_tenths_db: i32) -> Result<(), AudioError> {
        let tuner = self.tuner.ok_or(AudioError::UnsupportedTuner)?;
        let ops = usb_regs::r82xx_gain_manual_writes(gain_tenths_db);
        apply_tuner_ops(&self.usb, tuner.i2c_addr(), &mut self.tuner_shadow, &ops)
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
            // Unscripted reads (the demod-write latch reads) return no data; every
            // explicitly-tested read pushes its own canned payload.
            let mut queue = self.in_queue.borrow_mut();
            let canned = if queue.is_empty() { Vec::new() } else { queue.remove(0) };
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
    fn read_reg_one_byte_leaves_high_byte_zero() {
        let usb = FakeUsb::default();
        usb.in_queue.borrow_mut().push(vec![0x42]); // single-byte data stage
        let val = read_reg(&usb, Block::Demod, 0x0001, 1).unwrap();
        assert_eq!(val, 0x0042); // only data[0]; high byte untouched
    }

    #[test]
    fn read_reg_short_transfer_is_an_error() {
        let usb = FakeUsb::default();
        usb.in_queue.borrow_mut().push(vec![]); // device returned nothing
        let err = read_reg(&usb, Block::Sys, 0x0005, 2).unwrap_err();
        assert!(matches!(err, AudioError::Usb(_)));
    }

    #[test]
    fn demod_write_reg_setup_and_latch_read() {
        // librtlsdr's first baseband demod write: demod_write_reg(dev, 1, 0x01, 0x14, 1).
        let usb = FakeUsb::default();
        usb.in_queue.borrow_mut().push(vec![0x00]); // canned latch-read payload
        demod_write_reg(&usb, 1, 0x01, 0x14, 1).unwrap();

        let writes = usb.writes.borrow();
        assert_eq!(writes.len(), 1);
        let (setup, data) = &writes[0];
        assert_eq!(
            *setup,
            Setup {
                request_type: 0x40,             // vendor | out | device
                request: 0,
                value: (0x01 << 8) | 0x20,      // addr in high byte + demod flag
                index: 0x10 | 1,                // write flag + page 1
            }
        );
        assert_eq!(data, &[0x14]); // 1-byte value → low byte only

        // Every demod write is latched by a dummy read of demod page 0x0a / addr 0x01.
        assert_eq!(
            usb.last_in.borrow().unwrap(),
            Setup {
                request_type: 0xC0,             // vendor | in | device
                request: 0,
                value: (0x01 << 8) | 0x20,
                index: 0x0a,                    // page only, no write flag
            }
        );
    }

    #[test]
    fn demod_write_reg_two_byte_is_big_endian() {
        let usb = FakeUsb::default();
        demod_write_reg(&usb, 1, 0x9f, 0x0300, 2).unwrap();
        let writes = usb.writes.borrow();
        let (setup, data) = &writes[0];
        assert_eq!(setup.value, (0x9f << 8) | 0x20);
        assert_eq!(setup.index, 0x10 | 1);
        assert_eq!(data, &[0x03, 0x00]); // high byte first on the wire
    }

    #[test]
    fn init_baseband_emits_full_sequence_over_usb() {
        let usb = FakeUsb::default();
        apply_writes(&usb, &usb_regs::baseband_init()).unwrap();

        let writes = usb.writes.borrow();
        // Every op produces exactly one OUT transfer (latch reads are IN transfers).
        assert_eq!(writes.len(), usb_regs::baseband_init().len());
        // First OUT: USB_SYSCTL block write, block encoding.
        assert_eq!(
            writes[0].0,
            Setup { request_type: 0x40, request: 0, value: 0x2000, index: (1 << 8) | 0x10 }
        );
        assert_eq!(writes[0].1, vec![0x09]);
        // Sixth OUT (index 5): first demod write (soft reset), demod encoding.
        assert_eq!(
            writes[5].0,
            Setup { request_type: 0x40, request: 0, value: (0x01 << 8) | 0x20, index: 0x10 | 1 }
        );
        assert_eq!(writes[5].1, vec![0x14]);
    }

    #[test]
    fn set_sample_rate_emits_ratio_writes_over_usb() {
        let usb = FakeUsb::default();
        apply_writes(&usb, &usb_regs::sample_rate_writes(2_400_000).unwrap()).unwrap();

        let writes = usb.writes.borrow();
        assert_eq!(writes.len(), 4);
        // ratio 0x0300_0000 → high half 0x0300 into 0x9f, low half 0x0000 into 0xa1.
        assert_eq!(writes[0].0.value, (0x9f << 8) | 0x20);
        assert_eq!(writes[0].1, vec![0x03, 0x00]);
        assert_eq!(writes[1].0.value, (0xa1 << 8) | 0x20);
        assert_eq!(writes[1].1, vec![0x00, 0x00]);
        // Then the demod soft reset (assert / release).
        assert_eq!(writes[2].1, vec![0x14]);
        assert_eq!(writes[3].1, vec![0x10]);
    }

    #[test]
    fn known_dongle_ids_recognized() {
        assert!(is_rtl_dongle(0x0bda, 0x2838)); // generic RTL2832U OEM
        assert!(is_rtl_dongle(0x1d19, 0x1104)); // MSI DigiVox Micro HD (rebrand)
        assert!(!is_rtl_dongle(0x0bda, 0x0000)); // Realtek VID, non-RTL product
        assert!(!is_rtl_dongle(0x1234, 0x5678)); // unrelated device
    }

    #[test]
    fn i2c_write_reg_setup_and_payload() {
        // librtlsdr rtlsdr_i2c_write(reg,val) → write_array(IICB, addr, [reg,val]).
        let usb = FakeUsb::default();
        i2c_write_reg(&usb, 0x34, 0x05, 0x90).unwrap();

        let writes = usb.writes.borrow();
        assert_eq!(writes.len(), 1);
        let (setup, data) = &writes[0];
        assert_eq!(
            *setup,
            Setup {
                request_type: 0x40,               // vendor | out | device
                request: 0,
                value: 0x34,                      // i2c address in wValue
                index: ((Block::Iic as u16) << 8) | 0x10, // IICB + write flag
            }
        );
        assert_eq!(data, &[0x05, 0x90]); // [reg, val]
    }

    #[test]
    fn i2c_read_reg_writes_pointer_then_reads_one_byte() {
        // The probe reads a single register with no bit reversal.
        let usb = FakeUsb::default();
        usb.in_queue.borrow_mut().push(vec![0x69]);
        let val = i2c_read_reg(&usb, 0x34, 0x00).unwrap();
        assert_eq!(val, 0x69);

        // A one-byte register-pointer write precedes the read.
        let writes = usb.writes.borrow();
        assert_eq!(writes[0].1, vec![0x00]);
        assert_eq!(
            usb.last_in.borrow().unwrap(),
            Setup { request_type: 0xC0, request: 0, value: 0x34, index: (Block::Iic as u16) << 8 }
        );
    }

    #[test]
    fn r82xx_read_bit_reverses_each_byte() {
        let usb = FakeUsb::default();
        // 0x01 reverses to 0x80, 0x96 reverses to 0x69.
        usb.in_queue.borrow_mut().push(vec![0x01, 0x96]);
        let mut buf = [0u8; 2];
        r82xx_read(&usb, 0x34, &mut buf).unwrap();
        assert_eq!(buf, [0x80, 0x69]);
    }

    #[test]
    fn set_i2c_repeater_writes_demod_reg() {
        let usb = FakeUsb::default();
        set_i2c_repeater(&usb, true).unwrap();
        let writes = usb.writes.borrow();
        // demod_write_reg(1, 0x01, 0x18, 1): addr in high byte + demod flag, page 1.
        assert_eq!(writes[0].0.value, (0x01 << 8) | 0x20);
        assert_eq!(writes[0].0.index, 0x10 | 1);
        assert_eq!(writes[0].1, vec![0x18]);
    }

    #[test]
    fn apply_tuner_ops_read_modify_writes_against_shadow() {
        let usb = FakeUsb::default();
        let mut shadow = [0u8; usb_regs::NUM_REGS];
        shadow[0x10] = 0x6c; // seed reg 0x10 as init_array would

        apply_tuner_ops(
            &usb,
            0x34,
            &mut shadow,
            &[
                usb_regs::TunerOp::Mask { reg: 0x10, val: 0x60, mask: 0xe0 }, // div_num=3
                usb_regs::TunerOp::Write { reg: 0x14, val: 0x07 },
            ],
        )
        .unwrap();

        // Masked write keeps the low bits of 0x6c, replaces the top three: 0x6c → 0x6c.
        // (0x6c & !0xe0) | (0x60 & 0xe0) = 0x0c | 0x60 = 0x6c.
        assert_eq!(shadow[0x10], 0x6c);
        assert_eq!(shadow[0x14], 0x07);

        let writes = usb.writes.borrow();
        assert_eq!(writes[0].1, vec![0x10, 0x6c]); // [reg, merged byte]
        assert_eq!(writes[1].1, vec![0x14, 0x07]);
    }

    #[test]
    fn probe_reads_r820t_then_r828d_addresses() {
        // Drive the pure probe walk directly (RtlUsbTransport needs real hardware to
        // build). First address holds no R82xx (0x00), second answers 0x69 → R828D.
        let usb = FakeUsb::default();
        for addr in [usb_regs::R820T_I2C_ADDR, usb_regs::R828D_I2C_ADDR] {
            let want = if addr == usb_regs::R828D_I2C_ADDR { 0x69 } else { 0x00 };
            usb.in_queue.borrow_mut().push(vec![want]);
            let id = i2c_read_reg(&usb, addr, usb_regs::R82XX_CHECK_ADDR).unwrap();
            if let Some(kind) = usb_regs::tuner_kind_from_probe(addr, id) {
                assert_eq!(kind, usb_regs::TunerKind::R828D);
                return;
            }
        }
        panic!("probe should have matched R828D");
    }

    #[test]
    fn sysfs_port_chain_parsing() {
        assert_eq!(parse_sysfs_port_chain("1-4.2", 1).as_deref(), Some("4.2"));
        assert_eq!(parse_sysfs_port_chain("2-1", 2).as_deref(), Some("1"));
        assert_eq!(parse_sysfs_port_chain("1-4.2", 2), None); // bus mismatch
        assert_eq!(parse_sysfs_port_chain("usb1", 1), None); // root hub, no port chain
    }
}
