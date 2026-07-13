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

use super::{
    bias_tee_supported, direct_sampling_supported, pipeline, supported_sample_rates,
    tuner_freq_range, tuner_gains_db, tuner_name, usb_regs, AudioError, DemodMode, SdrControl,
    SdrTransport, TunerCaps, ADSB_CAPTURE_RATE, DEFAULT_CAPTURE_RATE, DEFAULT_DEVIATION_HZ,
};
use crate::audio::backend::{AudioBackend, CaptureHandle, PlaybackHandle};
use crate::audio::{AudioChunk, CHUNK_QUEUE_DEPTH, MAX_SAMPLE_RATE};
use crate::core::event::TelemetryEvent;
use crate::ids::{ChannelId, DeviceId, RtlKey};
use nusb::transfer::{Control, ControlType, Queue, Recipient, RequestBuffer};
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;
use tokio::sync::broadcast;

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

/// System-block GPIO output register. librtlsdr `GPO`.
const GPO: u16 = 0x3001;
/// System-block GPIO output-enable register. librtlsdr `GPOE`.
const GPOE: u16 = 0x3003;
/// System-block GPIO direction register (a clear bit selects output). librtlsdr `GPD`.
const GPD: u16 = 0x3004;
/// The GPIO pin the bias-tee power switch hangs off. librtlsdr defaults
/// `rtlsdr_set_bias_tee` to GPIO 0 (the RTL-SDR Blog V3 wiring).
const BIAS_TEE_GPIO: u8 = 0;

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

/// The RTL2832U streams sampled IQ on bulk-IN endpoint 0x81. librtlsdr `0x81`.
const BULK_ENDPOINT: u8 = 0x81;
/// Outstanding bulk-IN transfers kept submitted so the dongle never stalls waiting
/// for the host to hand back a buffer. librtlsdr's async reader defaults to 15; a
/// smaller depth is plenty at omnimodem's modest rates and keeps latency low.
const BULK_TRANSFERS: usize = 8;

/// `UsbControl` over a claimed `nusb::Interface`. The RTL2832U's register requests
/// are all vendor / device-recipient, so the direction alone distinguishes them;
/// nusb derives `bmRequestType` from the typed fields (matching [`CTRL_IN`] /
/// [`CTRL_OUT`], asserted in debug builds).
pub(crate) struct NusbControl {
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

// ---------------------------------------------------------------------------
// Bulk IQ streaming seam
// ---------------------------------------------------------------------------

/// The bulk-IN IQ source [`RtlUsbTransport::read_iq`] is written against, so the
/// streaming read (and the endpoint reset before it) is exercised with a fake and
/// no hardware. [`NusbBulk`] is the real implementation over the claimed interface's
/// bulk endpoint 0x81.
pub(crate) trait UsbBulk: Send {
    /// Fill `buf` with the next block of raw interleaved u8 IQ from the bulk-IN
    /// endpoint, blocking until data arrives. Returns the byte count, `Ok(0)` once
    /// a stop has been signalled via [`shutdown_handle`](UsbBulk::shutdown_handle),
    /// or an error on a terminal transfer failure.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, AudioError>;

    /// A closure that ends the stream so a parked [`read`](UsbBulk::read) returns
    /// `Ok(0)` on its next completion.
    fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send>;
}

/// `UsbBulk` over a claimed `nusb::Interface`: keeps a small pool of bulk-IN
/// transfers submitted on endpoint 0x81 and hands the pipeline each completed
/// buffer. The queue is built lazily on the first read so the (blocking) capture
/// thread owns it, and a shared stop flag ends the stream promptly (the dongle
/// streams continuously, so the next completion observes the flag within a block).
/// Mid-capture removal / hard-stall recovery is refined in P3-A.
pub(crate) struct NusbBulk {
    iface: nusb::Interface,
    queue: Option<Queue<RequestBuffer>>,
    stop: Arc<AtomicBool>,
}

impl NusbBulk {
    fn new(iface: nusb::Interface) -> Self {
        NusbBulk { iface, queue: None, stop: Arc::new(AtomicBool::new(false)) }
    }
}

impl UsbBulk for NusbBulk {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, AudioError> {
        if self.stop.load(Ordering::Relaxed) {
            return Ok(0);
        }
        let queue = self
            .queue
            .get_or_insert_with(|| self.iface.bulk_in_queue(BULK_ENDPOINT));
        // Keep the transfer pool full so the dongle never stalls between reads.
        while queue.pending() < BULK_TRANSFERS {
            queue.submit(RequestBuffer::new(buf.len()));
        }
        let completion = block_on(queue.next_complete());
        if self.stop.load(Ordering::Relaxed) {
            return Ok(0);
        }
        completion
            .status
            .map_err(|e| AudioError::Usb(format!("bulk in transfer: {e}")))?;
        let n = completion.data.len().min(buf.len());
        buf[..n].copy_from_slice(&completion.data[..n]);
        // Reuse the completed buffer for a fresh transfer, keeping the pool primed.
        queue.submit(RequestBuffer::reuse(completion.data, buf.len()));
        Ok(n)
    }

    fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send> {
        let stop = self.stop.clone();
        Box::new(move || stop.store(true, Ordering::Relaxed))
    }
}

/// Drive a `nusb` transfer future to completion on the calling thread by parking
/// until the transfer's waker unparks us. The capture thread is dedicated to this
/// one source, so a thread-parking executor is enough — it avoids pulling a full
/// async runtime onto the blocking [`SdrTransport::read_iq`] path.
fn block_on<F: Future>(future: F) -> F::Output {
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
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

/// Switch the bias-tee (inline LNA/antenna power over coax) on or off by driving the
/// [`BIAS_TEE_GPIO`] pin. Configures the pin as an output (clear its direction bit,
/// set its output-enable bit) then read-modify-writes the output level. Every step is
/// a read-modify-write against the current register so unrelated GPIO bits are
/// preserved. Transcribed from librtlsdr `rtlsdr_set_bias_tee_gpio` →
/// `rtlsdr_set_gpio_output` + `rtlsdr_set_gpio_bit`.
pub(crate) fn set_bias_tee(usb: &impl UsbControl, on: bool) -> Result<(), AudioError> {
    let bit = 1u16 << BIAS_TEE_GPIO;
    // Direction: clear the bit (0 = output) and enable the output driver.
    let gpd = read_reg(usb, Block::Sys, GPD, 1)?;
    write_reg(usb, Block::Sys, GPD, gpd & !bit, 1)?;
    let gpoe = read_reg(usb, Block::Sys, GPOE, 1)?;
    write_reg(usb, Block::Sys, GPOE, gpoe | bit, 1)?;
    // Level: drive the pin high to power the bias-tee, low to remove it.
    let gpo = read_reg(usb, Block::Sys, GPO, 1)?;
    let gpo = if on { gpo | bit } else { gpo & !bit };
    write_reg(usb, Block::Sys, GPO, gpo, 1)
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

/// A locally-attached RTL-SDR dongle, opened, claimed, and brought up for exclusive
/// use. [`open`](RtlUsbTransport::open) claims interface 0 (detaching the Linux
/// kernel driver) and runs the full bring-up (baseband init, tuner probe/init), so a
/// returned transport can be tuned, streamed, and its caps read immediately.
///
/// Generic over the register ([`UsbControl`]) and bulk-IQ ([`UsbBulk`]) seams so the
/// streaming [`SdrTransport`] impl is exercised end-to-end with fakes and no
/// hardware; production uses the [`NusbControl`] / [`NusbBulk`] defaults over one
/// claimed interface.
pub(crate) struct RtlUsbTransport<C = NusbControl, B = NusbBulk> {
    usb: C,
    /// Bulk-IN IQ source, driven by [`read_iq`](SdrTransport::read_iq).
    bulk: B,
    /// The probed tuner, or `None` until [`probe_tuner`](Self::probe_tuner) runs.
    tuner: Option<usb_regs::TunerKind>,
    /// Mirror of the R82xx register file (indexed by absolute register), seeded by
    /// [`init_tuner`](Self::init_tuner) so masked writes read-modify-write correctly.
    tuner_shadow: [u8; usb_regs::NUM_REGS],
    /// Last sample rate programmed into the resampler. `apply_hardware` reprograms
    /// the rate only when this changes, mirroring `RtlTcpTransport` (the resampler
    /// reset is disruptive to re-issue on a routine tune/gain change).
    last_rate: Option<u32>,
    /// Whether the bulk-IN FIFO has been reset (`rtlsdr_reset_buffer`) for this
    /// capture. Done once, lazily, on the first [`read_iq`](SdrTransport::read_iq).
    reset_done: bool,
}

impl RtlUsbTransport<NusbControl, NusbBulk> {
    /// List USB devices, match `key` to a present RTL dongle, open it, claim
    /// interface 0, and bring the device up (baseband + tuner probe/init). On Linux the
    /// kernel DVB driver (`dvb_usb_rtl28xxu`) is detached as part of the claim;
    /// macOS/Windows claim directly. A claim that fails (driver still bound, no
    /// permission) maps to [`AudioError::UsbClaim`], which a later phase classifies into
    /// `needs_setup`; a device with an unsupported tuner fails at bring-up.
    ///
    /// Bring-up runs here (not lazily) so a returned transport is fully initialized and
    /// its caps are available immediately — matching `RtlTcpTransport`, which
    /// handshakes and publishes caps in its constructor.
    pub(crate) fn open(key: &RtlKey) -> Result<Self, AudioError> {
        let id = DeviceId::Rtl { key: key.clone() }.to_canonical_string();

        let info = nusb::list_devices()
            .map_err(|e| AudioError::Usb(format!("enumerate usb: {e}")))?
            .find(|d| is_rtl_dongle(d.vendor_id(), d.product_id()) && key_matches(d, key))
            .ok_or_else(|| AudioError::DeviceNotFound(id.clone()))?;

        let dev = info
            .open()
            .map_err(|e| AudioError::UsbClaim(id.clone(), format!("open device: {e}")))?;
        let iface = claim_interface(&dev, RTL_INTERFACE, &id)?;
        // The register and bulk seams share one claimed interface (nusb's `Interface`
        // is a cheap clonable handle to it).
        let bulk = NusbBulk::new(iface.clone());

        let mut transport = Self {
            usb: NusbControl { iface },
            bulk,
            tuner: None,
            tuner_shadow: [0u8; usb_regs::NUM_REGS],
            last_rate: None,
            reset_done: false,
        };
        transport.bring_up()?;
        Ok(transport)
    }
}

impl<C: UsbControl, B> RtlUsbTransport<C, B> {
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

    /// Run `body` with the RTL2832U's I2C repeater enabled, releasing it afterward
    /// (best-effort) whatever the outcome. Every tuner register access must be
    /// bracketed this way: librtlsdr wraps `set_i2c_repeater(1)…(0)` around all tuner
    /// i2c, and a transfer issued with the repeater off silently never reaches the
    /// R82xx.
    fn with_repeater<T>(
        &mut self,
        body: impl FnOnce(&mut Self) -> Result<T, AudioError>,
    ) -> Result<T, AudioError> {
        set_i2c_repeater(&self.usb, true)?;
        let out = body(self);
        let _ = set_i2c_repeater(&self.usb, false);
        out
    }

    /// Probe the I2C bus for a supported tuner and record its kind. Walks the R820T
    /// then R828D addresses looking for the R82xx chip id. A non-R82xx dongle yields
    /// [`AudioError::UnsupportedTuner`]. Transcribed from librtlsdr's tuner-detect.
    #[allow(dead_code)] // Called from open()/apply_hardware once P2-D wires the transport.
    pub(crate) fn probe_tuner(&mut self) -> Result<usb_regs::TunerKind, AudioError> {
        let kind = self.with_repeater(|s| {
            for addr in [usb_regs::R820T_I2C_ADDR, usb_regs::R828D_I2C_ADDR] {
                let id = i2c_read_reg(&s.usb, addr, usb_regs::R82XX_CHECK_ADDR)?;
                if let Some(kind) = usb_regs::tuner_kind_from_probe(addr, id) {
                    return Ok(kind);
                }
            }
            Err(AudioError::UnsupportedTuner)
        })?;
        self.tuner = Some(kind);
        Ok(kind)
    }

    /// Load the R82xx power-on register image ([`usb_regs::r82xx_init_array`]) over
    /// I2C and seed the register shadow from it. Requires [`probe_tuner`](Self::probe_tuner)
    /// to have identified the tuner. Transcribed from librtlsdr `r82xx_init`
    /// (register-image write only; the IF-filter calibration and sysfreq programming
    /// of `r82xx_set_tv_standard` / `r82xx_sysfreq_sel` are not yet applied — the
    /// fixed 3.57 MHz digital-TV IF still lets the mux + PLL tune correctly).
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn init_tuner(&mut self) -> Result<(), AudioError> {
        let tuner = self.tuner.ok_or(AudioError::UnsupportedTuner)?;
        self.with_repeater(|s| {
            let arr = usb_regs::r82xx_init_array();
            for (i, byte) in arr.iter().enumerate() {
                let reg = (usb_regs::REG_SHADOW_START + i) as u8;
                s.tuner_shadow[reg as usize] = *byte;
                i2c_write_reg(&s.usb, tuner.i2c_addr(), reg, *byte)?;
            }
            Ok(())
        })
    }

    /// Tune the R82xx to RF frequency `rf_hz`: program the tracking-filter/mux band,
    /// solve + load the PLL for LO = `rf_hz + IF`, then (R828D only) switch the RF
    /// input for the band. Transcribed from librtlsdr `r82xx_set_freq` →
    /// `r82xx_set_mux` + `r82xx_set_pll` + the R828D Cable1/Air-In switch.
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn set_tuner_freq(&mut self, rf_hz: u32) -> Result<(), AudioError> {
        let tuner = self.tuner.ok_or(AudioError::UnsupportedTuner)?;
        let lo_hz = rf_hz + usb_regs::R82XX_IF_FREQ;
        self.with_repeater(|s| {
            let addr = tuner.i2c_addr();

            // vco_fine_tune lives in R4[5:4] and nudges the PLL divider selector.
            // librtlsdr reads it mid-`set_pll`; reading it up front is equivalent on a
            // settled VCO (and on a first tune it reads the reset default = pivot).
            let mut status = [0u8; 5];
            r82xx_read(&s.usb, addr, &mut status)?;
            let vco_fine_tune = (status[4] & 0x30) >> 4;

            apply_tuner_ops(&s.usb, addr, &mut s.tuner_shadow, &usb_regs::r82xx_mux_writes(lo_hz))?;

            let pll = usb_regs::r82xx_pll(lo_hz, tuner, vco_fine_tune)?;
            apply_tuner_ops(&s.usb, addr, &mut s.tuner_shadow, &usb_regs::r82xx_pll_writes(&pll))?;

            apply_tuner_ops(&s.usb, addr, &mut s.tuner_shadow, &usb_regs::r82xx_input_writes(tuner, rf_hz))
        })
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
        self.with_repeater(|s| {
            let ops = usb_regs::r82xx_gain_auto_writes();
            apply_tuner_ops(&s.usb, tuner.i2c_addr(), &mut s.tuner_shadow, &ops)
        })
    }

    /// Set a manual RF gain of `gain_tenths_db`, snapped to the R82xx gain table.
    /// Implies manual gain mode. Transcribed from librtlsdr `r82xx_set_gain`
    /// (manual branch).
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn set_tuner_gain(&mut self, gain_tenths_db: i32) -> Result<(), AudioError> {
        let tuner = self.tuner.ok_or(AudioError::UnsupportedTuner)?;
        self.with_repeater(|s| {
            let ops = usb_regs::r82xx_gain_manual_writes(gain_tenths_db);
            apply_tuner_ops(&s.usb, tuner.i2c_addr(), &mut s.tuner_shadow, &ops)
        })
    }

    /// Apply a `ppm` frequency correction to the RTL2832U resampler
    /// ([`usb_regs::freq_correction_writes`]). This programs only the demod IF-offset
    /// registers; the tuner PLL runs off the nominal crystal, so a large correction is
    /// approximate — matching the `rtl_tcp` opcode path, which likewise carries ppm as
    /// a standalone command.
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn set_freq_correction(&self, ppm: i32) -> Result<(), AudioError> {
        apply_writes(&self.usb, &usb_regs::freq_correction_writes(ppm))
    }

    /// Switch the bias-tee on or off (see [`set_bias_tee`]).
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn set_bias_tee(&self, on: bool) -> Result<(), AudioError> {
        set_bias_tee(&self.usb, on)
    }

    /// Select a direct-sampling `mode` (0 = off, 1 = I-branch, 2 = Q-branch), applying
    /// the demod datapath writes from [`usb_regs::direct_sampling_writes`]. Mode 0
    /// restores the R82xx tuned path (3.57 MHz IF + spectrum inversion).
    #[allow(dead_code)] // Called from apply_hardware once it lands in P2-D.
    pub(crate) fn set_direct_sampling(&self, mode: u32) -> Result<(), AudioError> {
        apply_writes(&self.usb, &usb_regs::direct_sampling_writes(mode))
    }

    /// One-time device bring-up: RTL2832U baseband init, tuner probe, and R82xx tuner
    /// init. Run by `open` so a fresh transport is fully initialized; idempotent once
    /// the tuner is known, so `apply_hardware` can guard on it defensively and pay the
    /// cost only on a not-yet-brought-up transport.
    fn bring_up(&mut self) -> Result<(), AudioError> {
        if self.tuner.is_some() {
            return Ok(());
        }
        self.init_baseband()?;
        self.probe_tuner()?;
        self.init_tuner()
    }

}

impl<C: UsbControl + Send, B: UsbBulk> SdrTransport for RtlUsbTransport<C, B> {
    /// Reset the bulk-IN FIFO once (lazily, on the first read for this capture), then
    /// hand the pipeline the next block of raw u8 IQ from the bulk endpoint. The reset
    /// is a control transfer on the register seam; the read is a bulk transfer on the
    /// IQ seam. Returns `Ok(0)` once the stream has been stopped.
    fn read_iq(&mut self, buf: &mut [u8]) -> Result<usize, AudioError> {
        if !self.reset_done {
            apply_writes(&self.usb, &usb_regs::reset_buffer_writes())?;
            self.reset_done = true;
        }
        self.bulk.read(buf)
    }

    /// Apply the current hardware parameters from `control` onto the dongle, in the
    /// exact order [`RtlTcpTransport`](super::rtl_tcp::RtlTcpTransport) sends them:
    /// sample rate (only when it changed), ppm, direct-sampling, bias-tee, gain
    /// mode/level, then center frequency. Brings the device up on the first call. See
    /// [`plan_apply_hardware`] for the pure ordering the mock-seam test asserts.
    fn apply_hardware(&mut self, control: &SdrControl) -> Result<(), AudioError> {
        self.bring_up()?;
        let rate = control.capture_rate();
        let send_rate = self.last_rate != Some(rate);
        for op in plan_apply_hardware(control, send_rate) {
            match op {
                HwCall::SampleRate(r) => self.set_sample_rate(r)?,
                HwCall::FreqCorrection(ppm) => self.set_freq_correction(ppm)?,
                HwCall::DirectSampling(mode) => self.set_direct_sampling(mode)?,
                HwCall::BiasTee(on) => self.set_bias_tee(on)?,
                HwCall::GainMode { auto } => self.set_gain_mode(auto)?,
                HwCall::TunerGain(tenths) => self.set_tuner_gain(tenths)?,
                HwCall::CenterFreq(hz) => self.set_tuner_freq(hz)?,
            }
        }
        if send_rate {
            self.last_rate = Some(rate);
        }
        Ok(())
    }

    /// The capabilities of the probed tuner, derived from the same per-tuner tables the
    /// `rtl_tcp` path publishes so `GetSdrCaps` answers identically. The tuner is always
    /// probed by [`open`](RtlUsbTransport::open), so this is infallible on any transport
    /// a caller can hold.
    fn caps(&self) -> TunerCaps {
        caps_for_tuner(self.tuner.expect("tuner probed by open()"))
    }

    /// End the bulk stream so the capture thread's next [`read_iq`](Self::read_iq)
    /// returns `Ok(0)` (delegated to the bulk seam's stop flag).
    fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send> {
        self.bulk.shutdown_handle()
    }
}

/// One hardware parameter operation [`RtlUsbTransport`]'s `apply_hardware` performs, in
/// the order it performs them. Modelling the plan as data keeps the ordering (and the
/// gain-mode / rate-gating branches) unit-testable against a mock with no USB.
#[derive(Debug, Clone, Copy, PartialEq)]
enum HwCall {
    SampleRate(u32),
    FreqCorrection(i32),
    DirectSampling(u32),
    BiasTee(bool),
    GainMode { auto: bool },
    TunerGain(i32),
    CenterFreq(u32),
}

/// The ordered hardware operations for a control snapshot, mirroring
/// `RtlTcpTransport::send_hardware` exactly: sample rate first (gated on `send_rate`
/// so a routine tune does not reprogram the resampler), then ppm, direct-sampling,
/// bias-tee, gain mode, an explicit manual gain level, and finally the center
/// frequency. Pure, so the `apply_hardware` ordering is asserted with no hardware.
fn plan_apply_hardware(control: &SdrControl, send_rate: bool) -> Vec<HwCall> {
    let mut ops = Vec::new();
    if send_rate {
        ops.push(HwCall::SampleRate(control.capture_rate()));
    }
    ops.push(HwCall::FreqCorrection(control.ppm()));
    ops.push(HwCall::DirectSampling(control.direct_sampling()));
    ops.push(HwCall::BiasTee(control.bias_tee()));
    ops.push(HwCall::GainMode { auto: control.gain_auto() });
    if !control.gain_auto() {
        let tenths = (control.gain_db() * 10.0).round() as i32;
        ops.push(HwCall::TunerGain(tenths));
    }
    ops.push(HwCall::CenterFreq(control.center_hz() as u32));
    ops
}

/// The [`TunerCaps`] for a probed [`TunerKind`](usb_regs::TunerKind), built from the
/// shared per-tuner tables keyed by the tuner's `rtl_tcp` type code so the native USB
/// path and the `rtl_tcp` path publish identical capabilities.
fn caps_for_tuner(tuner: usb_regs::TunerKind) -> TunerCaps {
    let t = tuner.type_code();
    let (freq_min_hz, freq_max_hz) = tuner_freq_range(t);
    TunerCaps {
        tuner: tuner_name(t).to_string(),
        freq_min_hz,
        freq_max_hz,
        sample_rates: supported_sample_rates(),
        gains_db: tuner_gains_db(t),
        bias_tee_supported: bias_tee_supported(t),
        direct_sampling_supported: direct_sampling_supported(t),
    }
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Opens the [`SdrTransport`] a capture will drive. Production opens the real USB
/// dongle (and applies the current hardware params); tests inject a fake so the
/// backend + shared pipeline run end-to-end with no hardware. The concrete transport
/// type is erased to a trait object because it differs between the two.
type TransportOpener =
    Box<dyn Fn(&SdrControl) -> Result<Box<dyn SdrTransport>, AudioError> + Send>;

/// A locally-attached RTL-SDR dongle bound as an audio capture device — the native
/// USB analogue of [`RtlTcpBackend`](super::rtl_tcp::RtlTcpBackend). RX-only:
/// dongles cannot transmit, so `open_playback` reports `Unsupported`. Each
/// `open_capture` opens a fresh [`RtlUsbTransport`] and spawns the shared
/// [`run_capture`](pipeline::run_capture) on it, so the entire IQ→audio DSP chain and
/// the runtime tune/gain/squelch control surface are reused verbatim from `rtl_tcp`.
pub struct RtlUsbBackend {
    key: RtlKey,
    capture_rate: u32,
    deviation_hz: f32,
    control: SdrControl,
    telemetry: Option<broadcast::Sender<TelemetryEvent>>,
    channel: ChannelId,
    open_transport: TransportOpener,
}

impl RtlUsbBackend {
    /// Construct a backend bound to the dongle identified by `key`, with default
    /// capture rate/deviation and a fresh control cell. The core replaces the control
    /// and wires the telemetry sink + channel via
    /// [`AudioBackend::attach_sdr_context`] before `open_capture`.
    pub fn new(key: RtlKey) -> Self {
        let open_key = key.clone();
        let open_transport: TransportOpener = Box::new(move |control| {
            // Open + bring the dongle up, then command the current hardware params so
            // the tuner/rate/gain match `control` before streaming, mirroring
            // `RtlTcpTransport::connect`'s handshake.
            let mut transport = RtlUsbTransport::open(&open_key)?;
            transport.apply_hardware(control)?;
            Ok(Box::new(transport) as Box<dyn SdrTransport>)
        });
        RtlUsbBackend {
            key,
            capture_rate: DEFAULT_CAPTURE_RATE,
            deviation_hz: DEFAULT_DEVIATION_HZ,
            control: SdrControl::default(),
            telemetry: None,
            channel: ChannelId(0),
            open_transport,
        }
    }

    /// The shared control cell (so the core can store a clone for gRPC to mutate).
    pub fn control(&self) -> SdrControl {
        self.control.clone()
    }

    /// Override the dongle capture (sample) rate. Kept a multiple of the audio
    /// channel rate so the complex decimator has an integer ratio.
    pub fn with_capture_rate(mut self, rate: u32) -> Self {
        self.capture_rate = rate;
        self
    }

    /// Construct a backend whose transport comes from `opener` instead of a real
    /// dongle, so the backend + pipeline are driven by a fake USB source in tests.
    #[cfg(test)]
    fn with_transport_opener(key: RtlKey, opener: TransportOpener) -> Self {
        RtlUsbBackend {
            key,
            capture_rate: DEFAULT_CAPTURE_RATE,
            deviation_hz: DEFAULT_DEVIATION_HZ,
            control: SdrControl::default(),
            telemetry: None,
            channel: ChannelId(0),
            open_transport: opener,
        }
    }
}

impl AudioBackend for RtlUsbBackend {
    fn open_capture(&self, requested_rate: u32) -> Result<CaptureHandle, AudioError> {
        // ADS-B binds the channel to the wideband `RawMag` path (full 2.4 Msps
        // magnitude envelope); every other mode decimates to the capped audio rate.
        // Mirrors `RtlTcpBackend::open_capture` exactly.
        let raw_mag = self.control.demod_mode() == DemodMode::RawMag;
        let seed_rate = if raw_mag { ADSB_CAPTURE_RATE } else { self.capture_rate };
        let channel_rate =
            if raw_mag { seed_rate } else { requested_rate.min(MAX_SAMPLE_RATE) };

        // Seed the shared capture rate; the control cell is authoritative thereafter,
        // so `ConfigureSdr` can change the rate on a running (audio) capture.
        self.control.set_capture_rate(seed_rate);

        // Open synchronously so a missing/unclaimable dongle fails fast and the tuner
        // caps publish before we return. The transport hides every later USB detail
        // from the pipeline.
        let transport = (self.open_transport)(&self.control)?;
        self.control.set_caps(transport.caps());
        let shutdown = transport.shutdown_handle();

        let (tx, rx) = sync_channel::<AudioChunk>(CHUNK_QUEUE_DEPTH);
        let control = self.control.clone();
        let telemetry = self.telemetry.clone();
        let channel = self.channel;
        let deviation_hz = self.deviation_hz;

        std::thread::Builder::new()
            .name("omni-rtl-usb-capture".into())
            .spawn(move || {
                pipeline::run_capture(
                    transport,
                    control,
                    telemetry,
                    channel,
                    deviation_hz,
                    channel_rate,
                    tx,
                );
            })
            .map_err(|e| AudioError::Io(e.to_string()))?;

        Ok(CaptureHandle::new(rx, channel_rate, shutdown))
    }

    fn open_playback(&self, _requested_rate: u32) -> Result<PlaybackHandle, AudioError> {
        // RTL dongles are receive-only.
        Err(AudioError::Unsupported)
    }

    fn device_id(&self) -> DeviceId {
        DeviceId::Rtl { key: self.key.clone() }
    }

    fn attach_sdr_context(
        &mut self,
        channel: ChannelId,
        telemetry: broadcast::Sender<TelemetryEvent>,
        control: SdrControl,
    ) {
        self.channel = channel;
        self.telemetry = Some(telemetry);
        self.control = control;
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
impl<C, B> RtlUsbTransport<C, B> {
    /// Build a transport directly from fake seams with the tuner already identified,
    /// so a test drives streaming / `apply_hardware` without the hardware-only
    /// [`open`](RtlUsbTransport::open) bring-up.
    fn from_fake(usb: C, bulk: B, tuner: usb_regs::TunerKind) -> Self {
        Self {
            usb,
            bulk,
            tuner: Some(tuner),
            tuner_shadow: [0u8; usb_regs::NUM_REGS],
            last_rate: None,
            reset_done: false,
        }
    }
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

    #[test]
    fn set_bias_tee_read_modify_writes_the_gpio() {
        // Configure GPIO0 as an output, then drive it high. Canned reads: GPD=0xff,
        // GPOE=0x00, GPO=0x00 (bit 0 clear, other bits arbitrary).
        let usb = FakeUsb::default();
        usb.in_queue.borrow_mut().push(vec![0xff]); // GPD
        usb.in_queue.borrow_mut().push(vec![0x00]); // GPOE
        usb.in_queue.borrow_mut().push(vec![0x00]); // GPO
        set_bias_tee(&usb, true).unwrap();

        let writes = usb.writes.borrow();
        assert_eq!(writes.len(), 3);
        // GPD: clear bit 0 (0xff → 0xfe), addressed as a SYSB block write.
        assert_eq!(writes[0].0.value, GPD);
        assert_eq!(writes[0].0.index, ((Block::Sys as u16) << 8) | 0x10);
        assert_eq!(writes[0].1, vec![0xfe]);
        // GPOE: set bit 0 (0x00 → 0x01).
        assert_eq!(writes[1].0.value, GPOE);
        assert_eq!(writes[1].1, vec![0x01]);
        // GPO: drive high (0x00 → 0x01).
        assert_eq!(writes[2].0.value, GPO);
        assert_eq!(writes[2].1, vec![0x01]);
    }

    #[test]
    fn set_bias_tee_off_clears_only_the_pin() {
        // GPO already has other bits set; turning the bias-tee off must clear only bit 0.
        let usb = FakeUsb::default();
        usb.in_queue.borrow_mut().push(vec![0x00]); // GPD
        usb.in_queue.borrow_mut().push(vec![0x00]); // GPOE
        usb.in_queue.borrow_mut().push(vec![0x05]); // GPO: bits 0 and 2 set
        set_bias_tee(&usb, false).unwrap();

        let writes = usb.writes.borrow();
        // GPO write clears bit 0 (0x05 → 0x04), preserving bit 2.
        assert_eq!(writes[2].0.value, GPO);
        assert_eq!(writes[2].1, vec![0x04]);
    }

    #[test]
    fn apply_hardware_plan_matches_rtl_tcp_order() {
        let c = SdrControl::default();
        c.set_capture_rate(2_400_000);
        c.set_ppm(-5);
        c.set_direct_sampling(2);
        c.set_bias_tee(true);
        c.set_gain(true, 0.0); // automatic AGC
        c.set_center_hz(144_390_000.0);

        // Auto gain with the rate to (re)send: rate first, no TunerGain, then center.
        assert_eq!(
            plan_apply_hardware(&c, true),
            vec![
                HwCall::SampleRate(2_400_000),
                HwCall::FreqCorrection(-5),
                HwCall::DirectSampling(2),
                HwCall::BiasTee(true),
                HwCall::GainMode { auto: true },
                HwCall::CenterFreq(144_390_000),
            ]
        );

        // Manual gain with the rate unchanged: SampleRate is omitted and a TunerGain
        // (tenths) follows the manual GainMode, mirroring rtl_tcp exactly.
        c.set_gain(false, 20.7);
        assert_eq!(
            plan_apply_hardware(&c, false),
            vec![
                HwCall::FreqCorrection(-5),
                HwCall::DirectSampling(2),
                HwCall::BiasTee(true),
                HwCall::GainMode { auto: false },
                HwCall::TunerGain(207),
                HwCall::CenterFreq(144_390_000),
            ]
        );
    }

    #[test]
    fn caps_for_tuner_matches_the_shared_tables() {
        let caps = caps_for_tuner(usb_regs::TunerKind::R820T);
        assert_eq!(caps.tuner, "R820T");
        assert_eq!(caps.gains_db.len(), 29); // the canonical R82xx gain table
        assert!(caps.bias_tee_supported); // R820-class: bias-tee capable
        assert!(caps.direct_sampling_supported); // universal ADC feature
        assert!(caps.sample_rates.contains(&240_000)); // the 240 kHz default
        assert!(caps.freq_min_hz < caps.freq_max_hz);

        // R828D reuses the same table set, reporting under its own tuner name.
        assert_eq!(caps_for_tuner(usb_regs::TunerKind::R828D).tuner, "R828D");
    }

    // -----------------------------------------------------------------------
    // Bulk streaming + backend
    // -----------------------------------------------------------------------

    /// A scripted [`UsbBulk`]: hands `read` a fixed IQ slice in `buf`-sized blocks,
    /// then reports the terminal `Ok(0)`. A [`shutdown_handle`](UsbBulk::shutdown_handle)
    /// ends it early, mirroring how the real stop flag unblocks the nusb queue.
    struct FakeBulk {
        iq: Vec<u8>,
        pos: usize,
        stop: Arc<AtomicBool>,
    }

    impl FakeBulk {
        fn new(iq: Vec<u8>) -> Self {
            FakeBulk { iq, pos: 0, stop: Arc::new(AtomicBool::new(false)) }
        }
    }

    impl UsbBulk for FakeBulk {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, AudioError> {
            if self.stop.load(Ordering::Relaxed) || self.pos >= self.iq.len() {
                return Ok(0);
            }
            let n = buf.len().min(self.iq.len() - self.pos);
            buf[..n].copy_from_slice(&self.iq[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }

        fn shutdown_handle(&self) -> Box<dyn FnOnce() + Send> {
            let stop = self.stop.clone();
            Box::new(move || stop.store(true, Ordering::Relaxed))
        }
    }

    #[test]
    fn read_iq_resets_the_endpoint_once_then_streams() {
        // The first read must reset the bulk-IN FIFO (rtlsdr_reset_buffer) via the
        // control seam, then hand back the bulk bytes; later reads must not re-reset.
        let usb = FakeUsb::default();
        let bulk = FakeBulk::new(vec![1, 2, 3, 4, 5, 6]);
        let mut transport = RtlUsbTransport::from_fake(usb, bulk, usb_regs::TunerKind::R820T);

        let mut buf = [0u8; 4];
        let n = transport.read_iq(&mut buf).unwrap();
        assert_eq!(&buf[..n], &[1, 2, 3, 4]);

        // Exactly the two USB_EPA_CTL writes (0x1002 stop/reset, then 0x0000 enable).
        {
            let writes = transport.usb.writes.borrow();
            assert_eq!(writes.len(), 2, "reset should emit exactly two control writes");
            assert_eq!(writes[0].0.value, 0x2148); // USB_EPA_CTL
            assert_eq!(writes[0].0.index, ((Block::Usb as u16) << 8) | 0x10);
            assert_eq!(writes[0].1, vec![0x10, 0x02]);
            assert_eq!(writes[1].1, vec![0x00, 0x00]);
        }

        // A second read streams the remainder without re-issuing the reset.
        let n = transport.read_iq(&mut buf).unwrap();
        assert_eq!(&buf[..n], &[5, 6]);
        assert_eq!(transport.usb.writes.borrow().len(), 2, "reset must run only once");

        // Exhausted → terminal.
        assert_eq!(transport.read_iq(&mut buf).unwrap(), 0);
    }

    #[test]
    fn shutdown_handle_ends_the_stream() {
        // The transport's shutdown handle (delegated to the bulk seam) makes the next
        // read return the terminal Ok(0), so the capture thread stops promptly.
        let bulk = FakeBulk::new(vec![9; 64]);
        let mut transport =
            RtlUsbTransport::from_fake(FakeUsb::default(), bulk, usb_regs::TunerKind::R820T);
        let stop = transport.shutdown_handle();
        stop();
        let mut buf = [0u8; 8];
        assert_eq!(transport.read_iq(&mut buf).unwrap(), 0);
    }

    #[test]
    fn backend_is_receive_only_with_the_rtl_identity() {
        let key = RtlKey::Serial("00000001".into());
        let backend = RtlUsbBackend::new(key.clone());
        assert!(matches!(backend.open_playback(48_000), Err(AudioError::Unsupported)));
        assert_eq!(backend.device_id(), DeviceId::Rtl { key });
    }

    /// Linearly upsample 48 kHz audio to the 240 kHz capture rate (integer 5:1).
    fn upsample(audio: &[f32], factor: usize) -> Vec<f32> {
        if audio.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(audio.len() * factor);
        for i in 0..audio.len() * factor {
            let t = i as f32 / factor as f32;
            let base = t.floor() as usize;
            let frac = t - base as f32;
            let a = audio[base];
            let b = *audio.get(base + 1).unwrap_or(&a);
            out.push(a * (1.0 - frac) + b * frac);
        }
        out
    }

    /// FM-modulate `audio` (at `rate`) onto a carrier at `offset_hz`, `dev_hz` peak
    /// deviation, quantized to interleaved unsigned-8-bit IQ as the dongle streams.
    fn fm_modulate_u8(audio: &[f32], rate: f32, offset_hz: f32, dev_hz: f32) -> Vec<u8> {
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(audio.len() * 2);
        for &a in audio {
            let inst = offset_hz + dev_hz * a;
            phase += std::f32::consts::TAU * inst / rate;
            let i = ((phase.cos() * 0.9 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
            let q = ((phase.sin() * 0.9 * 127.5) + 127.5).round().clamp(0.0, 255.0) as u8;
            out.push(i);
            out.push(q);
        }
        out
    }

    #[test]
    fn fake_usb_stream_decodes_aprs_frame() {
        // P2-E CLOSING GATE / milestone: a fake USB dongle streams an FM-modulated
        // AFSK1200 APRS burst as raw u8 IQ; the `RtlUsbBackend` drives the shared
        // capture pipeline over it, demodulates to audio, and the AFSK1200 ensemble
        // recovers the exact AX.25 frame — end to end, no hardware and no rtl_tcp.
        use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
        use omnimodem_dsp::mode::{Demodulator, Modulator};
        use omnimodem_dsp::modes::afsk1200::{Afsk1200Demod, Afsk1200Mod};
        use omnimodem_dsp::types::{Frame, FramePayload};
        use std::sync::mpsc::RecvTimeoutError;

        const CHANNEL_RATE: u32 = 48_000;
        const OFFSET_HZ: f32 = 30_000.0; // signal sits +30 kHz above the dongle center

        let expected = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("K1ABC", 7),
            digipeaters: vec![],
            info: b"!4903.50N/07201.75W-RTL-SDR over native USB".to_vec(),
        };

        // Build the AFSK1200 audio for the frame, upsample 5:1, FM-modulate into IQ.
        let mut modulator = Afsk1200Mod::new();
        let audio = modulator.modulate(&Frame::packet(expected.encode())).unwrap();
        let up = upsample(&audio, (DEFAULT_CAPTURE_RATE / CHANNEL_RATE) as usize);
        let iq = fm_modulate_u8(&up, DEFAULT_CAPTURE_RATE as f32, OFFSET_HZ, DEFAULT_DEVIATION_HZ);

        // A backend whose transport is the fake USB dongle streaming that IQ.
        let opener: TransportOpener = Box::new(move |_control| {
            let transport = RtlUsbTransport::from_fake(
                FakeUsb::default(),
                FakeBulk::new(iq.clone()),
                usb_regs::TunerKind::R820T,
            );
            Ok(Box::new(transport) as Box<dyn SdrTransport>)
        });
        let backend =
            RtlUsbBackend::with_transport_opener(RtlKey::Serial("00000001".into()), opener);
        backend.control().set_offset_hz(OFFSET_HZ); // channel-select the +30 kHz signal

        let cap = backend.open_capture(CHANNEL_RATE).unwrap();
        assert_eq!(cap.sample_rate, CHANNEL_RATE);

        // Drain the burst; the fake stream terminates, so collect until it goes quiet.
        let mut samples: Vec<f32> = Vec::new();
        loop {
            match cap.rx.recv_timeout(Duration::from_secs(2)) {
                Ok(chunk) => samples.extend(chunk.iter().map(|&s| s as f32 / 32768.0)),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(!samples.is_empty(), "no demodulated audio off the USB stream");

        // The AFSK1200 ensemble must recover the exact AX.25 frame with a good FCS.
        let mut demod = Afsk1200Demod::ensemble(9);
        let frames = demod.feed(&samples);
        let decoded = frames
            .iter()
            .find(|f| matches!(&f.payload, FramePayload::Packet(b) if *b == expected.encode()))
            .unwrap_or_else(|| {
                panic!(
                    "no matching AX.25 frame decoded off the USB stream (got {} frame(s))",
                    frames.len()
                )
            });
        assert!(decoded.meta.crc_ok, "decoded frame failed FCS");
    }
}
