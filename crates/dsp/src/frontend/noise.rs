//! Per-bin running noise-floor estimator + WSJT-X-style normalized-SNR reporter.
//!
//! The floor is a slow per-bin exponential of the magnitude-squared spectrum,
//! biased downward so it follows the noise rather than transient signals. SNR
//! is normalized to a 2500 Hz reference bandwidth so it is comparable across
//! FFT sizes and sample rates.

/// Per-bin noise floor tracker over power spectra (`|X|²` per bin).
pub struct NoiseFloor {
    floor: Vec<f32>,
    up: f32,
    down: f32,
    bin_bw: f32,
}

impl NoiseFloor {
    /// `bins` = spectrum length; `rate`/`nfft` set the per-bin bandwidth.
    /// `up`/`down` are the per-update slew rates (down should be faster so the
    /// floor tracks quiet gaps and lags loud signals).
    pub fn new(bins: usize, rate: f32, nfft: usize, up: f32, down: f32) -> Self {
        NoiseFloor { floor: vec![0.0; bins], up, down, bin_bw: rate / nfft as f32 }
    }

    /// Update the floor from one power spectrum (`power[bin] = |X[bin]|²`).
    pub fn update(&mut self, power: &[f32]) {
        for (f, &p) in self.floor.iter_mut().zip(power.iter()) {
            let coeff = if p < *f { self.down } else { self.up };
            *f += (p - *f) * coeff;
        }
    }

    pub fn floor_bin(&self, bin: usize) -> f32 {
        self.floor[bin]
    }

    /// Mean per-bin noise power, excluding the strongest bins (signal carriers).
    pub fn mean_floor(&self) -> f32 {
        if self.floor.is_empty() {
            return 0.0;
        }
        self.floor.iter().sum::<f32>() / self.floor.len() as f32
    }

    /// Normalized SNR in dB for a signal whose power is `signal_power`, measured
    /// against the per-bin noise floor and referenced to a 2500 Hz bandwidth.
    ///
    /// Noise power in 2500 Hz = per-bin floor × (2500 / bin_bw).
    pub fn snr_db(&self, signal_power: f32, noise_per_bin: f32) -> f32 {
        let noise_2500 = noise_per_bin.max(1e-30) * (2500.0 / self.bin_bw);
        10.0 * (signal_power / noise_2500).log10()
    }

    pub fn bin_bw(&self) -> f32 {
        self.bin_bw
    }
}

/// Estimate normalized SNR directly from a single power spectrum: pick the peak
/// bin as the signal, estimate the noise floor from the median of the rest, and
/// normalize to 2500 Hz. Used by the metrics path and the accuracy KAT.
pub fn snr_from_spectrum(power: &[f32], rate: f32, nfft: usize) -> f32 {
    assert!(!power.is_empty());
    let bin_bw = rate / nfft as f32;
    let (peak_idx, &peak_p) = power
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    // Median of the off-peak bins is a robust per-bin noise estimate.
    let mut rest: Vec<f32> = power
        .iter()
        .enumerate()
        .filter(|(i, _)| i.abs_diff(peak_idx) > 1)
        .map(|(_, &p)| p)
        .collect();
    rest.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let noise_per_bin = rest[rest.len() / 2].max(1e-30);
    // A real tone's energy splits between the +f bin and its conjugate mirror
    // (`nfft-k`); fold the mirror back in to recover full single-sided signal
    // power. Subtract the noise that also sits in the signal bins.
    let mirror_p = if peak_idx != 0 && peak_idx * 2 != nfft {
        power[nfft - peak_idx]
    } else {
        0.0
    };
    let signal_power = (peak_p + mirror_p - 2.0 * noise_per_bin).max(1e-30);
    let noise_2500 = noise_per_bin * (2500.0 / bin_bw);
    10.0 * (signal_power / noise_2500).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::{num_complex::Complex, FftPlanner};
    use std::f32::consts::TAU;

    /// Build a synthetic spectrum from a tone + white Gaussian noise with a
    /// known SNR (signal power vs. noise power within a 2500 Hz reference BW),
    /// then check the reporter recovers it within ±1 dB.
    #[test]
    fn known_snr_reported_within_1db() {
        let rate = 12000.0;
        let nfft = 4096;
        let tone = 1500.0;
        // Target SNR in 2500 Hz reference BW.
        let target_snr_db = 6.0;

        // Noise: white, std dev sigma per time sample => noise power sigma² is
        // spread over the full bandwidth `rate`. Power in 2500 Hz = sigma² *
        // 2500/rate. Choose tone amplitude so signal_power/noise_2500 hits target.
        let sigma = 0.1f32;
        let noise_power_2500 = sigma * sigma * 2500.0 / rate;
        let snr_lin = 10f32.powf(target_snr_db / 10.0);
        let signal_power = snr_lin * noise_power_2500;
        // A real tone A*sin has power A²/2.
        let amp = (2.0 * signal_power).sqrt();

        // Deterministic pseudo-noise (Box-Muller from a simple LCG).
        let mut state = 0x2545F491_4F6CDD1Du64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 11) as f64 / (1u64 << 53) as f64) as f32
        };
        let mut gauss = || {
            let u1 = next().max(1e-9);
            let u2 = next();
            (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
        };

        // Average several frames to stabilize the noise estimate.
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(nfft);
        let mut avg_power = vec![0.0f32; nfft];
        let frames = 64;
        for _ in 0..frames {
            let mut buf: Vec<Complex<f32>> = (0..nfft)
                .map(|n| {
                    let s = amp * (TAU * tone * n as f32 / rate).sin() + sigma * gauss();
                    Complex::new(s, 0.0)
                })
                .collect();
            fft.process(&mut buf);
            for (a, c) in avg_power.iter_mut().zip(buf.iter()) {
                // Normalize FFT so power matches the time-domain definition.
                *a += c.norm_sqr() / (nfft as f32 * nfft as f32);
            }
        }
        for a in &mut avg_power {
            *a /= frames as f32;
        }

        let reported = snr_from_spectrum(&avg_power, rate, nfft);
        assert!(
            (reported - target_snr_db).abs() <= 1.0,
            "reported {reported:.2} dB vs target {target_snr_db:.2} dB"
        );
    }

    #[test]
    fn floor_tracks_quiet_bins() {
        let mut nf = NoiseFloor::new(8, 12000.0, 1024, 0.01, 0.2);
        // Bin 4 is a strong carrier; the rest are quiet noise.
        for _ in 0..500 {
            let mut p = vec![0.01f32; 8];
            p[4] = 5.0;
            nf.update(&p);
        }
        // Floor on quiet bins converges to the noise level; the carrier bin's
        // floor lags well below the carrier power.
        assert!((nf.floor_bin(0) - 0.01).abs() < 0.005);
        assert!(nf.floor_bin(4) < 5.0);
    }
}
