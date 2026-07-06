//! Shared pixel-FSK codec for the fldigi in-band picture sub-protocols
//! (MFSK / THOR / IFKP / FSQ). These modes all switch out of their text state
//! into a raster state and send each 8-bit pixel as a raw frequency deviation
//! (no varicode, no FEC), one frequency held for a fixed number of samples per
//! pixel; the receiver phase-differentiates the carrier back to a frequency and
//! maps it to a byte. Only the header syntax, the pixel↔frequency scaling, the
//! samples-per-pixel, the luma weights, and the colour-plane order differ
//! between families — so the wire math lives here once and each mode
//! parametrises it.
//!
//! Reference: `fldigi/src/{mfsk/mfsk-pic,thor/thor-pic,ifkp/ifkp-pic,fsq/fsq-pic}.cxx`
//! (+ their `*.cxx` hosts), upstream 4.1.23 @ `61b97f413`.
//!
//! Two equivalence classes (Porting Doctrine §3):
//! - **Bit-exact (integer/index domain):** the colour-plane raster ordering and
//!   the MFSK integer luma reduction are pure integer transforms, asserted
//!   byte-for-byte here.
//! - **FP / tolerance domain:** the pixel↔frequency deviation math is audio-
//!   domain FP. The functions below transcribe the reference expressions with
//!   `ref:` cites; their *bit-exact* per-family quantiser KATs come from the T1
//!   golden vectors extracted from the unmodified fldigi dump (see the phase
//!   plan §2), not from self-referential asserts.

use crate::types::{Cplx, Frame, FrameMeta, FramePayload};

/// Pixel ↔ frequency-deviation scaling. Each family picks one; the deviation is
/// relative to the mode's picture carrier `fc`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PixelScale {
    /// MFSK / THOR / IFKP: linear over ±bandwidth/2, centred on 128.
    /// ref: mfsk.cxx:1000-1002, thor.cxx:1334, ifkp.cxx:753.
    Deviation256 { bandwidth_hz: f64 },
    // FSQ's linear scale (`fc − 200 + px·1.5`, fsq.cxx:1432/1206) is added in
    // Wave B (5D) alongside its golden vector, since its RX reference point
    // needs pinning against the reference dump — do not guess it here.
}

impl PixelScale {
    /// TX: pixel value → frequency **deviation** from the picture carrier (Hz).
    /// `reverse` flips the sign (MFSK's `reverse` / `CAP_REV`, mfsk.cxx:1000-1002).
    /// FP/audio domain — never asserted bit-exact.
    pub fn tx_deviation_hz(self, pixel: u8, reverse: bool) -> f64 {
        match self {
            // ref: mfsk.cxx:1000-1002 `bandwidth * (data - 128) / 256.0`.
            PixelScale::Deviation256 { bandwidth_hz } => {
                let dev = bandwidth_hz * (pixel as f64 - 128.0) / 256.0;
                if reverse {
                    -dev
                } else {
                    dev
                }
            }
        }
    }

    /// RX: measured frequency **deviation** from the picture carrier (Hz) → byte.
    /// Truncating clamp to 0..=255, mirroring the reference `(int)CLAMP(...)`.
    /// ref: thor.cxx:974 `byte = pixel*256.0/bandwidth + 128; (int)CLAMP(0,255)`.
    pub fn rx_byte(self, deviation_hz: f64, reverse: bool) -> u8 {
        match self {
            PixelScale::Deviation256 { bandwidth_hz } => {
                let dev = if reverse { -deviation_hz } else { deviation_hz };
                let v = dev * 256.0 / bandwidth_hz + 128.0;
                // `f64 as u8` truncates toward zero and saturates to 0..=255,
                // matching C's `(int)CLAMP(v, 0.0, 255.0)` for in-range inputs.
                v.clamp(0.0, 255.0) as u8
            }
        }
    }
}

/// Luma weights differ by family. MFSK uses an **integer** reduction; the others
/// use the BT.601 floating weights. Kept distinct on purpose (Doctrine §2:
/// transcribe verbatim, do not unify).
///
/// MFSK: `(31·R + 61·G + 8·B) / 100` — pure integer division, bit-exact.
/// ref: mfsk-pic.cxx:244.
pub fn luma_mfsk(r: u8, g: u8, b: u8) -> u8 {
    ((31 * r as u32 + 61 * g as u32 + 8 * b as u32) / 100) as u8
}

/// THOR / IFKP / FSQ: `0.3·R + 0.6·G + 0.1·B`, truncated to an integer.
/// ref: thor.cxx:1329-1331, ifkp.cxx:815-817, fsq.cxx:1426-1428.
/// FP-domain: the exact truncation at boundaries follows the reference op order;
/// its bit-exact KAT comes from the T1 golden vector.
pub fn luma_std(r: u8, g: u8, b: u8) -> u8 {
    (0.3 * r as f64 + 0.6 * g as f64 + 0.1 * b as f64) as u8
}

/// Colour-plane transmit order. Row-major; within each row all of plane A, then
/// plane B, then plane C. MFSK/THOR/IFKP send R→G→B; FSQ sends B→G→R.
/// ref: mfsk-pic.cxx:198-202, thor.cxx:1349-1362, fsq.cxx:1445 (`RGB[]={2,1,0}`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaneOrder {
    /// R, G, B (MFSK, THOR, IFKP).
    Rgb,
    /// B, G, R (FSQ).
    Bgr,
}

impl PlaneOrder {
    /// The source-plane index (0=R,1=G,2=B) sent in transmit-slot `slot` (0..3).
    fn plane_at(self, slot: usize) -> usize {
        match self {
            PlaneOrder::Rgb => slot,       // 0→R, 1→G, 2→B
            PlaneOrder::Bgr => 2 - slot,   // 0→B, 1→G, 2→R
        }
    }
}

/// Build the ordered TX byte stream for a colour image from row-major
/// interleaved RGB (`R,G,B,…`, `rgb.len() == width*rows*3`): for each row, all
/// pixels of the first plane, then the second, then the third, per `order`.
/// Pure integer/index domain — bit-exact. ref: the nested TX loops cited above.
pub fn color_tx_raster(rgb: &[u8], width: usize, order: PlaneOrder) -> Vec<u8> {
    debug_assert_eq!(rgb.len() % (width * 3), 0, "rgb must be whole rows of width*3");
    let rows = rgb.len() / (width * 3);
    let mut out = Vec::with_capacity(rgb.len());
    for row in 0..rows {
        for slot in 0..3 {
            let plane = order.plane_at(slot);
            for col in 0..width {
                out.push(rgb[3 * (col + row * width) + plane]);
            }
        }
    }
    out
}

/// RX inverse: the destination index into a row-major interleaved-RGB buffer for
/// the byte received in colour transmit-slot `rgb_slot` (0..3), column `col`,
/// row `row`. ref: mfsk.cxx:436-445, fsq.cxx:1226-1241 (`RGB[]` mapping).
pub fn rx_pixel_index(order: PlaneOrder, rgb_slot: usize, col: usize, row: usize, width: usize) -> usize {
    order.plane_at(rgb_slot) + 3 * (col + row * width)
}

/// Which luma reduction a family uses when transmitting a colour image as grey.
/// Kept distinct (Doctrine §2): MFSK is an integer reduction, the others BT.601.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LumaKind {
    /// `(31R+61G+8B)/100` (mfsk-pic.cxx:239).
    Mfsk,
    /// `0.3R+0.6G+0.1B` (thor/ifkp/fsq).
    Std,
}

impl LumaKind {
    pub fn luma(self, r: u8, g: u8, b: u8) -> u8 {
        match self {
            LumaKind::Mfsk => luma_mfsk(r, g, b),
            LumaKind::Std => luma_std(r, g, b),
        }
    }
}

/// Mode-agnostic in-band **pixel-FSK picture codec**: the raster is sent as raw
/// carrier-FSK, one frequency per 8-bit pixel held for `spp` samples, with a
/// carrier prologue/epilogue so the receiver settles. The four families
/// (MFSK/THOR/IFKP/FSQ) differ only in their FSK `scale`, `luma` reduction, and
/// colour-plane `order` — supply those and reuse this engine. The in-band header
/// that announces the picture is each mode's own concern (its text codec).
///
/// ref: the `sendpic`/`recvpic` loops — mfsk.cxx:988-1012/424-460,
/// thor.cxx:1324-1362/943-975, ifkp.cxx:807-850/556-617.
#[derive(Debug, Clone, Copy)]
pub struct PictureCodec {
    pub samplerate: f32,
    pub carrier_hz: f32,
    pub reverse: bool,
    pub scale: PixelScale,
    pub luma: LumaKind,
    pub order: PlaneOrder,
    /// `FrameMeta::decoder` label for decoded rasters (e.g. `"mfsk-pic"`).
    pub label: &'static str,
}

/// Carrier lead-in/out (in samples) so the RX down-converter, low-pass, and
/// discriminator settle before/after pixel timing — the role fldigi's
/// `send_prologue`/`send_epilogue` + flush delay play.
fn prologue_samples(spp: usize) -> usize {
    2 * spp
}

impl PictureCodec {
    /// The ordered on-air pixel byte stream: colour → the plane raster in this
    /// family's order; grey → one luma byte per pixel. `rgb` is row-major
    /// interleaved RGB (`rgb.len() == width*height*3`).
    fn pixel_stream(&self, rgb: &[u8], width: usize, color: bool) -> Vec<u8> {
        if color {
            color_tx_raster(rgb, width, self.order)
        } else {
            rgb.chunks_exact(3).map(|p| self.luma.luma(p[0], p[1], p[2])).collect()
        }
    }

    /// **Modulator.** Encode an image as pixel-FSK audio at `spp` samples/pixel.
    pub fn encode(&self, rgb: &[u8], width: usize, height: usize, color: bool, spp: usize) -> Vec<f32> {
        debug_assert_eq!(rgb.len(), width * height * 3);
        let stream = self.pixel_stream(rgb, width, color);
        let rate = self.samplerate;
        let mut osc = crate::frontend::osc::Oscillator::new(self.carrier_hz, rate);
        let prologue = prologue_samples(spp);
        let mut out = Vec::with_capacity(stream.len() * spp + 2 * prologue);
        for _ in 0..prologue {
            out.push(osc.next().0); // carrier lead-in
        }
        for &px in &stream {
            let f = self.carrier_hz + self.scale.tx_deviation_hz(px, self.reverse) as f32;
            osc.set_freq(f, rate);
            for _ in 0..spp {
                out.push(osc.next().0);
            }
        }
        osc.set_freq(self.carrier_hz, rate);
        for _ in 0..prologue {
            out.push(osc.next().0); // carrier lead-out
        }
        out
    }

    /// **Demodulator.** Decode pixel-FSK audio into a `FramePayload::Image` for
    /// the `width`×`height` (`color`) raster at `spp` samples/pixel.
    pub fn decode(&self, audio: &[f32], width: usize, height: usize, color: bool, spp: usize) -> Frame {
        use crate::frontend::fir::{design_lowpass, Fir};
        use crate::frontend::nco::DownConverter;
        let rate = self.samplerate;
        let n_pixels = if color { width * height * 3 } else { width * height };

        // A real input tone leaves an image at −(2fc+dev) that moves with the
        // pixel, so attenuate it across a band with a short linear-phase FIR
        // low-pass (integer group delay, compensated) rather than a point null.
        // NOTE: tuned for the 8 kHz families (MFSK/THOR). At IFKP's 16 kHz with a
        // low carrier the 2fc image crowds the pixel-rate sidebands and this
        // low-pass is not selective enough — the shared demod needs a rate-robust
        // analytic (image-free) front-end before the IFKP/FSQ loopback closes.
        let mut dc = DownConverter::new(self.carrier_hz, rate);
        let base: Vec<Cplx> = audio.iter().map(|&x| dc.push(x)).collect();
        let n = base.len();
        let taps = design_lowpass(9, self.carrier_hz, rate);
        let delay = (taps.len() - 1) / 2;
        let (mut fi, mut fq) = (Fir::new(taps.clone()), Fir::new(taps));
        let smooth: Vec<Cplx> =
            base.iter().map(|z| Cplx::new(fi.push(z.re), fq.push(z.im))).collect();
        let mut inst = vec![0.0f64; n];
        for i in 1..n {
            inst[i] = (smooth[i] * smooth[i - 1].conj()).arg() as f64 * rate as f64
                / std::f64::consts::TAU;
        }

        let prologue = prologue_samples(spp);
        let byte_at = |pixel: usize| -> u8 {
            let lo = prologue + pixel * spp + delay;
            let hi = (lo + spp).min(n);
            if lo >= n {
                return 128;
            }
            // Average the discriminator over the whole pixel span; the residual
            // image and the leading phase-step transient both average down.
            let dev = inst[lo..hi].iter().sum::<f64>() / (hi - lo).max(1) as f64;
            self.scale.rx_byte(dev, self.reverse)
        };

        let (channels, pixels) = if color {
            let mut recon = vec![0u8; n_pixels];
            let mut k = 0usize;
            for row in 0..height {
                for slot in 0..3 {
                    for col in 0..width {
                        recon[rx_pixel_index(self.order, slot, col, row, width)] = byte_at(k);
                        k += 1;
                    }
                }
            }
            (3u8, recon)
        } else {
            (1u8, (0..n_pixels).map(byte_at).collect())
        };

        Frame {
            payload: FramePayload::Image { width: width as u16, channels, pixels },
            meta: FrameMeta { crc_ok: true, decoder: Some(self.label.into()), ..Default::default() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mfsk_integer_luma_is_bit_exact() {
        // (31R + 61G + 8B) / 100, integer division. ref: mfsk-pic.cxx:244.
        assert_eq!(luma_mfsk(0, 0, 0), 0);
        assert_eq!(luma_mfsk(255, 255, 255), 255); // (31+61+8)*255/100 = 255
        assert_eq!(luma_mfsk(100, 200, 50), 157); // (3100+12200+400)/100
        assert_eq!(luma_mfsk(255, 0, 0), 79); // 7905/100
        assert_eq!(luma_mfsk(0, 255, 0), 155); // 15555/100
    }

    #[test]
    fn std_luma_matches_reference_expression() {
        assert_eq!(luma_std(0, 0, 0), 0);
        assert_eq!(luma_std(255, 255, 255), 255);
        // Pure-green: 0.6*200 = 120.
        assert_eq!(luma_std(0, 200, 0), 120);
    }

    #[test]
    fn deviation256_quantiser_boundaries() {
        let s = PixelScale::Deviation256 { bandwidth_hz: 316.0 };
        // Centre pixel → zero deviation; zero deviation → 128.
        assert_eq!(s.tx_deviation_hz(128, false), 0.0);
        assert_eq!(s.rx_byte(0.0, false), 128);
        // Extremes clamp.
        assert_eq!(s.rx_byte(-1e6, false), 0);
        assert_eq!(s.rx_byte(1e6, false), 255);
        // Reverse flips the sign of the deviation.
        assert_eq!(s.tx_deviation_hz(255, true), -s.tx_deviation_hz(255, false));
    }

    #[test]
    fn deviation256_tx_rx_round_trip_reconstructs_pixels() {
        // With an exact (noiseless) deviation, the quantiser recovers the pixel
        // for every value. This pins the TX/RX inverse relationship; the
        // bit-exact vs-fldigi KAT (measured deviations) is the T1 golden vector.
        let s = PixelScale::Deviation256 { bandwidth_hz: 316.0 };
        for px in 0u8..=255 {
            let dev = s.tx_deviation_hz(px, false);
            assert_eq!(s.rx_byte(dev, false), px, "round-trip failed at px={px}");
        }
    }

    #[test]
    fn color_plane_raster_orders_rgb_and_bgr() {
        // One row, two pixels: p0=(R=1,G=2,B=3), p1=(R=4,G=5,B=6).
        let rgb = [1u8, 2, 3, 4, 5, 6];
        // RGB: all R (1,4), all G (2,5), all B (3,6).
        assert_eq!(color_tx_raster(&rgb, 2, PlaneOrder::Rgb), vec![1, 4, 2, 5, 3, 6]);
        // BGR: all B (3,6), all G (2,5), all R (1,4).
        assert_eq!(color_tx_raster(&rgb, 2, PlaneOrder::Bgr), vec![3, 6, 2, 5, 1, 4]);
    }

    #[test]
    fn color_tx_raster_and_rx_index_are_inverse() {
        // Emitting a colour image then walking rx_pixel_index over the same slot
        // order must land each byte back at its source position.
        let width = 3;
        let rows = 2;
        let rgb: Vec<u8> = (0..(width * rows * 3) as u8).collect();
        for order in [PlaneOrder::Rgb, PlaneOrder::Bgr] {
            let tx = color_tx_raster(&rgb, width, order);
            let mut recon = vec![0u8; rgb.len()];
            let mut k = 0;
            for row in 0..rows {
                for slot in 0..3 {
                    for col in 0..width {
                        let dst = rx_pixel_index(order, slot, col, row, width);
                        recon[dst] = tx[k];
                        k += 1;
                    }
                }
            }
            assert_eq!(recon, rgb, "{order:?} raster/rx-index not inverse");
        }
    }
}
