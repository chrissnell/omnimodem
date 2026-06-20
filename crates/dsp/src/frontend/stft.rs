//! Overlapped STFT: windowed, hopped real-FFT producing magnitude/complex bins.
//! Powers the waterfall and noncoherent tone detection.

use crate::types::Sample;
use rustfft::{num_complex::Complex, FftPlanner};

pub struct Stft {
    nfft: usize,
    hop: usize,
    window: Vec<f32>,
    buf: Vec<f32>,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Stft {
    pub fn new(nfft: usize, hop: usize) -> Self {
        assert!(nfft > 0 && hop > 0 && hop <= nfft);
        let window: Vec<f32> = (0..nfft)
            .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / nfft as f32).cos())
            .collect();
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(nfft);
        Stft {
            nfft,
            hop,
            window,
            buf: Vec::with_capacity(nfft * 2),
            fft,
            scratch: vec![Complex::new(0.0, 0.0); nfft],
        }
    }

    /// Feed samples; for each complete hop, emit one complex spectrum (len nfft).
    pub fn feed(&mut self, samples: &[Sample]) -> Vec<Vec<Complex<f32>>> {
        self.buf.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.buf.len() >= self.nfft {
            for i in 0..self.nfft {
                self.scratch[i] = Complex::new(self.buf[i] * self.window[i], 0.0);
            }
            self.fft.process(&mut self.scratch);
            out.push(self.scratch.clone());
            self.buf.drain(0..self.hop);
        }
        out
    }

    pub fn nfft(&self) -> usize {
        self.nfft
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    pub fn bin_hz(&self, bin: usize, rate: f32) -> f32 {
        bin as f32 * rate / self.nfft as f32
    }

    /// Sum of the window taps; used to normalize bin magnitudes to amplitude.
    pub fn window_sum(&self) -> f32 {
        self.window.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn dominant_bin_maps_to_tone_frequency() {
        let rate = 12000.0;
        let nfft = 1920;
        let hop = 960;
        let tone = 1500.0;
        let mut s = Stft::new(nfft, hop);
        let samples: Vec<f32> = (0..nfft * 4)
            .map(|n| (TAU * tone * n as f32 / rate).sin())
            .collect();
        let frames = s.feed(&samples);
        assert!(!frames.is_empty());
        let spec = &frames[1]; // skip the partially-windowed first frame edge
        // Find the dominant bin in the positive-frequency half.
        let (peak, _) = spec[..nfft / 2]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm().partial_cmp(&b.1.norm()).unwrap())
            .unwrap();
        let peak_hz = s.bin_hz(peak, rate);
        assert!((peak_hz - tone).abs() <= rate / nfft as f32, "peak {peak_hz} vs {tone}");
    }

    #[test]
    fn hann_window_sum_is_half_nfft() {
        let s = Stft::new(1024, 512);
        // Hann window sums to ~nfft/2.
        assert!((s.window_sum() - 512.0).abs() < 1.0);
    }
}
