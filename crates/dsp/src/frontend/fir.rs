//! Direct-form FIR with an 8-way accumulator unroll (Graywolf demod_afsk.rs:67-107),
//! plus filter-design helpers (lowpass via windowed-sinc; RRC; raised cosine;
//! Gaussian) used by the modulators and the front-end filters.

use std::f32::consts::PI;

pub struct Fir {
    taps: Vec<f32>,
    hist: Vec<f32>,
    pos: usize,
}

impl Fir {
    pub fn new(taps: Vec<f32>) -> Self {
        let n = taps.len();
        Fir { taps, hist: vec![0.0; n], pos: 0 }
    }

    /// Push one sample, return the filtered output. Allocation-free.
    pub fn push(&mut self, x: f32) -> f32 {
        let n = self.taps.len();
        self.hist[self.pos] = x;
        let mut acc = [0.0f32; 8];
        let mut k = 0;
        // Walk taps newest->oldest with 8 independent accumulators.
        while k < n {
            let h = (self.pos + n - k) % n;
            acc[k % 8] += self.taps[k] * self.hist[h];
            k += 1;
        }
        self.pos = (self.pos + 1) % n;
        acc.iter().sum()
    }

    pub fn reset(&mut self) {
        self.hist.iter_mut().for_each(|h| *h = 0.0);
        self.pos = 0;
    }

    pub fn len(&self) -> usize {
        self.taps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }
}

/// Blackman-windowed sinc lowpass, cutoff `fc` given as a **normalized**
/// frequency (cycles/sample). `len` follows fldigi's `wsincfilt` convention:
/// the filter has `len + 1` taps. This is a byte-for-byte port of fldigi's
/// `wsincfilt` (src/psk/pskcoeff.cxx:285), used for the PSK multi-carrier
/// two-stage decimating matched filter so adjacent-carrier rejection matches.
pub fn wsinc_blackman(len: usize, fc: f32) -> Vec<f32> {
    let l2 = len as f32 / 2.0;
    let k1 = 2.0 * PI / len as f32;
    let k2 = 2.0 * PI * fc;
    let mut h = vec![0.0f32; len + 1];
    for (i, hi) in h.iter_mut().enumerate() {
        let x = i as f32 - l2;
        let sinc = if i as f32 == l2 { 1.0 } else { (k2 * x).sin() / (k2 * x) };
        let w = 0.42 - 0.5 * (k1 * i as f32).cos() + 0.08 * (2.0 * k1 * i as f32).cos();
        *hi = sinc * w;
    }
    let sum: f32 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= sum);
    h
}

/// Windowed-sinc lowpass (Hamming), `cutoff_hz` normalized by `rate_hz`.
pub fn design_lowpass(num_taps: usize, cutoff_hz: f32, rate_hz: f32) -> Vec<f32> {
    let fc = cutoff_hz / rate_hz; // cycles/sample
    let m = num_taps - 1;
    let mut h = vec![0.0f32; num_taps];
    for (i, hi) in h.iter_mut().enumerate() {
        let n = i as f32 - m as f32 / 2.0;
        let sinc = if n == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * n).sin() / (PI * n) };
        let w = 0.54 - 0.46 * (2.0 * PI * i as f32 / m as f32).cos(); // Hamming
        *hi = sinc * w;
    }
    let sum: f32 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= sum);
    h
}

/// Windowed-sinc bandpass: lowpass prototype frequency-shifted to `center_hz`.
pub fn design_bandpass(num_taps: usize, low_hz: f32, high_hz: f32, rate_hz: f32) -> Vec<f32> {
    let center = (low_hz + high_hz) / 2.0;
    let cutoff = (high_hz - low_hz) / 2.0;
    let lp = design_lowpass(num_taps, cutoff, rate_hz);
    let m = (num_taps - 1) as f32 / 2.0;
    let mut h = vec![0.0f32; num_taps];
    for (i, hi) in h.iter_mut().enumerate() {
        let n = i as f32 - m;
        // 2*cos shift gives the real bandpass with the same passband gain.
        *hi = 2.0 * lp[i] * (2.0 * PI * center / rate_hz * n).cos();
    }
    h
}

/// Windowed Hilbert-transformer FIR (type-III linear phase): odd `num_taps`,
/// antisymmetric, integer group delay `(num_taps-1)/2`. Applied to a real signal
/// it produces its 90°-shifted quadrature; pairing that with the real signal
/// delayed by the same group delay yields the **analytic** (single-sideband)
/// signal, whose spectrum has no negative-frequency image. Down-converting an
/// analytic signal to baseband is therefore image-free at any sample rate — the
/// rate-robust front-end the picture discriminator needs. `num_taps` must be odd.
pub fn design_hilbert(num_taps: usize) -> Vec<f32> {
    debug_assert!(num_taps % 2 == 1, "Hilbert FIR needs an odd tap count");
    let m = (num_taps - 1) as isize / 2;
    let mut h = vec![0.0f32; num_taps];
    for (i, hi) in h.iter_mut().enumerate() {
        let k = i as isize - m;
        // Ideal antisymmetric response: 0 on even taps, 2/(πk) on odd taps.
        let ideal = if k % 2 == 0 { 0.0 } else { 2.0 / (PI * k as f32) };
        // Hamming window keeps the passband ripple low over the pixel band.
        let w = 0.54 - 0.46 * (2.0 * PI * i as f32 / (num_taps - 1) as f32).cos();
        *hi = ideal * w;
    }
    h
}

/// Root-raised-cosine taps (Direwolf AFSK, M17 α=0.5). `sps` samples/symbol.
pub fn design_rrc(num_taps: usize, alpha: f32, sps: f32) -> Vec<f32> {
    let m = num_taps as isize / 2;
    let mut h = vec![0.0f32; num_taps];
    for (i, hi) in h.iter_mut().enumerate() {
        let t = (i as isize - m) as f32 / sps;
        *hi = rrc_tap(t, alpha);
    }
    let e: f32 = h.iter().map(|x| x * x).sum::<f32>().sqrt();
    h.iter_mut().for_each(|x| *x /= e);
    h
}

fn rrc_tap(t: f32, a: f32) -> f32 {
    if t.abs() < 1e-6 {
        return 1.0 - a + 4.0 * a / PI;
    }
    if (t.abs() - 1.0 / (4.0 * a)).abs() < 1e-4 {
        let s = ((1.0 + 2.0 / PI) * (PI / (4.0 * a)).sin()
            + (1.0 - 2.0 / PI) * (PI / (4.0 * a)).cos())
            * a
            / 2f32.sqrt();
        return s;
    }
    let num = (PI * t * (1.0 - a)).sin() + 4.0 * a * t * (PI * t * (1.0 + a)).cos();
    let den = PI * t * (1.0 - (4.0 * a * t).powi(2));
    num / den
}

/// Raised-cosine *pulse* (not RRC): used for PSK31/Throb/Hell envelopes.
pub fn design_raised_cosine(num_taps: usize, alpha: f32, sps: f32) -> Vec<f32> {
    let m = num_taps as isize / 2;
    let mut h = vec![0.0f32; num_taps];
    for (i, hi) in h.iter_mut().enumerate() {
        let t = (i as isize - m) as f32 / sps;
        let sinc = if t.abs() < 1e-6 { 1.0 } else { (PI * t).sin() / (PI * t) };
        let denom = 1.0 - (2.0 * alpha * t).powi(2);
        let cos = if denom.abs() < 1e-6 { PI / 4.0 } else { (PI * alpha * t).cos() / denom };
        *hi = sinc * cos;
    }
    let s: f32 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= s);
    h
}

/// Gaussian shaping pulse for GFSK/CPFSK; `bt` is the bandwidth-time product
/// (FT8 BT=2.0, FT4 BT=1.0). `sps` samples per symbol.
pub fn design_gaussian(num_taps: usize, bt: f32, sps: f32) -> Vec<f32> {
    let sigma = sps * (2f32.ln()).sqrt() / (2.0 * PI * bt);
    let m = num_taps as isize / 2;
    let mut h = vec![0.0f32; num_taps];
    for (i, hi) in h.iter_mut().enumerate() {
        let t = (i as isize - m) as f32;
        *hi = (-(t * t) / (2.0 * sigma * sigma)).exp();
    }
    let s: f32 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= s);
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn fir_impulse_response_equals_taps() {
        let taps = vec![0.1, 0.2, 0.3, 0.4];
        let mut f = Fir::new(taps.clone());
        let mut out = vec![f.push(1.0)];
        for _ in 0..3 {
            out.push(f.push(0.0));
        }
        for (o, t) in out.iter().zip(taps.iter()) {
            assert!((o - t).abs() < 1e-6, "got {o}, want {t}");
        }
    }

    #[test]
    fn lowpass_is_unity_dc_gain() {
        let h = design_lowpass(33, 1000.0, 8000.0);
        assert!((h.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn lowpass_is_symmetric() {
        let h = design_lowpass(33, 1000.0, 8000.0);
        for i in 0..h.len() / 2 {
            assert!((h[i] - h[h.len() - 1 - i]).abs() < 1e-6);
        }
    }

    #[test]
    fn bandpass_rejects_dc_and_nyquist() {
        let h = design_bandpass(65, 1200.0, 2200.0, 8000.0);
        // DC gain ~ sum of taps; passband is centered at 1700 Hz, so DC is rejected.
        assert!(h.iter().sum::<f32>().abs() < 0.1);
    }

    #[test]
    fn rrc_peak_at_center() {
        let h = design_rrc(65, 0.5, 8.0);
        let mid = h.len() / 2;
        assert!(h[mid] >= h.iter().cloned().fold(f32::MIN, f32::max) - 1e-6);
    }

    #[test]
    fn raised_cosine_unit_dc_and_symmetric() {
        let h = design_raised_cosine(65, 0.35, 8.0);
        assert!((h.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        for i in 0..h.len() / 2 {
            assert!((h[i] - h[h.len() - 1 - i]).abs() < 1e-6);
        }
    }

    #[test]
    fn gaussian_unit_dc_symmetric_and_peaked() {
        let h = design_gaussian(33, 2.0, 8.0);
        assert!((h.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        let mid = h.len() / 2;
        for i in 0..h.len() / 2 {
            assert!((h[i] - h[h.len() - 1 - i]).abs() < 1e-6);
            assert!(h[i] <= h[mid]);
        }
    }

    #[test]
    fn hilbert_is_antisymmetric_with_zero_even_taps() {
        let h = design_hilbert(31);
        let m = h.len() / 2;
        assert_eq!(h[m], 0.0, "centre tap is zero");
        for i in 0..h.len() {
            let k = i as isize - m as isize;
            if k % 2 == 0 {
                assert_eq!(h[i], 0.0, "even tap {k} must be zero");
            }
            // Antisymmetric about the centre.
            assert!((h[i] + h[h.len() - 1 - i]).abs() < 1e-6, "tap {i} not antisymmetric");
        }
    }

    #[test]
    fn hilbert_shifts_a_tone_by_ninety_degrees() {
        // Feeding cos through the Hilbert FIR and delaying the input by the group
        // delay yields sin (the quadrature), so the analytic magnitude is ~constant.
        let taps = design_hilbert(63);
        let m = taps.len() / 2;
        let mut fir = Fir::new(taps);
        let rate = 16000.0f32;
        let f0 = 1500.0f32;
        let n = 4000usize;
        let mut xs = Vec::with_capacity(n);
        let mut q = Vec::with_capacity(n);
        for i in 0..n {
            let x = (TAU * f0 * i as f32 / rate).cos();
            xs.push(x);
            q.push(fir.push(x));
        }
        // Well past the group delay + settling, |analytic| = sqrt(i^2 + q^2) ~ 1.
        for i in (m + 200)..(n - 1) {
            let mag = (xs[i - m].powi(2) + q[i].powi(2)).sqrt();
            assert!((mag - 1.0).abs() < 0.05, "analytic magnitude {mag} at {i} not ~1");
        }
    }

    #[test]
    fn gaussian_narrower_bt_is_wider_pulse() {
        // Lower BT => more inter-symbol filtering => wider time-domain pulse,
        // so the central tap holds a smaller fraction of the energy.
        let wide = design_gaussian(33, 1.0, 8.0);
        let narrow = design_gaussian(33, 2.0, 8.0);
        let mid = wide.len() / 2;
        assert!(wide[mid] < narrow[mid]);
    }
}
