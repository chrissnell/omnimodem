//! Complex (IQ) STFT for the wideband RF waterfall. Unlike the real `Stft`,
//! this keeps the imaginary part, so the output is the full two-sided spectrum:
//! bin 0 = DC, bins `1..nfft/2` = positive freqs, bins `nfft/2..nfft` = negative.
//! A Hann window is applied to both rails.

use crate::types::Cplx;
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

pub struct ComplexStft {
    nfft: usize,
    hop: usize,
    window: Vec<f32>,
    buf: Vec<Cplx>,
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    window_sum: f32,
}

impl ComplexStft {
    pub fn new(nfft: usize, hop: usize) -> Self {
        let window: Vec<f32> = (0..nfft)
            .map(|n| {
                0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / nfft as f32).cos()
            })
            .collect();
        let window_sum = window.iter().sum();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(nfft);
        ComplexStft {
            nfft,
            hop,
            window,
            buf: Vec::with_capacity(nfft),
            fft,
            scratch: vec![Complex::new(0.0, 0.0); nfft],
            window_sum,
        }
    }

    pub fn nfft(&self) -> usize {
        self.nfft
    }

    pub fn window_sum(&self) -> f32 {
        self.window_sum
    }

    /// Feed complex samples; emit one full complex spectrum (len `nfft`) per hop.
    pub fn feed(&mut self, samples: &[Cplx]) -> Vec<Vec<Complex<f32>>> {
        let mut out = Vec::new();
        for &s in samples {
            self.buf.push(s);
            if self.buf.len() == self.nfft {
                for i in 0..self.nfft {
                    let w = self.window[i];
                    self.scratch[i] = Complex::new(self.buf[i].re * w, self.buf[i].im * w);
                }
                self.fft.process(&mut self.scratch);
                out.push(self.scratch.clone());
                self.buf.drain(0..self.hop);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_tone_lands_in_a_positive_bin() {
        let nfft = 1024;
        let mut st = ComplexStft::new(nfft, nfft);
        let rate = 240_000.0;
        let f = 30_000.0; // bin = f/rate*nfft = 128
        let iq: Vec<Cplx> = (0..nfft)
            .map(|k| {
                let ph = std::f32::consts::TAU * f * k as f32 / rate;
                Cplx::new(ph.cos(), ph.sin())
            })
            .collect();
        let frames = st.feed(&iq);
        assert_eq!(frames.len(), 1);
        let (peak, _) = frames[0]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm().partial_cmp(&b.1.norm()).unwrap())
            .unwrap();
        assert!((peak as i32 - 128).abs() <= 2, "peak bin {peak}, expected ~128");
    }

    #[test]
    fn negative_tone_lands_in_upper_half() {
        let nfft = 1024;
        let mut st = ComplexStft::new(nfft, nfft);
        let rate = 240_000.0;
        let f = -30_000.0; // negative freq → bin nfft-128 = 896
        let iq: Vec<Cplx> = (0..nfft)
            .map(|k| {
                let ph = std::f32::consts::TAU * f * k as f32 / rate;
                Cplx::new(ph.cos(), ph.sin())
            })
            .collect();
        let frames = st.feed(&iq);
        let (peak, _) = frames[0]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm().partial_cmp(&b.1.norm()).unwrap())
            .unwrap();
        assert!((peak as i32 - 896).abs() <= 2, "peak bin {peak}, expected ~896");
    }
}
