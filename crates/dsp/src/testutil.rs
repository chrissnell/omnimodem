//! Deterministic test fixtures: a seeded AWGN source and hex/byte helpers.
//! Gated behind `cfg(test)` or the `testutil` feature so production never
//! links it. `Math::random`-free: a fixed-seed xorshift + Box–Muller, so BER
//! sweeps and corpus generation are bit-reproducible across runs and machines.

use crate::types::Sample;

/// Minimal reproducible PRNG (xorshift64*). NOT cryptographic.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Standard normal via Box–Muller.
    pub fn next_normal(&mut self) -> f32 {
        let u1 = self.next_f32().max(1e-7);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }
}

/// Add white Gaussian noise of standard deviation `sigma` to `signal` in place.
pub fn add_awgn(signal: &mut [Sample], sigma: f32, rng: &mut Rng) {
    for s in signal.iter_mut() {
        *s += sigma * rng.next_normal();
    }
}

/// Fraction of `trials` for which `decoded(i)` holds. A declarative helper for
/// BER / decode-rate sweeps.
pub fn decode_rate(trials: usize, mut decoded: impl FnMut(usize) -> bool) -> f32 {
    let ok = (0..trials).filter(|&i| decoded(i)).count();
    ok as f32 / trials.max(1) as f32
}

/// Watterson HF channel: two independent slowly-fading paths with a fixed delay
/// and a Doppler spread (CCIR good/moderate/poor presets). Deterministic given a
/// seeded [`Rng`]. Simplified (two taps, one-pole-filtered fading) — the design
/// requires a seedable fading fixture for the BER sweeps, not a bit-exact model.
pub struct WattersonChannel {
    rate: f32,
    delay_samples: usize,
    doppler_hz: f32,
}

impl WattersonChannel {
    pub fn new(rate: f32, delay_ms: f32, doppler_hz: f32) -> Self {
        WattersonChannel { rate, delay_samples: (delay_ms * 1e-3 * rate) as usize, doppler_hz }
    }
    pub fn ccir_good(rate: f32) -> Self {
        Self::new(rate, 0.5, 0.1)
    }
    pub fn ccir_moderate(rate: f32) -> Self {
        Self::new(rate, 1.0, 0.5)
    }
    pub fn ccir_poor(rate: f32) -> Self {
        Self::new(rate, 2.0, 1.0)
    }

    /// Apply the channel: each path is scaled by a band-limited fading envelope
    /// (one-pole-filtered Gaussian at `doppler_hz`), then summed and normalized.
    pub fn apply(&self, signal: &[Sample], rng: &mut Rng) -> Vec<Sample> {
        let n = signal.len();
        let g0 = self.fading_envelope(n, rng);
        let g1 = self.fading_envelope(n, rng);
        let mut out = vec![0.0f32; n];
        for (i, o) in out.iter_mut().enumerate() {
            let direct = signal[i] * g0[i];
            let delayed =
                if i >= self.delay_samples { signal[i - self.delay_samples] * g1[i] } else { 0.0 };
            *o = 0.707 * (direct + delayed); // normalize two equal-power paths
        }
        out
    }

    fn fading_envelope(&self, n: usize, rng: &mut Rng) -> Vec<f32> {
        let alpha = (-2.0 * std::f32::consts::PI * self.doppler_hz / self.rate).exp();
        let mut env = Vec::with_capacity(n);
        let mut state = 0.0f32;
        for _ in 0..n {
            state = alpha * state + (1.0 - alpha) * rng.next_normal();
            env.push(1.0 + 0.5 * state);
        }
        env
    }
}

/// `sigma` for a target Eb/N0 (dB) given energy per bit and samples per bit.
pub fn sigma_for_ebn0(eb: f32, ebn0_db: f32, samples_per_bit: f32) -> f32 {
    let ebn0 = 10f32.powf(ebn0_db / 10.0);
    let n0 = eb / ebn0;
    // Two-sided noise power N0/2 per sample, spread over samples_per_bit.
    (n0 / 2.0 * samples_per_bit).sqrt()
}

/// Parse a whitespace-tolerant hex string into bytes. **Panics** on a
/// non-hex digit or an odd number of hex digits — intentional fail-fast for
/// malformed KAT vectors.
pub fn hex_to_bytes(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

pub fn bytes_to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_for_seed() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn awgn_mean_near_zero_variance_near_sigma_sq() {
        let mut rng = Rng::new(7);
        let mut buf = vec![0.0f32; 100_000];
        add_awgn(&mut buf, 0.5, &mut rng);
        let mean: f32 = buf.iter().sum::<f32>() / buf.len() as f32;
        let var: f32 = buf.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / buf.len() as f32;
        assert!(mean.abs() < 0.02, "mean {mean}");
        assert!((var - 0.25).abs() < 0.02, "var {var}");
    }

    #[test]
    fn hex_roundtrips() {
        assert_eq!(bytes_to_hex(&hex_to_bytes("0a ff 10")), "0aff10");
    }

    #[test]
    fn watterson_preserves_length_and_is_deterministic() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(1);
        let sig: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.1).sin()).collect();
        let chan = WattersonChannel::ccir_good(8_000.0);
        let out1 = chan.apply(&sig, &mut a);
        let out2 = chan.apply(&sig, &mut b);
        assert_eq!(out1.len(), sig.len());
        assert_eq!(out1, out2, "same seed → identical fading");
    }

    #[test]
    fn decode_rate_counts_successes() {
        assert_eq!(decode_rate(10, |i| i % 2 == 0), 0.5);
        assert_eq!(decode_rate(4, |_| true), 1.0);
    }

    #[test]
    fn sigma_for_ebn0_matches_closed_form_and_is_monotonic() {
        // sigma = sqrt(N0/2 * sps), N0 = Eb / (10^(EbN0_dB/10)). For Eb=1, sps=1:
        //   0 dB  -> ebn0=1  -> N0=1   -> sigma = sqrt(0.5)  ≈ 0.70711
        //   10 dB -> ebn0=10 -> N0=0.1 -> sigma = sqrt(0.05) ≈ 0.22361
        assert!((sigma_for_ebn0(1.0, 0.0, 1.0) - 0.5f32.sqrt()).abs() < 1e-6);
        assert!((sigma_for_ebn0(1.0, 10.0, 1.0) - 0.05f32.sqrt()).abs() < 1e-6);
        // samples_per_bit scales the noise variance linearly.
        let s1 = sigma_for_ebn0(1.0, 3.0, 1.0);
        let s4 = sigma_for_ebn0(1.0, 3.0, 4.0);
        assert!((s4 / s1 - 2.0).abs() < 1e-5, "4x sps must double sigma");
        // Higher Eb/N0 => smaller sigma.
        assert!(sigma_for_ebn0(1.0, 6.0, 1.0) < sigma_for_ebn0(1.0, 0.0, 1.0));
    }
}
