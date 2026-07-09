//! Waterfall spectrum line: turn one `Stft` frame into a quantized line.
//!
//! Pure, allocation-light functions over an `stft::Stft` output. The pipeline is
//! magnitude → amplitude-normalized dBFS → range-restrict → max-pool → uint8, as
//! laid out in `docs/design/2026-06-23-omnimodem-waterfall-spectrum-api.md` §3.
//! The producer (RX worker) owns the `Stft`; this module owns the math so it can
//! be unit-tested without any audio plumbing.

use rustfft::num_complex::Complex;

/// Server defaults for `ConfigureSpectrum` (0 in the request means "use these").
pub const DEFAULT_BIN_COUNT: u32 = 256;
pub const DEFAULT_FFT_SIZE: u32 = 2048;
pub const DEFAULT_RATE_HZ: u32 = 15;
/// Fixed dynamic range reported in every frame (decision #3 in the design).
pub const DEFAULT_DB_FLOOR: f32 = -120.0;
pub const DEFAULT_DB_CEILING: f32 = 0.0;

const MIN_FFT_SIZE: u32 = 64;
const MAX_FFT_SIZE: u32 = 16_384;

/// Amplitude-normalized dBFS for the positive-frequency half (bins `0..=nfft/2`).
/// `window_sum` is [`crate::frontend::stft::Stft::window_sum`]; with that
/// normalization a full-scale sine peaks at ~0 dBFS, matching the daemon's dbfs
/// convention. `spectrum` is one complex frame of length `nfft`.
pub fn half_spectrum_dbfs(spectrum: &[Complex<f32>], window_sum: f32) -> Vec<f32> {
    let nyq = spectrum.len() / 2; // inclusive Nyquist bin
    let norm = (window_sum / 2.0).max(1e-12);
    spectrum[..=nyq]
        .iter()
        .map(|c| 20.0 * (c.norm() / norm).max(1e-9).log10())
        .collect()
}

/// Two-sided amplitude dBFS for a complex spectrum, reordered so bin[0] is the
/// most-negative frequency (fftshift) and the last bin is just below +Nyquist.
/// `window_sum` normalizes to amplitude, matching `half_spectrum_dbfs`.
pub fn full_spectrum_dbfs(spectrum: &[Complex<f32>], window_sum: f32) -> Vec<f32> {
    let n = spectrum.len();
    let mut out = vec![0.0f32; n];
    for k in 0..n {
        // fftshift: rotate by n/2 so negative freqs (upper FFT half) come first.
        let src = (k + n / 2) % n;
        let amp = spectrum[src].norm() / window_sum;
        out[k] = 20.0 * (amp + 1e-12).log10();
    }
    out
}

/// The fixed rendering geometry for a spectrum stream: which FFT bins map to which
/// output buckets, and the axis labels a client renders from. Built once when a
/// tap is (re)configured, then reused per line.
pub struct SpectrumPlan {
    lo_bin: usize, // first FFT bin in the window (inclusive)
    hi_bin: usize, // last FFT bin in the window (inclusive)
    /// Output buckets, after clamping to the FFT bins available in the window.
    pub bin_count: usize,
    /// Center frequency of output bin 0.
    pub freq_start_hz: f32,
    /// Hz per output bin.
    pub freq_step_hz: f32,
    /// dBFS mapped to uint8 0.
    pub db_floor: f32,
    /// dBFS mapped to uint8 255.
    pub db_ceiling: f32,
}

impl SpectrumPlan {
    /// Build the geometry. `rate` is the *sample rate of the tapped samples* (the
    /// demod native rate), `nfft` the FFT length. `req_bin_count` is clamped down
    /// to the number of FFT bins inside `[freq_lo, freq_hi]` (never up-sampled).
    ///
    /// `freq_lo`/`freq_hi` are clamped to `[0, rate/2]`; equal or inverted edges
    /// snap to a minimal window (one output bin over two FFT bins) rather than
    /// erroring, so a degenerate request still yields a valid line.
    pub fn new(
        nfft: usize,
        rate: f32,
        req_bin_count: usize,
        freq_lo: f32,
        freq_hi: f32,
        db_floor: f32,
        db_ceiling: f32,
    ) -> Self {
        let nyq_bin = nfft / 2;
        let bin_res = rate / nfft as f32; // Hz per FFT bin
        // Clamp the window to [0, Nyquist] so axis labels can't run past the data
        // even when a caller passes raw, unclamped edges directly.
        let nyq_hz = rate / 2.0;
        let freq_lo = freq_lo.clamp(0.0, nyq_hz);
        let freq_hi = freq_hi.clamp(0.0, nyq_hz);
        let lo = freq_lo;
        let hi = freq_hi;
        let mut lo_bin = (lo / bin_res).floor() as usize;
        let mut hi_bin = (hi / bin_res).ceil() as usize;
        lo_bin = lo_bin.min(nyq_bin);
        hi_bin = hi_bin.min(nyq_bin);
        if hi_bin <= lo_bin {
            hi_bin = (lo_bin + 1).min(nyq_bin);
            lo_bin = hi_bin.saturating_sub(1).min(lo_bin);
        }
        let range_bins = hi_bin - lo_bin + 1;
        let bin_count = req_bin_count.clamp(1, range_bins);
        // Label from the requested window (design §3), not the snapped bin edges.
        let span = (freq_hi - freq_lo).max(bin_res);
        let freq_step_hz = span / bin_count as f32;
        let freq_start_hz = freq_lo + freq_step_hz / 2.0;
        SpectrumPlan {
            lo_bin,
            hi_bin,
            bin_count,
            freq_start_hz,
            freq_step_hz,
            db_floor,
            db_ceiling,
        }
    }

    /// Geometry for a full two-sided (RF) spectrum centered at `center_hz` with
    /// total span `rate` Hz. `freq_lo`/`freq_hi` are an optional zoom window in
    /// Hz *relative to center* (pass the full ±rate/2 for no zoom). Renders the
    /// same uint8 bins as `new`, so callers reuse `render` unchanged.
    pub fn new_centered(
        nfft: usize,
        rate: f32,
        center_hz: f32,
        req_bin_count: usize,
        freq_lo: f32,
        freq_hi: f32,
    ) -> Self {
        let step = rate / nfft as f32;
        // Map the relative zoom window to shifted-bin indices (bin 0 == -rate/2).
        let lo = (((freq_lo + rate / 2.0) / step).floor() as isize).clamp(0, nfft as isize - 1);
        let hi = (((freq_hi + rate / 2.0) / step).ceil() as isize).clamp(lo + 1, nfft as isize);
        let lo_bin = lo as usize;
        let hi_bin = hi as usize;
        let span_bins = hi_bin - lo_bin;
        let bin_count = req_bin_count.max(1).min(span_bins);
        let freq_start_hz = center_hz - rate / 2.0 + lo_bin as f32 * step;
        let freq_step_hz = (span_bins as f32 * step) / bin_count as f32;
        SpectrumPlan {
            lo_bin,
            hi_bin,
            bin_count,
            freq_start_hz,
            freq_step_hz,
            db_floor: -120.0,
            db_ceiling: 0.0,
        }
    }

    /// Pool the windowed dBFS bins into `bin_count` uint8 buckets by peak-hold.
    /// `half_dbfs` is the output of [`half_spectrum_dbfs`] (length `nfft/2 + 1`).
    pub fn render(&self, half_dbfs: &[f32]) -> Vec<u8> {
        let range = self.hi_bin - self.lo_bin + 1;
        (0..self.bin_count)
            .map(|b| {
                let start = self.lo_bin + (b * range) / self.bin_count;
                let mut end = self.lo_bin + ((b + 1) * range) / self.bin_count;
                end = end.clamp(start + 1, self.hi_bin + 1).min(half_dbfs.len());
                let peak = half_dbfs[start..end]
                    .iter()
                    .copied()
                    .fold(f32::NEG_INFINITY, f32::max);
                quantize(peak, self.db_floor, self.db_ceiling)
            })
            .collect()
    }
}

/// Resolved, clamped parameters for a spectrum stream at a given native rate.
/// One `resolve` feeds both the RPC echo (clamped params) and the producer (the
/// `Stft` size + hop), so the two never disagree.
pub struct SpectrumSetup {
    pub nfft: usize,
    pub hop: usize,
    /// Achievable lines/sec = `native_rate / hop` (the requested rate is a target;
    /// hop is capped at `nfft`, so very low rates floor out — see design §4).
    pub rate_hz: u32,
    pub plan: SpectrumPlan,
}

impl SpectrumSetup {
    /// Apply defaults (a `0` field means "server default"), round `fft_size` up to
    /// a power of two, derive the hop from the target line rate, and build the
    /// plan. Uses the fixed default dynamic range.
    pub fn resolve(
        native_rate: u32,
        bin_count: u32,
        fft_size: u32,
        rate_hz: u32,
        freq_lo_hz: f32,
        freq_hi_hz: f32,
    ) -> Self {
        let nfft = round_fft_size(fft_size);
        let bin_count = if bin_count == 0 { DEFAULT_BIN_COUNT } else { bin_count };
        let rate = if rate_hz == 0 { DEFAULT_RATE_HZ } else { rate_hz };
        let nyquist = native_rate as f32 / 2.0;
        let lo = freq_lo_hz.clamp(0.0, nyquist);
        let hi = if freq_hi_hz <= 0.0 { nyquist } else { freq_hi_hz.clamp(0.0, nyquist) };
        let hi = if hi <= lo { nyquist } else { hi };

        let hop = (native_rate as f32 / rate as f32).round() as usize;
        let hop = hop.clamp(1, nfft);
        let achievable = (native_rate as f32 / hop as f32).round() as u32;

        let plan = SpectrumPlan::new(
            nfft,
            native_rate as f32,
            bin_count as usize,
            lo,
            hi,
            DEFAULT_DB_FLOOR,
            DEFAULT_DB_CEILING,
        );
        SpectrumSetup { nfft, hop, rate_hz: achievable, plan }
    }
}

fn round_fft_size(fft_size: u32) -> usize {
    let req = if fft_size == 0 { DEFAULT_FFT_SIZE } else { fft_size };
    req.clamp(MIN_FFT_SIZE, MAX_FFT_SIZE).next_power_of_two() as usize
}

/// Map one dBFS value to uint8 over `[floor, ceiling]`.
fn quantize(db: f32, floor: f32, ceiling: f32) -> u8 {
    let span = (ceiling - floor).max(1e-6);
    let t = ((db - floor) / span).clamp(0.0, 1.0);
    (t * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::stft::Stft;
    use std::f32::consts::TAU;

    fn tone_frame(rate: f32, nfft: usize, tone: f32) -> (Vec<Complex<f32>>, f32) {
        let hop = nfft / 2;
        let mut s = Stft::new(nfft, hop);
        let samples: Vec<f32> = (0..nfft * 4).map(|n| (TAU * tone * n as f32 / rate).sin()).collect();
        let frames = s.feed(&samples);
        (frames[1].clone(), s.window_sum())
    }

    #[test]
    fn full_scale_sine_is_about_zero_dbfs() {
        let (frame, wsum) = tone_frame(48_000.0, 2048, 1000.0);
        let half = half_spectrum_dbfs(&frame, wsum);
        let peak = half.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(peak.abs() < 1.0, "full-scale sine peak {peak} dBFS, expected ~0");
    }

    #[test]
    fn tone_lands_in_the_right_output_bucket() {
        let rate = 48_000.0;
        let nfft = 2048;
        let tone = 3000.0;
        let (frame, wsum) = tone_frame(rate, nfft, tone);
        let half = half_spectrum_dbfs(&frame, wsum);
        // Full passband, 256 buckets.
        let plan = SpectrumPlan::new(nfft, rate, 256, 0.0, rate / 2.0, -120.0, 0.0);
        let bins = plan.render(&half);
        let (peak_idx, _) = bins.iter().enumerate().max_by_key(|(_, &v)| v).unwrap();
        let peak_hz = plan.freq_start_hz + peak_idx as f32 * plan.freq_step_hz;
        assert!((peak_hz - tone).abs() < 2.0 * plan.freq_step_hz, "peak {peak_hz} vs {tone}");
        assert_eq!(bins.len(), 256);
    }

    #[test]
    fn bin_count_is_clamped_to_available_fft_bins() {
        // A 0–3 kHz window at 12 kHz native with nfft=2048: bin_res ≈ 5.86 Hz, so
        // ~512 FFT bins are available — far fewer than the 4000 requested.
        let setup = SpectrumSetup::resolve(12_000, 4000, 2048, 15, 0.0, 3000.0);
        assert!((500..=515).contains(&setup.plan.bin_count), "got {}", setup.plan.bin_count);
        assert_eq!(setup.nfft, 2048);
    }

    #[test]
    fn defaults_applied_and_fft_rounded_to_pow2() {
        let setup = SpectrumSetup::resolve(48_000, 0, 1500, 0, 0.0, 0.0);
        assert_eq!(setup.nfft, 2048); // 1500 -> next pow2
        // hop capped at nfft, so 15/s target floors out to ~23/s at 48 kHz.
        assert_eq!(setup.hop, 2048);
        assert_eq!(setup.rate_hz, 23);
        assert_eq!(setup.plan.bin_count, 256);
        // default window is the full passband to Nyquist.
        assert!((setup.plan.freq_step_hz * 256.0 - 24_000.0).abs() < 1.0);
    }

    #[test]
    fn ft8_rate_is_achievable_at_native_12k() {
        // 12 kHz native, hop = 12000/15 = 800 <= 2048, so 15/s is exact.
        let setup = SpectrumSetup::resolve(12_000, 256, 2048, 15, 0.0, 0.0);
        assert_eq!(setup.hop, 800);
        assert_eq!(setup.rate_hz, 15);
    }

    #[test]
    fn out_of_range_window_is_clamped_to_nyquist() {
        // A high edge above Nyquist must be clamped: labels stay within the data
        // (Nyquist = 24 kHz at 48 kHz), instead of running up to the raw 40 kHz.
        let plan = SpectrumPlan::new(2048, 48_000.0, 64, 10_000.0, 40_000.0, -120.0, 0.0);
        let max_label = plan.freq_start_hz + (plan.bin_count as f32 - 1.0) * plan.freq_step_hz;
        assert!(max_label <= 24_000.0, "max label {max_label} exceeds Nyquist 24 kHz");
        assert!(plan.freq_start_hz >= 10_000.0, "low edge should be preserved");
    }

    #[test]
    fn quantize_maps_floor_ceiling_midpoint() {
        assert_eq!(quantize(-130.0, -120.0, 0.0), 0); // below floor
        assert_eq!(quantize(10.0, -120.0, 0.0), 255); // above ceiling
        assert_eq!(quantize(0.0, -120.0, 0.0), 255);
        assert_eq!(quantize(-120.0, -120.0, 0.0), 0);
        assert_eq!(quantize(-60.0, -120.0, 0.0), 128); // midpoint
    }

    #[test]
    fn render_length_matches_clamped_bin_count() {
        let half = vec![-50.0f32; 1025]; // nfft=2048 half-spectrum
        let plan = SpectrumPlan::new(2048, 48_000.0, 100, 0.0, 24_000.0, -120.0, 0.0);
        assert_eq!(plan.render(&half).len(), 100);
        assert!(plan.render(&half).iter().all(|&v| v == quantize(-50.0, -120.0, 0.0)));
    }

    #[test]
    fn full_spectrum_is_fftshifted() {
        use rustfft::num_complex::Complex;
        // Length-8 spectrum with all energy in the DC bin (index 0). After
        // fftshift, DC moves to the center (index nfft/2 == 4).
        let mut spec = vec![Complex::new(0.0f32, 0.0); 8];
        spec[0] = Complex::new(8.0, 0.0);
        let db = full_spectrum_dbfs(&spec, 8.0);
        assert_eq!(db.len(), 8);
        let (peak, _) = db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(peak, 4, "DC should land at the shifted center");
    }

    #[test]
    fn centered_plan_axis_starts_at_minus_nyquist() {
        // 240 kHz span centered at 144.39 MHz → bin[0] at center - 120 kHz. The
        // zoom window is the full band (±rate/2 in Hz), i.e. no zoom.
        let plan =
            SpectrumPlan::new_centered(1024, 240_000.0, 144_390_000.0, 256, -120_000.0, 120_000.0);
        assert!((plan.freq_start_hz - (144_390_000.0 - 120_000.0)).abs() < 1.0);
        assert!(plan.freq_step_hz > 0.0);
    }
}
