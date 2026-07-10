//! ADS-B (Mode S) pulse-position modulator/demodulator on a magnitude stream.
//!
//! Mode S extended squitter is 1 Mbit/s PPM at 1090 MHz: an 8 µs preamble
//! (pulses at 0.0, 1.0, 3.5, 4.5 µs) followed by 56- or 112-bit frames, each
//! data bit a 0.5 µs pulse in the first (`1`) or second (`0`) half of its
//! microsecond. Both sides work on a magnitude (envelope) stream — the value a
//! receiver recovers as `|I + jQ|` — so the modulator output feeds straight
//! back into the demodulator for loopback self-test.
//!
//! Samples are `f32` at `samples_per_us` samples per microsecond (even; 2 is
//! the dump1090 rate, i.e. omnimodem's 2 MHz `native_rate`). Slicing and
//! preamble correlation are scale-independent (relative comparisons), so the
//! magnitude need not be normalized.

use super::crc;
use super::{
    long_frame_df, DATA_SLOTS_PER_BIT, LONG_FRAME_BITS, PREAMBLE_HIGH_SLOTS, PREAMBLE_LOW_SLOTS,
    PREAMBLE_SLOTS, SHORT_FRAME_BITS,
};

/// A demodulated Mode S frame with its position in the fed stream.
#[derive(Clone, Debug, PartialEq)]
pub struct RawFrame {
    /// Raw message bytes (7 or 14).
    pub bytes: Vec<u8>,
    /// Downlink format (top 5 bits of byte 0).
    pub df: u8,
    /// CRC residual — `0` for a clean extended-squitter frame.
    pub crc_residual: u32,
    /// Sample offset of the preamble start within the scanned buffer.
    pub offset: usize,
}

impl RawFrame {
    pub fn crc_ok(&self) -> bool {
        self.crc_residual == 0
    }
}

/// Builds Mode S PPM magnitude waveforms.
#[derive(Clone, Copy, Debug)]
pub struct PpmModulator {
    samples_per_us: usize,
    high: f32,
    low: f32,
}

impl PpmModulator {
    /// Modulator at `samples_per_us` (must be even) with unit-amplitude pulses.
    pub fn new(samples_per_us: usize) -> Self {
        assert!(
            samples_per_us >= 2 && samples_per_us.is_multiple_of(2),
            "samples_per_us must be even and >= 2"
        );
        Self { samples_per_us, high: 1.0, low: 0.0 }
    }

    fn slot_len(&self) -> usize {
        self.samples_per_us / 2
    }

    fn push_slot(&self, out: &mut Vec<f32>, high: bool) {
        let v = if high { self.high } else { self.low };
        for _ in 0..self.slot_len() {
            out.push(v);
        }
    }

    /// Modulate a raw Mode S frame (7 or 14 bytes, parity already present) into
    /// a magnitude waveform: preamble + PPM-encoded bits.
    pub fn modulate(&self, frame: &[u8]) -> Vec<f32> {
        let nbits = frame.len() * 8;
        let total_slots = PREAMBLE_SLOTS + nbits * DATA_SLOTS_PER_BIT;
        let mut out = Vec::with_capacity(total_slots * self.slot_len());

        for slot in 0..PREAMBLE_SLOTS {
            self.push_slot(&mut out, PREAMBLE_HIGH_SLOTS.contains(&slot));
        }
        for &byte in frame {
            for bit in (0..8).rev() {
                let one = (byte >> bit) & 1 == 1;
                self.push_slot(&mut out, one);
                self.push_slot(&mut out, !one);
            }
        }
        out
    }

    /// Modulate with `lead`/`trail` microseconds of silence so a demodulator
    /// scanning a longer buffer has quiet margins to lock onto.
    pub fn modulate_padded(&self, frame: &[u8], lead_us: usize, trail_us: usize) -> Vec<f32> {
        let mut out = vec![self.low; lead_us * self.samples_per_us];
        out.extend(self.modulate(frame));
        out.extend(std::iter::repeat_n(self.low, trail_us * self.samples_per_us));
        out
    }
}

/// Decodes Mode S PPM from a magnitude stream.
#[derive(Clone, Copy, Debug)]
pub struct PpmDemodulator {
    samples_per_us: usize,
    /// Accept only frames whose extended-squitter CRC is zero. When false,
    /// frames are returned regardless of residual (DF0/4/5/11/20/21 overlay
    /// their parity with an address).
    pub require_crc: bool,
}

impl PpmDemodulator {
    pub fn new(samples_per_us: usize) -> Self {
        assert!(
            samples_per_us >= 2 && samples_per_us.is_multiple_of(2),
            "samples_per_us must be even and >= 2"
        );
        Self { samples_per_us, require_crc: true }
    }

    fn slot_len(&self) -> usize {
        self.samples_per_us / 2
    }

    /// Mean magnitude over the half-microsecond slot beginning at `sample`.
    fn slot_mag(&self, mag: &[f32], sample: usize) -> f32 {
        let len = self.slot_len();
        let mut sum = 0.0f32;
        for k in 0..len {
            sum += mag[sample + k];
        }
        sum / len as f32
    }

    /// Preamble match at offset `i`: every high slot exceeds every low slot.
    ///
    /// This strict correlation is tuned for a clean modulator/offline stream —
    /// on a noisy off-air envelope a single elevated low slot rejects the
    /// match. A field-grade detector would add a per-pulse threshold and
    /// margin; that is deferred to the live rtl_tcp wiring.
    fn preamble_ok(&self, mag: &[f32], i: usize) -> bool {
        let slot = self.slot_len();
        let mut high_min = f32::MAX;
        for &s in &PREAMBLE_HIGH_SLOTS {
            high_min = high_min.min(self.slot_mag(mag, i + s * slot));
        }
        if high_min <= 0.0 {
            return false;
        }
        let mut low_max = 0.0f32;
        for &s in &PREAMBLE_LOW_SLOTS {
            low_max = low_max.max(self.slot_mag(mag, i + s * slot));
        }
        high_min > low_max
    }

    /// Slice `nbits` PPM data bits following the preamble at offset `i`.
    fn slice_bits(&self, mag: &[f32], i: usize, nbits: usize) -> Vec<u8> {
        let slot = self.slot_len();
        let data_start = i + PREAMBLE_SLOTS * slot;
        let mut bytes = vec![0u8; nbits / 8];
        for j in 0..nbits {
            let base = data_start + j * DATA_SLOTS_PER_BIT * slot;
            let first = self.slot_mag(mag, base);
            let second = self.slot_mag(mag, base + slot);
            if first > second {
                bytes[j / 8] |= 1 << (7 - (j % 8));
            }
        }
        bytes
    }

    /// Samples spanned by a full frame (preamble + `nbits` data).
    fn frame_samples(&self, nbits: usize) -> usize {
        (PREAMBLE_SLOTS + nbits * DATA_SLOTS_PER_BIT) * self.slot_len()
    }

    /// Scan `mag` for frames. Returns the accepted frames plus the number of
    /// samples consumed; `mag[consumed..]` is the unscanned tail the caller
    /// retains for the next call. When `flush` is false the scan stops one long
    /// frame short of the end so a frame straddling the buffer boundary is
    /// deferred rather than truncated.
    pub fn scan(&self, mag: &[f32], flush: bool) -> (Vec<RawFrame>, usize) {
        let long_span = self.frame_samples(LONG_FRAME_BITS);
        let short_span = self.frame_samples(SHORT_FRAME_BITS);
        let limit = if flush { short_span } else { long_span };
        let mut frames = Vec::new();
        let mut i = 0usize;
        while i + limit <= mag.len() {
            if !self.preamble_ok(mag, i) {
                i += 1;
                continue;
            }
            // Read the DF from a short slice, then extend to a long frame when
            // the format demands it (and the buffer allows).
            let short_bytes = self.slice_bits(mag, i, SHORT_FRAME_BITS);
            let df = short_bytes[0] >> 3;
            let nbits = if long_frame_df(df) && i + long_span <= mag.len() {
                LONG_FRAME_BITS
            } else {
                SHORT_FRAME_BITS
            };
            let bytes = if nbits == SHORT_FRAME_BITS {
                short_bytes
            } else {
                self.slice_bits(mag, i, LONG_FRAME_BITS)
            };

            let residual = crc::checksum(&bytes);
            if !self.require_crc || residual == 0 {
                frames.push(RawFrame { bytes, df, crc_residual: residual, offset: i });
                i += self.frame_samples(nbits);
            } else {
                i += 1;
            }
        }
        (frames, i)
    }
}
