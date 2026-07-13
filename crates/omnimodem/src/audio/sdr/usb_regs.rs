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
}
