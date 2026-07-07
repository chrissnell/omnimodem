//! Offset-MSK / OQPSK front-end for WSJT-X MSK144 (meteor scatter), ported from
//! `wsjtx/lib/`. MSK144 sends a 72 ms, 864-sample frame (144 half-symbols at
//! 2000 baud, 6 samples each) of continuous-phase MSK: two tones 1000 Hz apart
//! (`freq ± baud/4`) shaped by a half-sine pulse. This module holds the reusable
//! MSK primitives — the half-sine pulse, the continuous-phase MSK modulator, the
//! FFT analytic-signal filter, and the frequency shifter — that the MSK144 mode
//! assembly (`modes::msk144`) builds its framer/sync/matched-filter on.
//!
//! ref: wsjtx/lib/{msk144sim.f90, analytic.f90, tweak1.f90, msk144sync.f90}
//! (WSJTX/wsjtx @ ccdfaf3c1c109010d15399674ce278167cfde848).

use crate::types::{Cplx, Sample};
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

/// MSK144 works at a fixed 12 kHz audio rate. ref: msk144sim.f90 (fs=12000).
pub const MSK_FS: f32 = 12_000.0;
/// Symbol (baud) rate. ref: msk144sim.f90 (baud=2000).
pub const MSK_BAUD: f32 = 2_000.0;
/// Samples per half-symbol. ref: msk144sim.f90 (nsps=6).
pub const MSK_NSPS: usize = 6;
/// Half-symbols (tones) per frame. ref: genmsk_128_90.f90 (i4tone(144)).
pub const MSK_NSYM: usize = 144;
/// Samples per message frame `NSPM = NSYM * NSPS`. ref: msk144decodeframe.f90.
pub const MSK_NSPM: usize = MSK_NSYM * MSK_NSPS; // 864

/// Half-sine pulse `pp(i) = sin((i-1)·π/12)`, i=1..12 (const `sin` is
/// unavailable, so this is a small runtime build). ref: msk144decodeframe.f90:29-32.
pub fn half_sine() -> [f32; 12] {
    let mut pp = [0.0f32; 12];
    let pi = std::f32::consts::PI;
    for (i, p) in pp.iter_mut().enumerate() {
        *p = (i as f32 * pi / 12.0).sin();
    }
    pp
}

/// Continuous-phase MSK modulator: map 144 tone values (0/1) to 864 audio
/// samples. Tone 0 → `freq - baud/4`, tone 1 → `freq + baud/4`; phase is carried
/// across half-symbols (6 samples each). ref: msk144sim.f90:52-76.
pub fn cpfsk_modulate(itone: &[u8], freq_hz: f32) -> Vec<f32> {
    let twopi = std::f32::consts::TAU;
    let dphi0 = twopi * (freq_hz - 0.25 * MSK_BAUD) / MSK_FS;
    let dphi1 = twopi * (freq_hz + 0.25 * MSK_BAUD) / MSK_FS;
    let mut wave = Vec::with_capacity(itone.len() * MSK_NSPS);
    let mut phi = 0.0f32;
    for &t in itone {
        let dphi = if t == 0 { dphi0 } else { dphi1 };
        for _ in 0..MSK_NSPS {
            wave.push(phi.cos());
            phi = (phi + dphi) % twopi;
        }
    }
    wave
}

/// FFT-based analytic-signal filter for MSK144: a fixed bandpass centred at
/// 1500 Hz (flat within ±900 Hz, raised-cosine rolloff to ±1100 Hz), keeping
/// only positive frequencies (Hilbert). Caches the FFT plans and filter shape.
/// ref: analytic.f90 (beq=false path; the default equaliser is identity).
pub struct Analytic {
    nfft: usize,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    h: Vec<f32>, // bandpass magnitude, length nfft/2 + 1
    buf: Vec<Complex<f32>>,
}

impl Analytic {
    /// Build for a given FFT size (`nfft = 8192` in WSJT-X). Must be ≥ the block.
    pub fn new(nfft: usize) -> Self {
        let mut planner = FftPlanner::new();
        let fwd = planner.plan_fft_forward(nfft);
        let inv = planner.plan_fft_inverse(nfft);
        let nh = nfft / 2;
        let df = MSK_FS / nfft as f32;
        let pi = std::f32::consts::PI;
        let t = 1.0 / 2000.0f32;
        let beta = 0.1f32;
        let lo = (1.0 - beta) / (2.0 * t); // 900 Hz
        let hi = (1.0 + beta) / (2.0 * t); // 1100 Hz
        let mut h = vec![0.0f32; nh + 1];
        for (i, hi_val) in h.iter_mut().enumerate() {
            let ff = i as f32 * df;
            let f = ff - 1500.0;
            let af = f.abs();
            *hi_val = if af <= lo {
                1.0
            } else if af <= hi {
                0.5 * (1.0 + ((pi * t / beta) * (af - lo)).cos())
            } else {
                0.0
            };
        }
        Analytic { nfft, fwd, inv, h, buf: vec![Complex::new(0.0, 0.0); nfft] }
    }

    /// Convert `npts` real samples to the analytic (complex) signal, returning
    /// `nfft` complex samples (the caller uses the first `npts`). ref: analytic.f90.
    pub fn transform(&mut self, d: &[Sample]) -> Vec<Cplx> {
        let npts = d.len().min(self.nfft);
        let fac = 2.0 / self.nfft as f32;
        for (i, b) in self.buf.iter_mut().enumerate() {
            *b = if i < npts { Complex::new(fac * d[i], 0.0) } else { Complex::new(0.0, 0.0) };
        }
        self.fwd.process(&mut self.buf);
        let nh = self.nfft / 2;
        for i in 0..=nh {
            self.buf[i] *= self.h[i];
        }
        self.buf[0] *= 0.5; // half of DC term
        for i in (nh + 1)..self.nfft {
            self.buf[i] = Complex::new(0.0, 0.0); // zero negative frequencies
        }
        self.inv.process(&mut self.buf);
        self.buf.iter().map(|c| Cplx::new(c.re, c.im)).collect()
    }
}

/// Shift the frequency of an analytic signal by `f0` Hz (into `dst`), i.e.
/// multiply sample `i` by `e^{j·2π·f0·i/fs}` with `w` advanced *before* use.
/// ref: tweak1.f90.
pub fn freq_shift_into(src: &[Cplx], f0_hz: f32, dst: &mut [Cplx]) {
    let twopi = std::f64::consts::TAU;
    let dphi = twopi * f0_hz as f64 / MSK_FS as f64;
    let wstep = num_complex::Complex::<f64>::new(dphi.cos(), dphi.sin());
    let mut w = num_complex::Complex::<f64>::new(1.0, 0.0);
    for (s, d) in src.iter().zip(dst.iter_mut()) {
        w *= wstep;
        *d = Cplx::new((w.re as f32) * s.re - (w.im as f32) * s.im, (w.re as f32) * s.im + (w.im as f32) * s.re);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_sine_matches_reference() {
        let pp = half_sine();
        assert!((pp[0]).abs() < 1e-6);
        assert!((pp[6] - 1.0).abs() < 1e-6); // sin(6π/12)=sin(π/2)=1
        assert!((pp[3] - (std::f32::consts::PI / 4.0).sin()).abs() < 1e-6);
    }

    #[test]
    fn cpfsk_is_phase_continuous_and_unit_amplitude() {
        let itone = [0u8, 1, 0, 1, 1, 0];
        let w = cpfsk_modulate(&itone, 1500.0);
        assert_eq!(w.len(), itone.len() * MSK_NSPS);
        // Every sample is a cosine of a running phase → magnitude ≤ 1.
        assert!(w.iter().all(|&x| x.abs() <= 1.0 + 1e-6));
        // A pure tone-1 run advances phase by dphi1 each sample (continuity).
        let dphi1 = std::f32::consts::TAU * (1500.0 + 500.0) / MSK_FS;
        // samples 18..24 correspond to the 4th tone (value 1): check the local
        // sample-to-sample phase step is consistent with a single tone.
        let a = (w[19] - (w[18] * dphi1.cos())).abs();
        assert!(a < 0.2, "phase step inconsistent: {a}");
    }

    #[test]
    fn analytic_passes_inband_tone_and_rejects_out_of_band() {
        let mut an = Analytic::new(8192);
        let n = 7168;
        let fs = MSK_FS;
        // In-band 1500 Hz tone should survive; a 3500 Hz tone should be crushed.
        let mk = |f: f32| -> Vec<f32> {
            (0..n).map(|i| (std::f32::consts::TAU * f * i as f32 / fs).cos()).collect()
        };
        let inband = an.transform(&mk(1500.0));
        let oob = an.transform(&mk(3500.0));
        let e_in: f32 = inband[1000..6000].iter().map(|c| c.norm_sqr()).sum();
        let e_oob: f32 = oob[1000..6000].iter().map(|c| c.norm_sqr()).sum();
        assert!(e_in > 50.0 * e_oob, "bandpass failed: in={e_in} oob={e_oob}");
    }

    #[test]
    fn freq_shift_rotates() {
        let src: Vec<Cplx> = (0..100).map(|_| Cplx::new(1.0, 0.0)).collect();
        let mut dst = vec![Cplx::new(0.0, 0.0); 100];
        freq_shift_into(&src, 0.0, &mut dst);
        // Zero shift leaves magnitude 1 and (with w advanced first) a tiny phase.
        assert!(dst.iter().all(|c| (c.norm() - 1.0).abs() < 1e-4));
    }
}
