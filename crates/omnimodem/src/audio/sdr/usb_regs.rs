//! RTL2832U register constants and the byte-level init/resampler sequences,
//! transcribed from librtlsdr (`librtlsdr.c`) — the byte-level source of truth.
//!
//! The sequences are modelled as ordered [`RegWrite`] data rather than emitted
//! inline so they can be asserted, register for register, against the reference in
//! unit tests before any hardware is present. [`usb`](super::usb) turns each
//! [`RegWrite`] into the matching control transfer (`write_reg` for block
//! registers, `demod_write_reg` for the demodulator core).

use super::usb::Block;
use super::AudioError;

/// RTL2832U reference crystal frequency, Hz. librtlsdr `DEF_RTL_XTAL_FREQ`.
pub(crate) const RTL_XTAL_FREQ: u32 = 28_800_000;

// --- USB block registers (addressed through `Block::Usb`) ---
/// librtlsdr `USB_SYSCTL`.
const USB_SYSCTL: u16 = 0x2000;
/// librtlsdr `USB_EPA_MAXPKT`.
const USB_EPA_MAXPKT: u16 = 0x2158;
/// librtlsdr `USB_EPA_CTL`.
const USB_EPA_CTL: u16 = 0x2148;

// --- System block registers (addressed through `Block::Sys`) ---
/// librtlsdr `DEMOD_CTL`.
const DEMOD_CTL: u16 = 0x3000;
/// librtlsdr `DEMOD_CTL_1`.
const DEMOD_CTL_1: u16 = 0x300b;

/// Default IF FIR-filter coefficients: eight 8-bit taps followed by eight 12-bit
/// taps. librtlsdr `fir_default`.
const FIR_DEFAULT: [i32; 16] = [
    -54, -36, -41, -40, -32, -14, 14, 53, // int8
    101, 156, 215, 273, 327, 372, 404, 421, // int12
];

/// One register write in an RTL2832U init sequence. The RTL2832U reaches its
/// block registers and its demodulator core through two different control-transfer
/// encodings, so the sequence records which one each write uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegWrite {
    /// A generic block register (`write_reg(block, addr, val, len)`).
    Block { block: Block, addr: u16, val: u16, len: u8 },
    /// A demodulator register (`demod_write_reg(page, addr, val, len)`).
    Demod { page: u8, addr: u16, val: u16, len: u8 },
}

/// The full RTL2832U baseband bring-up, transcribed one-to-one from librtlsdr
/// `rtlsdr_init_baseband`: USB block init, demod power-on, a demod soft reset,
/// spectrum-inversion/DDC clears, the FIR filter, then the SDR-mode/AGC/Zero-IF
/// datapath configuration.
pub(crate) fn baseband_init() -> Vec<RegWrite> {
    use RegWrite::{Block as B, Demod as D};

    let mut ops = vec![
        // initialize USB
        B { block: Block::Usb, addr: USB_SYSCTL, val: 0x09, len: 1 },
        B { block: Block::Usb, addr: USB_EPA_MAXPKT, val: 0x0002, len: 2 },
        B { block: Block::Usb, addr: USB_EPA_CTL, val: 0x1002, len: 2 },
        // power on demod
        B { block: Block::Sys, addr: DEMOD_CTL_1, val: 0x22, len: 1 },
        B { block: Block::Sys, addr: DEMOD_CTL, val: 0xe8, len: 1 },
        // reset demod (bit 3, soft_rst)
        D { page: 1, addr: 0x01, val: 0x14, len: 1 },
        D { page: 1, addr: 0x01, val: 0x10, len: 1 },
        // disable spectrum inversion and adjacent channel rejection
        D { page: 1, addr: 0x15, val: 0x00, len: 1 },
        D { page: 1, addr: 0x16, val: 0x0000, len: 2 },
    ];

    // clear both DDC shift and IF frequency registers
    for i in 0..6u16 {
        ops.push(D { page: 1, addr: 0x16 + i, val: 0x00, len: 1 });
    }

    // set up the IF FIR filter (rtlsdr_set_fir)
    for (i, byte) in fir_registers().iter().enumerate() {
        ops.push(D { page: 1, addr: 0x1c + i as u16, val: u16::from(*byte), len: 1 });
    }

    ops.extend([
        // enable SDR mode, disable DAGC (bit 5)
        D { page: 0, addr: 0x19, val: 0x05, len: 1 },
        // init FSM state-holding register
        D { page: 1, addr: 0x93, val: 0xf0, len: 1 },
        D { page: 1, addr: 0x94, val: 0x0f, len: 1 },
        // disable AGC (en_dagc, bit 0)
        D { page: 1, addr: 0x11, val: 0x00, len: 1 },
        // disable RF and IF AGC loop
        D { page: 1, addr: 0x04, val: 0x00, len: 1 },
        // disable PID filter (enable_PID = 0)
        D { page: 0, addr: 0x61, val: 0x60, len: 1 },
        // opt_adc_iq = 0, default ADC_I/ADC_Q datapath
        D { page: 0, addr: 0x06, val: 0x80, len: 1 },
        // Zero-IF mode: en_bbin, en_dc_est, en_iq_comp, en_iq_est
        D { page: 1, addr: 0xb1, val: 0x1b, len: 1 },
        // disable 4.096 MHz clock output on pin TP_CK0
        D { page: 0, addr: 0x0d, val: 0x83, len: 1 },
    ]);

    ops
}

/// Pack [`FIR_DEFAULT`] into the 20 filter-register bytes exactly as librtlsdr
/// `rtlsdr_set_fir`: the first eight taps are one byte each, the next eight are
/// 12-bit values packed two per three bytes.
fn fir_registers() -> [u8; 20] {
    let mut fir = [0u8; 20];
    for i in 0..8 {
        fir[i] = FIR_DEFAULT[i] as u8;
    }
    let mut i = 0;
    while i < 8 {
        let val0 = FIR_DEFAULT[8 + i];
        let val1 = FIR_DEFAULT[8 + i + 1];
        let base = 8 + i * 3 / 2;
        fir[base] = (val0 >> 4) as u8;
        fir[base + 1] = ((val0 << 4) | ((val1 >> 8) & 0x0f)) as u8;
        fir[base + 2] = val1 as u8;
        i += 2;
    }
    fir
}

/// The RTL2832U resampler ratio for `samp_rate`, or [`AudioError::UnsupportedSampleRate`]
/// when the rate falls outside the resampler's usable window. librtlsdr
/// `rtlsdr_set_sample_rate`: reject `<= 225 kHz`, `> 3.2 MHz`, and the
/// `(300 kHz, 900 kHz]` gap, then compute `(rtl_xtal * 2^22) / samp_rate` in
/// floating point and mask to the ratio's valid bits.
///
/// The upper-gap bound is `<= 900_000`, matching `librtlsdr/librtlsdr` master
/// (this project's reference). The older `osmocom/rtl-sdr` fork uses `< 900_000`,
/// i.e. it accepts exactly 900 kHz; that lone value is the only behavioral
/// difference between the two, kept here on the librtlsdr side deliberately.
pub(crate) fn resamp_ratio(samp_rate: u32) -> Result<u32, AudioError> {
    if samp_rate <= 225_000
        || samp_rate > 3_200_000
        || (samp_rate > 300_000 && samp_rate <= 900_000)
    {
        return Err(AudioError::UnsupportedSampleRate(samp_rate));
    }

    let two_pow_22 = f64::from(1u32 << 22);
    let ratio = (f64::from(RTL_XTAL_FREQ) * two_pow_22 / f64::from(samp_rate)) as u32;
    Ok(ratio & 0x0fff_fffc)
}

/// The demod register writes that program the resampler ratio for `samp_rate`,
/// transcribed from librtlsdr `rtlsdr_set_sample_rate`: the high and low ratio
/// halves into `0x9f`/`0xa1`, then a demod soft reset to latch them. Frequency
/// (ppm) correction is a separate setter applied by `apply_hardware`.
pub(crate) fn sample_rate_writes(samp_rate: u32) -> Result<Vec<RegWrite>, AudioError> {
    let ratio = resamp_ratio(samp_rate)?;
    Ok(vec![
        RegWrite::Demod { page: 1, addr: 0x9f, val: (ratio >> 16) as u16, len: 2 },
        RegWrite::Demod { page: 1, addr: 0xa1, val: (ratio & 0xffff) as u16, len: 2 },
        // reset demod (bit 3, soft_rst) to latch the new ratio
        RegWrite::Demod { page: 1, addr: 0x01, val: 0x14, len: 1 },
        RegWrite::Demod { page: 1, addr: 0x01, val: 0x10, len: 1 },
    ])
}

// ===========================================================================
// R820T / R828D tuner (transcribed from librtlsdr `tuner_r82xx.c` + `librtlsdr.c`)
// ===========================================================================

/// I2C bus address of an R820T tuner. librtlsdr `R820T_I2C_ADDR`.
pub(crate) const R820T_I2C_ADDR: u8 = 0x34;
/// I2C bus address of an R828D tuner. librtlsdr `R828D_I2C_ADDR`.
pub(crate) const R828D_I2C_ADDR: u8 = 0x74;
/// Register read for the tuner-presence probe. librtlsdr `R82XX_CHECK_ADDR`.
pub(crate) const R82XX_CHECK_ADDR: u8 = 0x00;
/// The chip-id byte an R82xx returns at [`R82XX_CHECK_ADDR`]. librtlsdr
/// `R82XX_CHECK_VAL`.
const R82XX_CHECK_VAL: u8 = 0x69;
/// R82xx intermediate frequency (Hz): the tuner mixes RF down to this IF, so the
/// programmed LO sits `R82XX_IF_FREQ` above the requested RF. librtlsdr
/// `R82XX_IF_FREQ`.
pub(crate) const R82XX_IF_FREQ: u32 = 3_570_000;
/// First register the R82xx register shadow covers. librtlsdr `REG_SHADOW_START`.
pub(crate) const REG_SHADOW_START: usize = 5;
/// R82xx register-file size. librtlsdr `NUM_REGS`.
pub(crate) const NUM_REGS: usize = 32;

/// The two R82xx tuner variants omnimodem drives natively. Any other tuner chip is
/// rejected at probe with [`AudioError::UnsupportedTuner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TunerKind {
    R820T,
    R828D,
}

impl TunerKind {
    /// The tuner's I2C bus address.
    pub(crate) fn i2c_addr(self) -> u8 {
        match self {
            TunerKind::R820T => R820T_I2C_ADDR,
            TunerKind::R828D => R828D_I2C_ADDR,
        }
    }

    /// `rtl_tcp` / `rtlsdr.h` tuner-type code, so [`TunerCaps`](super::TunerCaps)
    /// reuse the same per-tuner tables the rtl_tcp path publishes.
    pub(crate) fn type_code(self) -> u32 {
        match self {
            TunerKind::R820T => 5,
            TunerKind::R828D => 6,
        }
    }

    /// `vco_power_ref`: the R828D's VCO runs at half the R820T's reference, which
    /// changes both the div-num fine-tune pivot and the `nint` ceiling. librtlsdr
    /// `r82xx_set_pll` (`vco_power_ref = 1` for `CHIP_R828D`, else `2`).
    fn vco_power_ref(self) -> u8 {
        match self {
            TunerKind::R820T => 2,
            TunerKind::R828D => 1,
        }
    }
}

/// Map an i2c presence probe — the chip-id byte read at a candidate tuner address —
/// to a [`TunerKind`]. `None` when the byte is not the R82xx chip id (the address
/// held a different chip, or nothing answered). librtlsdr's tuner-detect walks
/// `R820T_I2C_ADDR` then `R828D_I2C_ADDR`, matching `R82XX_CHECK_VAL` at each.
pub(crate) fn tuner_kind_from_probe(i2c_addr: u8, chip_id: u8) -> Option<TunerKind> {
    if chip_id != R82XX_CHECK_VAL {
        return None;
    }
    match i2c_addr {
        R820T_I2C_ADDR => Some(TunerKind::R820T),
        R828D_I2C_ADDR => Some(TunerKind::R828D),
        _ => None,
    }
}

/// The R82xx power-on register image, regs `0x05..=0x1f`, transcribed verbatim from
/// librtlsdr `r82xx_init_array` (with `DEFAULT_IF_VGA_VAL = 11` folded into reg
/// `0x0c` and `VER_NUM = 49` into reg `0x13`). Written in one I2C burst at init and
/// used to seed the register shadow the masked writes read-modify-write against.
pub(crate) fn r82xx_init_array() -> [u8; NUM_REGS - REG_SHADOW_START] {
    [
        0x80, // 0x05
        0x13, // 0x06
        0x70, // 0x07
        0xc0, // 0x08
        0x40, // 0x09
        0xdb, // 0x0a
        0x6b, // 0x0b
        0xeb, // 0x0c  (0xe0 | DEFAULT_IF_VGA_VAL=11)
        0x53, // 0x0d
        0x75, // 0x0e
        0x68, // 0x0f
        0x6c, // 0x10
        0xbb, // 0x11
        0x80, // 0x12
        0x31, // 0x13  (VER_NUM=49 & 0x3f)
        0x0f, // 0x14
        0x00, // 0x15
        0xc0, // 0x16
        0x30, // 0x17
        0x48, // 0x18
        0xec, // 0x19
        0x60, // 0x1a
        0x00, // 0x1b
        0x24, // 0x1c
        0xdd, // 0x1d
        0x0e, // 0x1e
        0x40, // 0x1f
    ]
}

/// One R82xx tuner register write. The R82xx is programmed over I2C register by
/// register, and most writes are read-modify-write against the driver's register
/// shadow, so the op records whether it replaces the whole byte or only a masked
/// field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TunerOp {
    /// Replace the whole register (`r82xx_write_reg`).
    Write { reg: u8, val: u8 },
    /// Replace only the `mask` bits with `val & mask` (`r82xx_write_reg_mask`).
    Mask { reg: u8, val: u8, mask: u8 },
}

/// One RF band's mux/tracking-filter programming. librtlsdr `struct
/// r82xx_freq_range`; only the fields that vary with the fixed
/// `XTAL_HIGH_CAP_0P` xtal setting omnimodem uses are kept.
struct FreqRange {
    /// Band start (MHz).
    freq_mhz: u32,
    /// Open-drain (`R23[3]`).
    open_d: u8,
    /// RF mux / poly-mux (`R26`).
    rf_mux_ploy: u8,
    /// Tracking-filter band (`R27`).
    tf_c: u8,
}

/// librtlsdr `freq_ranges` (`tuner_r82xx.c`) — the tracking-filter/mux band table.
/// The last entry covers everything above its start frequency.
const FREQ_RANGES: &[FreqRange] = &[
    FreqRange { freq_mhz: 0, open_d: 0x08, rf_mux_ploy: 0x02, tf_c: 0xdf },
    FreqRange { freq_mhz: 50, open_d: 0x08, rf_mux_ploy: 0x02, tf_c: 0xbe },
    FreqRange { freq_mhz: 55, open_d: 0x08, rf_mux_ploy: 0x02, tf_c: 0x8b },
    FreqRange { freq_mhz: 60, open_d: 0x08, rf_mux_ploy: 0x02, tf_c: 0x7b },
    FreqRange { freq_mhz: 65, open_d: 0x08, rf_mux_ploy: 0x02, tf_c: 0x69 },
    FreqRange { freq_mhz: 70, open_d: 0x08, rf_mux_ploy: 0x02, tf_c: 0x58 },
    FreqRange { freq_mhz: 75, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x44 },
    FreqRange { freq_mhz: 80, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x44 },
    FreqRange { freq_mhz: 90, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x34 },
    FreqRange { freq_mhz: 100, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x34 },
    FreqRange { freq_mhz: 110, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x24 },
    FreqRange { freq_mhz: 120, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x24 },
    FreqRange { freq_mhz: 140, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x14 },
    FreqRange { freq_mhz: 180, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x13 },
    FreqRange { freq_mhz: 220, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x13 },
    FreqRange { freq_mhz: 250, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x11 },
    FreqRange { freq_mhz: 280, open_d: 0x00, rf_mux_ploy: 0x02, tf_c: 0x00 },
    FreqRange { freq_mhz: 310, open_d: 0x00, rf_mux_ploy: 0x41, tf_c: 0x00 },
    FreqRange { freq_mhz: 450, open_d: 0x00, rf_mux_ploy: 0x41, tf_c: 0x00 },
    FreqRange { freq_mhz: 588, open_d: 0x00, rf_mux_ploy: 0x40, tf_c: 0x00 },
    FreqRange { freq_mhz: 650, open_d: 0x00, rf_mux_ploy: 0x40, tf_c: 0x00 },
];

/// The mux / tracking-filter writes for LO `lo_hz`, transcribed from librtlsdr
/// `r82xx_set_mux`. The xtal-cap write is the `XTAL_HIGH_CAP_0P` branch omnimodem
/// pins at init, which resolves to `0x00` for every band.
pub(crate) fn r82xx_mux_writes(lo_hz: u32) -> Vec<TunerOp> {
    let freq_mhz = lo_hz / 1_000_000;
    let mut range = &FREQ_RANGES[0];
    for r in FREQ_RANGES {
        if freq_mhz < r.freq_mhz {
            break;
        }
        range = r;
    }
    vec![
        TunerOp::Mask { reg: 0x17, val: range.open_d, mask: 0x08 },
        TunerOp::Mask { reg: 0x1a, val: range.rf_mux_ploy, mask: 0xc3 },
        TunerOp::Write { reg: 0x1b, val: range.tf_c },
        // XTAL CAP & Drive — XTAL_HIGH_CAP_0P: xtal_cap0p (0x00) | 0x00.
        TunerOp::Mask { reg: 0x10, val: 0x00, mask: 0x0b },
    ]
}

/// The solved R82xx PLL parameters for one LO frequency. librtlsdr `r82xx_set_pll`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct R82xxPll {
    /// Mixer divider (2..=64) whose product with the LO lands in the VCO window.
    pub mix_div: u8,
    /// `R16[7:5]` divider selector (log2(mix_div)-1, fine-tune-adjusted).
    pub div_num: u8,
    /// Integer part of `vco / (2·ref)`.
    pub nint: u8,
    /// Fractional part, in 1/65536ths (the sigma-delta value).
    pub sdm: u16,
    /// `R20[5:0]` low nibble (`nint` sub-index).
    pub ni: u8,
    /// `R20[7:6]` (`nint` remainder).
    pub si: u8,
}

/// Solve the R82xx PLL for LO `lo_hz`, transcribed from librtlsdr `r82xx_set_pll`
/// (the default `vco_algo == 0` path). `vco_fine_tune` is `(R4[5:4])` read back from
/// the tuner; it nudges `div_num` by ±1 versus the chip's `vco_power_ref`. Returns
/// [`AudioError::TunerFreqRange`] when no divider lands the VCO in range or `nint`
/// overflows the PLL.
pub(crate) fn r82xx_pll(
    lo_hz: u32,
    tuner: TunerKind,
    vco_fine_tune: u8,
) -> Result<R82xxPll, AudioError> {
    const VCO_MIN_KHZ: u32 = 1_770_000;
    const VCO_MAX_KHZ: u32 = VCO_MIN_KHZ * 2;
    let pll_ref = RTL_XTAL_FREQ;
    let freq_khz = (lo_hz + 500) / 1000;

    // Pick the mixer divider that lands freq·mix_div inside the VCO window, and the
    // R16 selector (div_num) that halves down to it.
    let mut mix_div: u32 = 2;
    let mut div_num: i32 = 0;
    let mut found = false;
    while mix_div <= 64 {
        if freq_khz * mix_div >= VCO_MIN_KHZ && freq_khz * mix_div < VCO_MAX_KHZ {
            let mut div_buf = mix_div;
            while div_buf > 2 {
                div_buf >>= 1;
                div_num += 1;
            }
            found = true;
            break;
        }
        mix_div <<= 1;
    }
    if !found {
        return Err(AudioError::TunerFreqRange(lo_hz));
    }

    let vco_power_ref = tuner.vco_power_ref();
    if vco_fine_tune > vco_power_ref {
        div_num -= 1;
    } else if vco_fine_tune < vco_power_ref {
        div_num += 1;
    }

    // vco_div = round(65536 · vco / (2·ref)); nint + sdm/65536 = vco / (2·ref).
    let vco_freq = u64::from(lo_hz) * u64::from(mix_div);
    let vco_div = (u64::from(pll_ref) + 65536 * vco_freq) / (2 * u64::from(pll_ref));
    let nint = (vco_div / 65536) as u32;
    let sdm = (vco_div % 65536) as u16;

    if nint > u32::from(128 / vco_power_ref) - 1 {
        return Err(AudioError::TunerFreqRange(lo_hz));
    }

    let ni = ((nint - 13) / 4) as u8;
    let si = (nint - 4 * u32::from(ni) - 13) as u8;

    Ok(R82xxPll {
        mix_div: mix_div as u8,
        div_num: (div_num as u8) & 0x07,
        nint: nint as u8,
        sdm,
        ni,
        si,
    })
}

/// The register writes that program a solved [`R82xxPll`], transcribed from
/// librtlsdr `r82xx_set_pll`. `vco_fine_tune` is folded into `pll.div_num` already;
/// dither stays enabled (the default). The interleaved lock-status reads and the
/// VCO-current retry are runtime concerns and live in the transport, not here.
pub(crate) fn r82xx_pll_writes(pll: &R82xxPll) -> Vec<TunerOp> {
    // pw_sdm: power down the sigma-delta only when the fraction is exactly zero.
    let pw_sdm = if pll.sdm == 0 { 0x08 } else { 0x00 };
    vec![
        // refdiv2 off
        TunerOp::Mask { reg: 0x10, val: 0x00, mask: 0x10 },
        // pll autotune = 128 kHz (fastest) while acquiring
        TunerOp::Mask { reg: 0x1a, val: 0x00, mask: 0x0c },
        // VCO current = min (default 0x80)
        TunerOp::Mask { reg: 0x12, val: 0x80, mask: 0xe0 },
        // divider selector
        TunerOp::Mask { reg: 0x10, val: pll.div_num << 5, mask: 0xe0 },
        // nint: ni in [5:0], si in [7:6]
        TunerOp::Write { reg: 0x14, val: pll.ni + (pll.si << 6) },
        // pw_sdm (dither bit 0x10 left clear = dither enabled)
        TunerOp::Mask { reg: 0x12, val: pw_sdm, mask: 0x18 },
        // sdm high then low byte
        TunerOp::Write { reg: 0x16, val: (pll.sdm >> 8) as u8 },
        TunerOp::Write { reg: 0x15, val: (pll.sdm & 0xff) as u8 },
        // pll autotune = 8 kHz (settle)
        TunerOp::Mask { reg: 0x1a, val: 0x08, mask: 0x08 },
    ]
}

/// Per-stage LNA gain increments (tenths of dB). librtlsdr `r82xx_lna_gain_steps`.
const R82XX_LNA_GAIN_STEPS: [i32; 16] =
    [0, 9, 13, 40, 38, 13, 31, 22, 26, 31, 26, 14, 19, 5, 35, 13];
/// Per-stage mixer gain increments (tenths of dB). librtlsdr `r82xx_mixer_gain_steps`.
const R82XX_MIXER_GAIN_STEPS: [i32; 16] =
    [0, 5, 10, 10, 19, 9, 10, 25, 17, 10, 8, 16, 13, 6, 3, -8];

/// The LNA and mixer register indices realising `gain_tenths_db` of manual RF gain,
/// transcribed from librtlsdr `r82xx_get_rf_gain_index`: walk the LNA and mixer
/// step tables alternately, accumulating until the target is met. The reachable sums
/// are exactly the R82xx gain table [`tuner_gains_db`](super::tuner_gains_db)
/// exposes, so the gRPC snap and this index search agree.
pub(crate) fn r82xx_gain_indices(gain_tenths_db: i32) -> (u8, u8) {
    let mut total = 0i32;
    let mut lna_index = 0usize;
    let mut mix_index = 0usize;
    for _ in 0..15 {
        if total >= gain_tenths_db {
            break;
        }
        lna_index += 1;
        total += R82XX_LNA_GAIN_STEPS[lna_index];
        if total >= gain_tenths_db {
            break;
        }
        mix_index += 1;
        total += R82XX_MIXER_GAIN_STEPS[mix_index];
    }
    (lna_index as u8, mix_index as u8)
}

/// The register writes that select manual RF gain, transcribed from librtlsdr
/// `r82xx_set_gain` (manual branch): switch LNA and mixer AGC off, then load the
/// solved LNA/mixer indices. VGA is left at the init-array default. `gain_tenths_db`
/// is snapped to the R82xx table before the index search so the tuner lands on a
/// table value.
pub(crate) fn r82xx_gain_manual_writes(gain_tenths_db: i32) -> Vec<TunerOp> {
    let snapped = snap_gain_tenths(gain_tenths_db);
    let (lna, mix) = r82xx_gain_indices(snapped);
    vec![
        // LNA auto off (manual)
        TunerOp::Mask { reg: 0x05, val: 0x10, mask: 0x10 },
        // Mixer auto off (manual)
        TunerOp::Mask { reg: 0x07, val: 0x00, mask: 0x10 },
        // set LNA gain index
        TunerOp::Mask { reg: 0x05, val: lna, mask: 0x0f },
        // set mixer gain index
        TunerOp::Mask { reg: 0x07, val: mix, mask: 0x0f },
    ]
}

/// The register writes that hand RF gain back to the tuner AGC, transcribed from
/// librtlsdr `r82xx_set_gain` (automatic branch): LNA and mixer AGC on.
pub(crate) fn r82xx_gain_auto_writes() -> Vec<TunerOp> {
    vec![
        // LNA auto on (AGC)
        TunerOp::Mask { reg: 0x05, val: 0x00, mask: 0x10 },
        // Mixer auto on (AGC)
        TunerOp::Mask { reg: 0x07, val: 0x10, mask: 0x10 },
    ]
}

/// Snap a tenths-of-dB gain request to the nearest R82xx table entry, reusing the
/// shared [`snap_gain`](super::snap_gain) / [`tuner_gains_db`](super::tuner_gains_db)
/// used by the gRPC gain setter, so the native path and the rtl_tcp path snap
/// identically. R820T and R828D share one table (type code 5).
fn snap_gain_tenths(gain_tenths_db: i32) -> i32 {
    let table = super::tuner_gains_db(TunerKind::R820T.type_code());
    let snapped_db = super::snap_gain(&table, gain_tenths_db as f32 / 10.0);
    (snapped_db * 10.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fir_default_packs_like_librtlsdr() {
        // The int8 taps map straight to two's-complement bytes; the int12 taps are
        // packed two per three bytes. Values hand-computed from librtlsdr's
        // fir_default table.
        assert_eq!(
            fir_registers(),
            [
                0xca, 0xdc, 0xd7, 0xd8, 0xe0, 0xf2, 0x0e, 0x35, // int8 taps
                0x06, 0x50, 0x9c, // 101, 156
                0x0d, 0x71, 0x11, // 215, 273
                0x14, 0x71, 0x74, // 327, 372
                0x19, 0x41, 0xa5, // 404, 421
            ]
        );
    }

    #[test]
    fn baseband_init_matches_reference_sequence() {
        use RegWrite::{Block as B, Demod as D};
        let ops = baseband_init();

        // Head: USB block init + demod power-on + soft reset + inversion/DDC clears.
        assert_eq!(
            &ops[..9],
            &[
                B { block: Block::Usb, addr: 0x2000, val: 0x09, len: 1 },
                B { block: Block::Usb, addr: 0x2158, val: 0x0002, len: 2 },
                B { block: Block::Usb, addr: 0x2148, val: 0x1002, len: 2 },
                B { block: Block::Sys, addr: 0x300b, val: 0x22, len: 1 },
                B { block: Block::Sys, addr: 0x3000, val: 0xe8, len: 1 },
                D { page: 1, addr: 0x01, val: 0x14, len: 1 },
                D { page: 1, addr: 0x01, val: 0x10, len: 1 },
                D { page: 1, addr: 0x15, val: 0x00, len: 1 },
                D { page: 1, addr: 0x16, val: 0x0000, len: 2 },
            ]
        );

        // The six-register DDC/IF clear loop (0x16..=0x1b), one byte each.
        assert_eq!(
            &ops[9..15],
            &[
                D { page: 1, addr: 0x16, val: 0x00, len: 1 },
                D { page: 1, addr: 0x17, val: 0x00, len: 1 },
                D { page: 1, addr: 0x18, val: 0x00, len: 1 },
                D { page: 1, addr: 0x19, val: 0x00, len: 1 },
                D { page: 1, addr: 0x1a, val: 0x00, len: 1 },
                D { page: 1, addr: 0x1b, val: 0x00, len: 1 },
            ]
        );

        // Then the 20 FIR bytes to 0x1c..=0x2f.
        assert_eq!(ops[15], D { page: 1, addr: 0x1c, val: 0xca, len: 1 });
        assert_eq!(ops[34], D { page: 1, addr: 0x2f, val: 0xa5, len: 1 });

        // Tail: SDR mode / AGC / Zero-IF datapath configuration.
        assert_eq!(
            &ops[35..],
            &[
                D { page: 0, addr: 0x19, val: 0x05, len: 1 },
                D { page: 1, addr: 0x93, val: 0xf0, len: 1 },
                D { page: 1, addr: 0x94, val: 0x0f, len: 1 },
                D { page: 1, addr: 0x11, val: 0x00, len: 1 },
                D { page: 1, addr: 0x04, val: 0x00, len: 1 },
                D { page: 0, addr: 0x61, val: 0x60, len: 1 },
                D { page: 0, addr: 0x06, val: 0x80, len: 1 },
                D { page: 1, addr: 0xb1, val: 0x1b, len: 1 },
                D { page: 0, addr: 0x0d, val: 0x83, len: 1 },
            ]
        );
    }

    #[test]
    fn resamp_ratio_matches_reference_values() {
        // (28_800_000 * 2^22) / rate, masked with 0x0ffffffc.
        // 2.4 MHz → 0x0300_0000, 240 kHz → 0x0e00_0000 (both exact divisors).
        assert_eq!(resamp_ratio(2_400_000).unwrap(), 0x0300_0000);
        assert_eq!(resamp_ratio(240_000).unwrap(), 0x0e00_0000);
    }

    #[test]
    fn sample_rate_writes_program_ratio_and_reset() {
        use RegWrite::Demod as D;

        assert_eq!(
            sample_rate_writes(2_400_000).unwrap(),
            vec![
                D { page: 1, addr: 0x9f, val: 0x0300, len: 2 },
                D { page: 1, addr: 0xa1, val: 0x0000, len: 2 },
                D { page: 1, addr: 0x01, val: 0x14, len: 1 },
                D { page: 1, addr: 0x01, val: 0x10, len: 1 },
            ]
        );

        assert_eq!(
            sample_rate_writes(240_000).unwrap(),
            vec![
                D { page: 1, addr: 0x9f, val: 0x0e00, len: 2 },
                D { page: 1, addr: 0xa1, val: 0x0000, len: 2 },
                D { page: 1, addr: 0x01, val: 0x14, len: 1 },
                D { page: 1, addr: 0x01, val: 0x10, len: 1 },
            ]
        );
    }

    #[test]
    fn resamp_ratio_rejects_out_of_window_rates() {
        // 900_000 is rejected per librtlsdr/librtlsdr master (`<= 900000`); the
        // osmocom fork would accept it. See resamp_ratio's doc comment.
        for bad in [200_000, 300_001, 900_000, 3_200_001] {
            assert!(matches!(
                resamp_ratio(bad),
                Err(AudioError::UnsupportedSampleRate(r)) if r == bad
            ));
        }
        // Just inside each valid edge.
        assert!(resamp_ratio(225_001).is_ok());
        assert!(resamp_ratio(300_000).is_ok());
        assert!(resamp_ratio(900_001).is_ok());
        assert!(resamp_ratio(3_200_000).is_ok());
    }

    #[test]
    fn tuner_probe_id_maps_addr_and_chip_id() {
        // The R82xx chip id (0x69) at each known address selects the variant.
        assert_eq!(tuner_kind_from_probe(0x34, 0x69), Some(TunerKind::R820T));
        assert_eq!(tuner_kind_from_probe(0x74, 0x69), Some(TunerKind::R828D));
        // Wrong chip id → no match (a different chip answered, or nothing did).
        assert_eq!(tuner_kind_from_probe(0x34, 0x00), None);
        assert_eq!(tuner_kind_from_probe(0x74, 0x68), None);
        // Right id, unknown address → no match.
        assert_eq!(tuner_kind_from_probe(0x12, 0x69), None);
    }

    #[test]
    fn init_array_is_the_reference_image() {
        let arr = r82xx_init_array();
        assert_eq!(arr.len(), 27); // regs 0x05..=0x1f
        assert_eq!(arr[0], 0x80); // reg 0x05
        assert_eq!(arr[0x0c - 0x05], 0xeb); // reg 0x0c = 0xe0 | DEFAULT_IF_VGA_VAL(11)
        assert_eq!(arr[0x13 - 0x05], 0x31); // reg 0x13 = VER_NUM(49) & 0x3f
        assert_eq!(arr[0x1f - 0x05], 0x40); // reg 0x1f
    }

    #[test]
    fn pll_divider_math_at_144_39_mhz() {
        // RF 144.39 MHz → LO = RF + IF(3.57 MHz) = 147.96 MHz. With vco_fine_tune at
        // the R820T's vco_power_ref (2), div_num takes no ±1 adjustment. Values
        // hand-derived from librtlsdr r82xx_set_pll (default vco_algo path):
        //   mix_div=16, div_num=3, vco=2367.36 MHz, nint=41, sdm=6554, ni=7, si=0.
        let lo = 144_390_000 + R82XX_IF_FREQ;
        let pll = r82xx_pll(lo, TunerKind::R820T, 2).unwrap();
        assert_eq!(
            pll,
            R82xxPll { mix_div: 16, div_num: 3, nint: 41, sdm: 6554, ni: 7, si: 0 }
        );

        // The emitted register writes carry those values.
        let ops = r82xx_pll_writes(&pll);
        assert!(ops.contains(&TunerOp::Mask { reg: 0x10, val: 3 << 5, mask: 0xe0 }));
        assert!(ops.contains(&TunerOp::Write { reg: 0x14, val: 7 })); // ni + (si<<6)
        assert!(ops.contains(&TunerOp::Write { reg: 0x16, val: 0x19 })); // 6554 >> 8
        assert!(ops.contains(&TunerOp::Write { reg: 0x15, val: 0x9a })); // 6554 & 0xff
        // sdm != 0 → sigma-delta stays powered (pw_sdm bit clear).
        assert!(ops.contains(&TunerOp::Mask { reg: 0x12, val: 0x00, mask: 0x18 }));
    }

    #[test]
    fn pll_fine_tune_nudges_div_num() {
        let lo = 144_390_000 + R82XX_IF_FREQ;
        // fine_tune below the pivot bumps div_num up, above bumps it down.
        assert_eq!(r82xx_pll(lo, TunerKind::R820T, 0).unwrap().div_num, 4);
        assert_eq!(r82xx_pll(lo, TunerKind::R820T, 3).unwrap().div_num, 2);
    }

    #[test]
    fn pll_rejects_unreachable_lo() {
        // Far below the lowest divider's VCO window → no solution.
        assert!(matches!(
            r82xx_pll(1_000_000, TunerKind::R820T, 2),
            Err(AudioError::TunerFreqRange(_))
        ));
    }

    #[test]
    fn mux_band_selection_tracks_lo() {
        // 147.96 MHz → the 140 MHz band (open_d high, tf_c 0x14).
        let ops = r82xx_mux_writes(147_960_000);
        assert_eq!(ops[0], TunerOp::Mask { reg: 0x17, val: 0x00, mask: 0x08 });
        assert_eq!(ops[1], TunerOp::Mask { reg: 0x1a, val: 0x02, mask: 0xc3 });
        assert_eq!(ops[2], TunerOp::Write { reg: 0x1b, val: 0x14 });
        assert_eq!(ops[3], TunerOp::Mask { reg: 0x10, val: 0x00, mask: 0x0b });

        // Above the last band start (650 MHz) clamps to the final entry.
        let hi = r82xx_mux_writes(900_000_000);
        assert_eq!(hi[1], TunerOp::Mask { reg: 0x1a, val: 0x40, mask: 0xc3 });
    }

    #[test]
    fn gain_indices_snap_to_the_r82xx_table() {
        // 19.7 dB (197 tenths) is a table entry → lna=6, mixer=5 (hand-traced from
        // the alternating LNA/mixer accumulation).
        assert_eq!(r82xx_gain_indices(197), (6, 5));
        // Zero gain → both indices at the bottom.
        assert_eq!(r82xx_gain_indices(0), (0, 0));

        // Manual writes snap an off-table request (200 → 197) to the nearest entry
        // and load its indices; auto writes just re-enable the AGC.
        let manual = r82xx_gain_manual_writes(200);
        assert!(manual.contains(&TunerOp::Mask { reg: 0x05, val: 6, mask: 0x0f }));
        assert!(manual.contains(&TunerOp::Mask { reg: 0x07, val: 5, mask: 0x0f }));
        assert!(manual.contains(&TunerOp::Mask { reg: 0x05, val: 0x10, mask: 0x10 }));

        let auto = r82xx_gain_auto_writes();
        assert_eq!(auto[0], TunerOp::Mask { reg: 0x05, val: 0x00, mask: 0x10 });
        assert_eq!(auto[1], TunerOp::Mask { reg: 0x07, val: 0x10, mask: 0x10 });
    }
}
