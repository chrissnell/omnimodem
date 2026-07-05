//! Hellschreiber (Feld Hell) family: on/off-keyed (or FSK) column-scan facsimile.
//!
//! Port of fldigi's Feld Hell modem (`fldigi/src/feld/feld.cxx`, upstream 4.1.23
//! @ 61b97f413). Hell is a *facsimile* mode: text is painted as a stream of 14-row
//! pixel columns from a bitmap font (`framing::hellfont`), never decoded to
//! characters on the wire, so the RX output is a raster (`FramePayload::Image`),
//! not text. Each pixel row of a column is one on/off symbol sent at the pixel
//! rate `txpixrate = 14 * feldcolumnrate` (feld.cxx:146-259, `restart`); a
//! character is a leading null column, its glyph columns, and a trailing null
//! column (feld.cxx:633-659, `tx_char`).
//!
//! Bit-exact vs fldigi is the *pixel-column raster* (`hellfont::on_air_columns`,
//! already KAT-gated). The **audio** that carries it is NOT bit-exact (Doctrine
//! §3): fldigi's `send_symbol` nco/`OnShape`/`OffShape`/`ModulateXmtr` path is
//! entangled with its FLTK/modem runtime and float op-ordering. The gate here is a
//! loopback whose decoded raster columns reproduce the reference glyph columns,
//! plus (deferred) an `#[ignore]` cross-decode against the fldigi binary. Like the
//! `dominoex` demod, RX assumes pixel alignment to the fed buffer — column sync /
//! AFC and live incremental raster streaming arrive with the typed `Image` wire
//! format (Phase 10 T7).
//!
//! Submode parameters. ref: feld.cxx:153-229.
//! | submode  | feldcolumnrate | keying |
//! |----------|----------------|--------|
//! | FELDHELL | 17.5           | AM on/off |
//! | SLOWHELL | 2.1875         | AM on/off |
//! | HELLX5   | 87.5           | AM on/off |
//! | HELLX9   | 157.5          | AM on/off |
//! | HELL80   | 35             | 2-FSK, ±bandwidth/2 (=±150) |
//!
//! fldigi shapes the FELDHELL/SLOWHELL AM edges with a raised cosine
//! (`OnShape`/`OffShape`, feld.cxx:717-741, selectable via `HellPulseFast`);
//! HELLX5/X9 key hard. That edge shaping is a spectral/audio refinement in the
//! FP domain (Doctrine §3) — it does not change the bit-exact pixel raster — so
//! this port keys AM hard for all submodes and leaves the raised-cosine envelope
//! as an on-air-splatter refinement for the cross-decode gate. The RX detects
//! per-pixel envelope power, which is invariant to that choice.

use crate::framing::hellfont::{on_air_columns, COLUMN_ROWS, DEFAULT_XMT_WIDTH};
use crate::frontend::nco::DownConverter;
use crate::frontend::osc::Oscillator;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Cplx, Frame, FrameMeta, FramePayload, Sample};

/// Fixed working sample rate for every Hell submode. ref: feld.h:37 (`FeldSampleRate`).
const SAMPLE_RATE: u32 = 8000;

/// How a submode keys the carrier. ref: feld.cxx:586-604 (`send_symbol`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Keying {
    /// Amplitude on/off keying (FELDHELL/SLOWHELL/HELLX5/HELLX9). fldigi shapes
    /// FELDHELL/SLOWHELL edges; we key hard — see the module doc.
    Am,
    /// 2-FSK: an on pixel shifts the tone `-shift_hz`, an off pixel `+shift_hz`
    /// (HELL80). ref: feld.cxx:586-587.
    Fsk { shift_hz: f32 },
}

/// The Feld Hell submodes ported here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HellVariant {
    FeldHell,
    SlowHell,
    HellX5,
    HellX9,
    Hell80,
}

impl HellVariant {
    /// fldigi's `feldcolumnrate`. ref: feld.cxx:154-219.
    fn column_rate(self) -> f32 {
        use HellVariant::*;
        match self {
            FeldHell => 17.5,
            SlowHell => 2.1875,
            HellX5 => 87.5,
            HellX9 => 157.5,
            Hell80 => 35.0,
        }
    }

    fn keying(self) -> Keying {
        use HellVariant::*;
        match self {
            FeldHell | SlowHell | HellX5 | HellX9 => Keying::Am,
            // hell_bandwidth = 300 (feld.cxx:224); tone shift = bandwidth/2.
            Hell80 => Keying::Fsk { shift_hz: 150.0 },
        }
    }

    /// Pixel (row-symbol) rate: `TxColumnLen * feldcolumnrate`. ref: feld.cxx:156.
    fn pixel_rate(self) -> f32 {
        COLUMN_ROWS as f32 * self.column_rate()
    }

    /// Occupied bandwidth (Hz) for the mode descriptor. AM ≈ pixel rate
    /// (feld.cxx:159); FSK is the fixed 300 Hz shift channel (feld.cxx:224).
    pub fn bandwidth(self) -> f32 {
        match self.keying() {
            Keying::Fsk { shift_hz } => 2.0 * shift_hz,
            _ => self.pixel_rate(),
        }
    }

    /// Baud (symbol rate) = `0.5 * txpixrate`. ref: feld.cxx:256.
    pub fn baud(self) -> f32 {
        0.5 * self.pixel_rate()
    }

    pub fn samplerate(self) -> u32 {
        SAMPLE_RATE
    }

    pub fn from_label(s: &str) -> Option<HellVariant> {
        use HellVariant::*;
        Some(match s {
            "feldhell" => FeldHell,
            "slowhell" => SlowHell,
            "hellx5" => HellX5,
            "hellx9" => HellX9,
            "hell80" => Hell80,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        use HellVariant::*;
        match self {
            FeldHell => "feldhell",
            SlowHell => "slowhell",
            HellX5 => "hellx5",
            HellX9 => "hellx9",
            Hell80 => "hell80",
        }
    }

    /// Every ported submode, for table-driven tests, the registry and the TUI.
    pub fn all() -> &'static [HellVariant] {
        use HellVariant::*;
        &[FeldHell, SlowHell, HellX5, HellX9, Hell80]
    }
}

/// The flat pixel-symbol stream for a column raster: each 14-bit column expands to
/// 14 on/off symbols, row 0 (LSB) first — the order fldigi's `tx_char` feeds
/// `send_symbol`. ref: feld.cxx:647-652.
fn column_pixels(cols: &[u16]) -> Vec<bool> {
    let mut px = Vec::with_capacity(cols.len() * COLUMN_ROWS);
    for &c in cols {
        for row in 0..COLUMN_ROWS {
            px.push((c >> row) & 1 == 1);
        }
    }
    px
}

fn caps(v: HellVariant) -> ModeCaps {
    ModeCaps {
        native_rate: v.samplerate(),
        bandwidth_hz: v.bandwidth(),
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

/// Pixel index of output sample `n` given the pixel rate: `floor(n * pixrate /
/// samplerate)`. TX and RX share this map so the loopback is pixel-aligned.
fn pixel_of_sample(n: usize, inc: f64) -> usize {
    (n as f64 * inc) as usize
}

/// Total output samples needed to carry `n_pixels`.
fn total_samples(n_pixels: usize, inc: f64) -> usize {
    if n_pixels == 0 {
        return 0;
    }
    // Smallest N with pixel_of_sample(N-1) == n_pixels-1 and pixel_of_sample(N) ==
    // n_pixels: ceil(n_pixels / inc).
    (n_pixels as f64 / inc).ceil() as usize
}

pub struct HellMod {
    v: HellVariant,
    center_hz: f32,
}

impl HellMod {
    pub fn new(v: HellVariant, center_hz: f32) -> Self {
        HellMod { v, center_hz }
    }
}

impl Modulator for HellMod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("hell needs text")),
        };
        let cols = on_air_columns(text, DEFAULT_XMT_WIDTH);
        let pixels = column_pixels(&cols);
        let rate = self.v.samplerate() as f32;
        let inc = self.v.pixel_rate() as f64 / rate as f64;
        let total = total_samples(pixels.len(), inc);
        let keying = self.v.keying();

        let mut osc = Oscillator::new(self.center_hz, rate);
        let mut out = Vec::with_capacity(total);

        for n in 0..total {
            let p = pixel_of_sample(n, inc);
            let bit = pixels.get(p).copied().unwrap_or(false);

            let sample = match keying {
                Keying::Fsk { shift_hz } => {
                    // On pixel -> low tone, off pixel -> high tone (feld.cxx:586-587).
                    let f = self.center_hz + if bit { -shift_hz } else { shift_hz };
                    osc.set_freq(f, rate);
                    let (c, _) = osc.next();
                    c
                }
                Keying::Am => {
                    let (c, _) = osc.next();
                    if bit {
                        c
                    } else {
                        0.0
                    }
                }
            };
            out.push(sample);
        }
        Ok(out)
    }
}

pub struct HellDemod {
    v: HellVariant,
    center_hz: f32,
    buf: Vec<Sample>,
}

impl HellDemod {
    pub fn new(v: HellVariant, center_hz: f32) -> Self {
        HellDemod { v, center_hz, buf: Vec::new() }
    }

    /// Decode the buffered transmission into a raster. Down-converts to baseband,
    /// recovers a per-pixel metric (AM envelope or FSK instantaneous frequency),
    /// bins samples to pixels via the shared `pixel_of_sample` map, thresholds,
    /// and assembles 14-pixel columns into a `FramePayload::Image`.
    ///
    /// The image is 14 px wide (`COLUMN_ROWS`); each successive 14-byte row is one
    /// on-air column, `gray[row*14 + r]` = pixel row `r` of that column (0 or 255).
    /// This "column stream" layout is what the typed `Image` wire format (T7)
    /// appends to incrementally.
    fn decode_raster(&self) -> Option<Frame> {
        let n = self.buf.len();
        if n == 0 {
            return None;
        }
        let rate = self.v.samplerate() as f32;
        let inc = self.v.pixel_rate() as f64 / rate as f64;

        // Down-convert to complex baseband (removes the carrier phase, so the
        // per-sample metric is delay-free — no smoothing filter to shift the
        // pixel grid).
        let mut dc = DownConverter::new(self.center_hz, rate);
        let mut base: Vec<Cplx> = Vec::with_capacity(n);
        for &x in &self.buf {
            base.push(dc.push(x));
        }

        // Pixel sample spans: sample `s` belongs to pixel `pixel_of_sample(s)`.
        // Count of pixels carried = index of the last pixel + 1.
        let n_pixels = pixel_of_sample(n - 1, inc) + 1;
        let n_cols = n_pixels / COLUMN_ROWS;
        if n_cols == 0 {
            return None;
        }
        let n_pixels = n_cols * COLUMN_ROWS;

        let mut spans: Vec<(usize, usize)> = vec![(usize::MAX, 0); n_pixels];
        for s in 0..n {
            let p = pixel_of_sample(s, inc);
            if p < n_pixels {
                let e = &mut spans[p];
                if s < e.0 {
                    e.0 = s;
                }
                if s + 1 > e.1 {
                    e.1 = s + 1;
                }
            }
        }
        let span = |lo: usize, hi: usize| -> (usize, usize) {
            if lo == usize::MAX {
                (0, 0)
            } else {
                (lo, hi)
            }
        };

        // Per-pixel metric and threshold.
        //
        // AM: envelope power `Σ|z|²` over the pixel's whole span (phase-independent;
        // the residual 2·fc image averages out). On where it exceeds half the peak.
        // Averaging over the whole span recovers the raster exactly for every AM
        // submode in loopback — including HELLX9, which is only ~3.6 samples/pixel at
        // 8 kHz (a visual facsimile rate; fldigi RX-resamples it sub-pixel too).
        //
        // FSK: instantaneous frequency `arg(z[n]·conj(z[n-1]))` (a short boxcar
        // first cleans the discriminator), averaged over the pixel's central half;
        // on where it is negative (the low tone). ref: feld.cxx:312-349 (`FSKH_rx`).
        let pixels: Vec<bool> = match self.v.keying() {
            Keying::Am => {
                let power: Vec<f32> = spans
                    .iter()
                    .map(|&(lo, hi)| {
                        let (lo, hi) = span(lo, hi);
                        let len = (hi - lo).max(1) as f32;
                        base[lo..hi].iter().map(|z| z.norm_sqr()).sum::<f32>() / len
                    })
                    .collect();
                let peak = power.iter().cloned().fold(0.0f32, f32::max);
                let thr = 0.5 * peak;
                power.iter().map(|&m| m > thr).collect()
            }
            Keying::Fsk { .. } => {
                let lc = ((rate / self.center_hz).round() as usize).clamp(2, 32);
                let smooth = boxcar(&base, lc);
                let mut freq = vec![0.0f32; n];
                for i in 1..n {
                    freq[i] = (smooth[i] * smooth[i - 1].conj()).arg();
                }
                spans
                    .iter()
                    .map(|&(lo, hi)| {
                        let (lo, hi) = span(lo, hi);
                        let q = (hi - lo) / 4;
                        let (a, b) = (lo + q, (hi - q).max(lo + q + 1).min(hi));
                        let mean = if b <= a {
                            freq.get(lo).copied().unwrap_or(0.0)
                        } else {
                            freq[a..b].iter().sum::<f32>() / (b - a) as f32
                        };
                        mean < 0.0
                    })
                    .collect()
            }
        };

        let mut gray = vec![0u8; n_pixels];
        for (i, &on) in pixels.iter().enumerate() {
            gray[i] = if on { 255 } else { 0 };
        }
        Some(Frame {
            payload: FramePayload::Image { width: COLUMN_ROWS as u16, gray },
            meta: FrameMeta { crc_ok: true, decoder: Some("hell".into()), ..Default::default() },
        })
    }
}

/// Complex boxcar (moving average) of length `len` (causal). Length 1 is identity.
fn boxcar(x: &[Cplx], len: usize) -> Vec<Cplx> {
    if len <= 1 {
        return x.to_vec();
    }
    let mut out = Vec::with_capacity(x.len());
    let mut acc = Cplx::new(0.0, 0.0);
    for i in 0..x.len() {
        acc += x[i];
        if i >= len {
            acc -= x[i - len];
        }
        let d = (i + 1).min(len) as f32;
        out.push(acc / d);
    }
    out
}

impl Demodulator for HellDemod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        // Facsimile RX finalizes the raster at end-of-transmission (flush); live
        // incremental streaming arrives with the typed Image wire format (T7).
        self.buf.extend_from_slice(samples);
        Vec::new()
    }

    fn flush(&mut self) -> Vec<Frame> {
        let out = self.decode_raster().into_iter().collect();
        self.buf.clear();
        out
    }

    fn reset(&mut self) {
        self.buf.clear();
    }
}

/// Reconstruct the 14-bit column values from a Hell raster `Image` payload (the
/// inverse of `column_pixels`): each 14-byte row is one column, byte `r` (0/255)
/// -> bit `r`. Used by the loopback KAT and available to raster consumers.
pub fn image_columns(width: u16, gray: &[u8]) -> Vec<u16> {
    assert_eq!(width as usize, COLUMN_ROWS, "hell raster is 14 rows per column");
    gray
        .chunks_exact(COLUMN_ROWS)
        .map(|row| {
            let mut c = 0u16;
            for (r, &g) in row.iter().enumerate() {
                if g > 127 {
                    c |= 1 << r;
                }
            }
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_round_trip() {
        for &v in HellVariant::all() {
            assert_eq!(HellVariant::from_label(v.label()), Some(v));
        }
        assert_eq!(HellVariant::from_label("hell9000"), None);
        assert_eq!(HellVariant::all().len(), 5);
    }

    #[test]
    fn params_match_reference() {
        // ref: feld.cxx column rates and derived baud/pixrate.
        assert!((HellVariant::FeldHell.pixel_rate() - 245.0).abs() < 1e-3);
        assert!((HellVariant::FeldHell.baud() - 122.5).abs() < 1e-3);
        assert!((HellVariant::HellX9.pixel_rate() - 2205.0).abs() < 1e-3);
        assert_eq!(HellVariant::Hell80.bandwidth() as i32, 300);
        for &v in HellVariant::all() {
            assert_eq!(v.samplerate(), 8000);
        }
    }

    // ---- loopback gate: TX audio -> RX raster reproduces the on-air columns ----

    fn loopback_columns(v: HellVariant, msg: &str) -> Vec<u16> {
        let mut tx = HellMod::new(v, 1500.0);
        let audio = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = HellDemod::new(v, 1500.0);
        assert!(rx.feed(&audio).is_empty());
        let frames = rx.flush();
        assert_eq!(frames.len(), 1, "{} should emit one raster", v.label());
        match &frames[0].payload {
            FramePayload::Image { width, gray } => image_columns(*width, gray),
            _ => panic!("expected Image payload"),
        }
    }

    #[test]
    fn loopback_recovers_raster_all_submodes() {
        let msg = "CQ DE K1ABC";
        let want = on_air_columns(msg, DEFAULT_XMT_WIDTH);
        for &v in HellVariant::all() {
            let got = loopback_columns(v, msg);
            // RX may drop the final partial pixel; compare the common prefix and
            // require it to cover the whole message raster.
            assert!(got.len() >= want.len(), "{}: got {} cols, want {}", v.label(), got.len(), want.len());
            assert_eq!(&got[..want.len()], &want[..], "submode {}", v.label());
        }
    }

    #[test]
    fn loopback_recovers_glyph_raster_with_punctuation() {
        let msg = "DE W1AW 73!";
        let want = on_air_columns(msg, DEFAULT_XMT_WIDTH);
        let got = loopback_columns(HellVariant::FeldHell, msg);
        assert_eq!(&got[..want.len()], &want[..]);
    }

    #[test]
    fn image_columns_inverts_column_pixels() {
        let cols = vec![0u16, 4088, 0, 7360];
        let px = column_pixels(&cols);
        let gray: Vec<u8> = px.iter().map(|&b| if b { 255 } else { 0 }).collect();
        assert_eq!(image_columns(COLUMN_ROWS as u16, &gray), cols);
    }

    #[test]
    fn blank_message_produces_all_off_raster_without_panicking() {
        // A space is five null columns: an all-off raster. The AM threshold sees
        // peak == 0, so nothing crosses it — no division/normalisation blow-up.
        for &v in HellVariant::all() {
            let got = loopback_columns(v, " ");
            assert!(got.iter().all(|&c| c == 0), "{} blank raster must be all-off", v.label());
            assert_eq!(got.len(), on_air_columns(" ", DEFAULT_XMT_WIDTH).len());
        }
    }

    #[test]
    fn empty_input_yields_no_frame() {
        // No text, no samples: flush emits nothing rather than an empty raster.
        let mut tx = HellMod::new(HellVariant::FeldHell, 1500.0);
        assert!(tx.modulate(&Frame::text("")).unwrap().is_empty());
        let mut rx = HellDemod::new(HellVariant::FeldHell, 1500.0);
        assert!(rx.feed(&[]).is_empty());
        assert!(rx.flush().is_empty());
    }
}


