//! Polyphase rational resampler (upgrade of the Phase-2 linear interpolator).
//!
//! Rate change `in_rate -> out_rate` is reduced to `L/M` (up by `L`, down by
//! `M`). A single windowed-sinc prototype lowpass at the smaller of the two
//! band edges is decomposed into `L` polyphase sub-filters; each output sample
//! picks one phase and a decimation stride. This kills the imaging/aliasing the
//! Phase-2 linear interpolator left behind. Operates on `Sample` (`f32`).

use crate::frontend::fir::design_lowpass;
use crate::types::{Cplx, Sample};

pub struct Resampler {
    /// Interpolation factor (numerator of the reduced ratio).
    up: usize,
    /// Decimation factor (denominator of the reduced ratio).
    down: usize,
    /// Polyphase banks: `phases[p][k]` is tap `k` of sub-filter `p`.
    phases: Vec<Vec<f32>>,
    taps_per_phase: usize,
    /// Delay line of input samples (newest pushed at the back via ring index).
    hist: Vec<Sample>,
    hist_pos: usize,
    /// Polyphase commutator position in `[0, up)`.
    phase: usize,
    passthrough: bool,
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

impl Resampler {
    /// `taps_per_phase` controls the prototype length (`up * taps_per_phase`
    /// total taps); 12–24 gives ≥40 dB image rejection for typical ratios.
    pub fn new(in_rate: u32, out_rate: u32, taps_per_phase: usize) -> Self {
        assert!(in_rate > 0 && out_rate > 0 && taps_per_phase > 0);
        let g = gcd(in_rate as usize, out_rate as usize);
        let up = out_rate as usize / g;
        let down = in_rate as usize / g;

        if up == 1 && down == 1 {
            return Resampler {
                up,
                down,
                phases: Vec::new(),
                taps_per_phase: 0,
                hist: Vec::new(),
                hist_pos: 0,
                phase: 0,
                passthrough: true,
            };
        }

        // Prototype lowpass: cutoff at the lower Nyquist of in/out, designed at
        // the upsampled rate and scaled by `up` to keep unity passband gain.
        let proto_len = up * taps_per_phase;
        let proto_len = proto_len | 1; // odd length => linear phase, integer delay
        let cutoff = 0.5 / up.max(down) as f32; // cycles/sample at the up-rate
        let mut proto = design_lowpass(proto_len, cutoff, 1.0);
        proto.iter_mut().for_each(|t| *t *= up as f32);

        // Decompose into `up` polyphase sub-filters. Sub-filter `p` holds taps
        // p, p+up, p+2*up, ... so each runs at the *input* rate.
        let mut phases = vec![Vec::new(); up];
        for (i, &t) in proto.iter().enumerate() {
            phases[i % up].push(t);
        }
        let taps_per_phase = phases.iter().map(|p| p.len()).max().unwrap_or(0);
        for p in &mut phases {
            p.resize(taps_per_phase, 0.0);
        }

        Resampler {
            up,
            down,
            phases,
            taps_per_phase,
            hist: vec![0.0; taps_per_phase],
            hist_pos: 0,
            phase: 0,
            passthrough: false,
        }
    }

    fn push_input(&mut self, x: Sample) {
        self.hist[self.hist_pos] = x;
        self.hist_pos = (self.hist_pos + 1) % self.taps_per_phase;
    }

    /// Convolve the current delay line with polyphase sub-filter `p`.
    fn phase_output(&self, p: usize) -> Sample {
        let bank = &self.phases[p];
        let n = self.taps_per_phase;
        let mut acc = 0.0f32;
        // hist_pos points at the slot the *next* input will overwrite, so the
        // newest sample is at hist_pos-1. Walk newest->oldest against tap 0..n.
        for (k, &h) in bank.iter().enumerate() {
            let idx = (self.hist_pos + n - 1 - k) % n;
            acc += h * self.hist[idx];
        }
        acc
    }

    pub fn process(&mut self, input: &[Sample]) -> Vec<Sample> {
        if self.passthrough {
            return input.to_vec();
        }
        let mut out = Vec::with_capacity(input.len() * self.up / self.down + 1);
        for &x in input {
            self.push_input(x);
            // For this new input sample, emit every polyphase output whose
            // commutator position falls before the next input boundary `up`.
            while self.phase < self.up {
                out.push(self.phase_output(self.phase));
                self.phase += self.down;
            }
            self.phase -= self.up;
        }
        out
    }

    pub fn ratio(&self) -> (usize, usize) {
        (self.up, self.down)
    }
}

/// Complex decimating/interpolating resampler: two real polyphase `Resampler`s,
/// one per quadrature rail. Correct for a narrowband channel centered at DC
/// (the output of an NCO channel-select), which is all the SDR RX path needs.
pub struct ComplexResampler {
    i: Resampler,
    q: Resampler,
}

impl ComplexResampler {
    pub fn new(in_rate: u32, out_rate: u32, taps_per_phase: usize) -> Self {
        ComplexResampler {
            i: Resampler::new(in_rate, out_rate, taps_per_phase),
            q: Resampler::new(in_rate, out_rate, taps_per_phase),
        }
    }

    /// Resample a block of complex samples. I and Q share the ratio, so both
    /// rails emit the same count and zip cleanly.
    pub fn process(&mut self, input: &[Cplx]) -> Vec<Cplx> {
        let re: Vec<f32> = input.iter().map(|z| z.re).collect();
        let im: Vec<f32> = input.iter().map(|z| z.im).collect();
        let ro = self.i.process(&re);
        let io = self.q.process(&im);
        ro.into_iter().zip(io).map(|(r, i)| Cplx::new(r, i)).collect()
    }

    /// Reduced (up, down) ratio, from the I rail (both rails are identical).
    pub fn ratio(&self) -> (usize, usize) {
        self.i.ratio()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn goertzel_power(sig: &[f32], freq: f32, rate: f32) -> f32 {
        let w = TAU * freq / rate;
        let cw = w.cos();
        let coeff = 2.0 * cw;
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in sig {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    #[test]
    fn passthrough_when_rates_equal() {
        let mut r = Resampler::new(48000, 48000, 16);
        let input: Vec<f32> = (0..100).map(|n| (n as f32 * 0.1).sin()).collect();
        assert_eq!(r.process(&input), input);
    }

    #[test]
    fn output_length_matches_ratio() {
        let mut r = Resampler::new(48000, 12000, 16);
        let n = 4000;
        let input: Vec<f32> = (0..n).map(|i| (TAU * 1000.0 * i as f32 / 48000.0).sin()).collect();
        let out = r.process(&input);
        let expected = n * 12000 / 48000;
        assert!(
            (out.len() as i64 - expected as i64).abs() <= 1,
            "len {} vs expected {}",
            out.len(),
            expected
        );
    }

    #[test]
    fn tone_survives_decimation_with_images_down() {
        // 1 kHz tone, 48k -> 12k. Tone must survive; an image (e.g. an alias of
        // a higher band) must be ≥40 dB down. We measure the surviving tone vs.
        // an out-of-band probe frequency that should contain only filter floor.
        let mut r = Resampler::new(48000, 12000, 24);
        let in_n = 48000;
        // Mix a 1 kHz wanted tone with a 5 kHz tone that must be filtered out
        // (it aliases to 12000-5000 region; the anti-alias LP should kill it).
        let input: Vec<f32> = (0..in_n)
            .map(|i| {
                let t = i as f32 / 48000.0;
                (TAU * 1000.0 * t).sin() + (TAU * 5000.0 * t).sin()
            })
            .collect();
        let out = r.process(&input);
        // Drop filter transient.
        let steady = &out[200..];
        let want = goertzel_power(steady, 1000.0, 12000.0);
        // The 5 kHz tone is above the 6 kHz Nyquist? No: 5k<6k. It will alias
        // only if not filtered; our LP cutoff is ~6 kHz so 5 kHz passes. Use a
        // 5.5 kHz tone instead to be safely in the transition/stopband.
        let _ = want;

        let mut r2 = Resampler::new(48000, 12000, 24);
        let input2: Vec<f32> = (0..in_n)
            .map(|i| {
                let t = i as f32 / 48000.0;
                (TAU * 1000.0 * t).sin() + (TAU * 9000.0 * t).sin()
            })
            .collect();
        let out2 = r2.process(&input2);
        let steady2 = &out2[200..];
        let p_wanted = goertzel_power(steady2, 1000.0, 12000.0);
        // 9 kHz aliases to 12000-9000 = 3000 Hz after decimation; must be ≥40 dB down.
        let p_image = goertzel_power(steady2, 3000.0, 12000.0);
        let ratio_db = 10.0 * (p_wanted / p_image.max(1e-20)).log10();
        assert!(ratio_db >= 40.0, "image only {ratio_db:.1} dB down");
    }

    #[test]
    fn upsample_preserves_tone() {
        // 8k -> 24k upsample of a 1 kHz tone.
        let mut r = Resampler::new(8000, 24000, 16);
        let input: Vec<f32> = (0..8000).map(|i| (TAU * 1000.0 * i as f32 / 8000.0).sin()).collect();
        let out = r.process(&input);
        let expected = 8000 * 24000 / 8000;
        assert!((out.len() as i64 - expected as i64).abs() <= 2);
        let p1k = goertzel_power(&out[200..], 1000.0, 24000.0);
        let p3k = goertzel_power(&out[200..], 3000.0, 24000.0);
        assert!(10.0 * (p1k / p3k.max(1e-20)).log10() >= 40.0);
    }

    #[test]
    fn complex_resampler_decimates_length() {
        use crate::types::Cplx;
        let mut r = ComplexResampler::new(240_000, 48_000, 16);
        let input: Vec<Cplx> = (0..4800).map(|_| Cplx::new(0.25, -0.25)).collect();
        let out = r.process(&input);
        // 5:1 decimation → ~960 out; polyphase warm-up allows a small delta.
        assert!((out.len() as i32 - 960).abs() <= 16, "got {}", out.len());
    }

    #[test]
    fn complex_resampler_preserves_dc() {
        use crate::types::Cplx;
        let mut r = ComplexResampler::new(240_000, 48_000, 16);
        let input: Vec<Cplx> = (0..48_000).map(|_| Cplx::new(0.5, 0.5)).collect();
        let out = r.process(&input);
        let mean = out.iter().copied().sum::<Cplx>() / out.len() as f32;
        assert!((mean.re - 0.5).abs() < 0.05 && (mean.im - 0.5).abs() < 0.05, "got {mean:?}");
    }
}
