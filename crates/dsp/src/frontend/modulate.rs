//! Symmetric modulator bank: each modulator is the TX twin of a Phase-4 RX
//! detector. All output `Vec<Sample>` at a native rate. Phase-continuous FSK
//! integrates instantaneous frequency; PSK shapes a raised-cosine envelope.

use crate::frontend::fir::design_gaussian;
use crate::types::Sample;
use std::f32::consts::TAU;

/// Continuous-phase FSK with optional Gaussian pulse shaping (GFSK/CPFSK).
/// Used by FT8/FT4 (8-FSK). `tone_hz(symbol)` maps a symbol to a tone.
pub struct Gfsk {
    rate: f32,
    sps: usize,
    base_hz: f32,
    spacing_hz: f32,
    bt: f32,
}

impl Gfsk {
    pub fn new(rate: f32, sps: usize, base_hz: f32, spacing_hz: f32, bt: f32) -> Self {
        Gfsk { rate, sps, base_hz, spacing_hz, bt }
    }

    /// Modulate a symbol stream. Each symbol's tone is `base + symbol*spacing`.
    /// A Gaussian filter smooths the per-sample frequency trajectory, and phase
    /// is the running integral of frequency => phase-continuous output.
    pub fn modulate(&self, symbols: &[u32]) -> Vec<Sample> {
        // Per-sample target frequency, one value per output sample.
        let mut freq: Vec<f32> = Vec::with_capacity(symbols.len() * self.sps);
        for &s in symbols {
            let f = self.base_hz + s as f32 * self.spacing_hz;
            for _ in 0..self.sps {
                freq.push(f);
            }
        }
        if freq.is_empty() {
            return Vec::new();
        }
        // Gaussian-shape the frequency trajectory (BT product). Skip for BT<=0.
        let shaped = if self.bt > 0.0 {
            let taps = (self.sps * 4) | 1;
            let g = design_gaussian(taps, self.bt, self.sps as f32);
            convolve_same(&freq, &g, freq[0])
        } else {
            freq
        };
        // Integrate frequency -> phase.
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(shaped.len());
        for &f in &shaped {
            phase += TAU * f / self.rate;
            if phase > TAU {
                phase -= TAU;
            }
            out.push(phase.sin());
        }
        out
    }
}

/// Same-length convolution with edge-extension by `edge` (keeps the frequency
/// trajectory from dipping toward zero at the boundaries).
fn convolve_same(x: &[f32], h: &[f32], edge: f32) -> Vec<f32> {
    let n = x.len();
    let m = h.len();
    let half = m / 2;
    let mut out = vec![0.0f32; n];
    for (i, o) in out.iter_mut().enumerate() {
        let mut acc = 0.0f32;
        for (k, &hk) in h.iter().enumerate() {
            let idx = i as isize + k as isize - half as isize;
            let xv = if idx < 0 {
                edge
            } else if idx as usize >= n {
                x[n - 1]
            } else {
                x[idx as usize]
            };
            acc += hk * xv;
        }
        *o = acc;
    }
    out
}

/// Parametric M-FSK tone bank (MFSK/Olivia/WSPR/4-FSK). Non-shaped continuous
/// phase: tone `k` is `base + k*spacing`.
pub struct MFsk {
    rate: f32,
    sps: usize,
    base_hz: f32,
    spacing_hz: f32,
    tones: u32,
}

impl MFsk {
    pub fn new(rate: f32, sps: usize, base_hz: f32, spacing_hz: f32, tones: u32) -> Self {
        MFsk { rate, sps, base_hz, spacing_hz, tones }
    }

    pub fn modulate(&self, symbols: &[u32]) -> Vec<Sample> {
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(symbols.len() * self.sps);
        for &s in symbols {
            let tone = s.min(self.tones - 1);
            let f = self.base_hz + tone as f32 * self.spacing_hz;
            let dp = TAU * f / self.rate;
            for _ in 0..self.sps {
                phase += dp;
                if phase > TAU {
                    phase -= TAU;
                }
                out.push(phase.sin());
            }
        }
        out
    }
}

/// Differential PSK (BPSK/QPSK) with a raised-cosine symbol envelope (PSK31/
/// ARDOP). Bits are differentially encoded so the demod needs no absolute phase.
pub struct DiffPsk {
    rate: f32,
    carrier_hz: f32,
    sps: usize,
    /// Bits per symbol: 1 = DBPSK, 2 = DQPSK.
    bps: u32,
}

impl DiffPsk {
    pub fn new(rate: f32, carrier_hz: f32, sps: usize, bps: u32) -> Self {
        assert!(bps == 1 || bps == 2);
        DiffPsk { rate, carrier_hz, sps, bps }
    }

    /// Differential phase encode: returns the absolute phase index per symbol
    /// (in units of 2π/M), where M = 2^bps.
    pub fn diff_encode(&self, symbols: &[u32]) -> Vec<u32> {
        let m = 1u32 << self.bps;
        let mut acc = 0u32;
        let mut out = Vec::with_capacity(symbols.len());
        for &s in symbols {
            acc = (acc + s) % m;
            out.push(acc);
        }
        out
    }

    /// Inverse of `diff_encode`: recover the symbol stream from absolute phases.
    pub fn diff_decode(&self, phases: &[u32]) -> Vec<u32> {
        let m = 1u32 << self.bps;
        let mut prev = 0u32;
        let mut out = Vec::with_capacity(phases.len());
        for &p in phases {
            out.push((p + m - prev) % m);
            prev = p;
        }
        out
    }

    pub fn modulate(&self, symbols: &[u32]) -> Vec<Sample> {
        let m = 1u32 << self.bps;
        let abs_phase = self.diff_encode(symbols);
        // Raised-cosine half-sine envelope across each symbol for spectral
        // containment; BPSK phase reversals pass through an amplitude null.
        let mut out = Vec::with_capacity(symbols.len() * self.sps);
        let mut t = 0u64;
        for &ph_idx in &abs_phase {
            let sym_phase = TAU * ph_idx as f32 / m as f32;
            for k in 0..self.sps {
                let env = 0.5 - 0.5 * (TAU * k as f32 / self.sps as f32).cos();
                let carrier = TAU * self.carrier_hz * t as f32 / self.rate + sym_phase;
                out.push(env * carrier.cos());
                t += 1;
            }
        }
        out
    }
}

/// 2-FSK with a selectable shift; mark/space tones sit at `center ± shift/2`
/// (RTTY/NAVTEX). Continuous phase. `bit = true` => mark.
pub struct Fsk2 {
    rate: f32,
    sps: usize,
    mark_hz: f32,
    space_hz: f32,
}

impl Fsk2 {
    pub fn new(rate: f32, sps: usize, center_hz: f32, shift_hz: f32) -> Self {
        Fsk2 {
            rate,
            sps,
            mark_hz: center_hz + shift_hz / 2.0,
            space_hz: center_hz - shift_hz / 2.0,
        }
    }

    pub fn mark_hz(&self) -> f32 {
        self.mark_hz
    }

    pub fn space_hz(&self) -> f32 {
        self.space_hz
    }

    pub fn modulate(&self, bits: &[bool]) -> Vec<Sample> {
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(bits.len() * self.sps);
        for &b in bits {
            let f = if b { self.mark_hz } else { self.space_hz };
            let dp = TAU * f / self.rate;
            for _ in 0..self.sps {
                phase += dp;
                if phase > TAU {
                    phase -= TAU;
                }
                out.push(phase.sin());
            }
        }
        out
    }
}

/// Bell 202 AFSK (1200 Hz mark / 2200 Hz space), continuous-phase, 1200 baud.
pub struct Afsk {
    rate: f32,
    sps: usize,
    mark_hz: f32,
    space_hz: f32,
}

impl Afsk {
    /// Bell 202 default. `baud` and tones are fixed for AX.25 1200.
    pub fn bell202(rate: f32) -> Self {
        let sps = (rate / 1200.0).round() as usize;
        Afsk { rate, sps, mark_hz: 1200.0, space_hz: 2200.0 }
    }

    pub fn new(rate: f32, baud: f32, mark_hz: f32, space_hz: f32) -> Self {
        Afsk { rate, sps: (rate / baud).round() as usize, mark_hz, space_hz }
    }

    pub fn modulate(&self, bits: &[bool]) -> Vec<Sample> {
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(bits.len() * self.sps);
        for &b in bits {
            let f = if b { self.mark_hz } else { self.space_hz };
            let dp = TAU * f / self.rate;
            for _ in 0..self.sps {
                phase += dp;
                if phase > TAU {
                    phase -= TAU;
                }
                out.push(phase.sin());
            }
        }
        out
    }
}

/// CW keyer: raised-edge on/off keying of a tone. WPM sets the dit length via
/// PARIS timing (1 WPM => 1.2 s/dit).
pub struct CwKeyer {
    rate: f32,
    tone_hz: f32,
    dit_samples: usize,
    /// Raised-cosine rise/fall length in samples (key-click suppression).
    edge_samples: usize,
}

impl CwKeyer {
    pub fn new(rate: f32, tone_hz: f32, wpm: f32) -> Self {
        let dit_s = 1.2 / wpm;
        let dit_samples = (dit_s * rate).round() as usize;
        // 5 ms raised edges.
        let edge_samples = ((0.005 * rate).round() as usize).min(dit_samples / 2);
        CwKeyer { rate, tone_hz, dit_samples, edge_samples }
    }

    pub fn dit_samples(&self) -> usize {
        self.dit_samples
    }

    /// Key the tone for `elements` dit-lengths with raised edges.
    fn keyed(&self, phase: &mut f32, elements: usize, out: &mut Vec<Sample>) {
        let total = elements * self.dit_samples;
        let dp = TAU * self.tone_hz / self.rate;
        for k in 0..total {
            *phase += dp;
            if *phase > TAU {
                *phase -= TAU;
            }
            let env = if k < self.edge_samples {
                0.5 - 0.5 * (std::f32::consts::PI * k as f32 / self.edge_samples as f32).cos()
            } else if k >= total - self.edge_samples {
                let j = total - 1 - k;
                0.5 - 0.5 * (std::f32::consts::PI * j as f32 / self.edge_samples as f32).cos()
            } else {
                1.0
            };
            out.push(env * phase.sin());
        }
    }

    fn silence(&self, out: &mut Vec<Sample>, elements: usize) {
        out.extend(std::iter::repeat_n(0.0, elements * self.dit_samples));
    }

    /// Render a Morse element string: `.` dit, `-` dah (3 dits), ` ` inter-word
    /// gap. Inter-element gap (1 dit) is inserted between symbols; letters are
    /// expected pre-spaced by the caller via spaces in `morse`.
    pub fn modulate(&self, morse: &str) -> Vec<Sample> {
        let mut out = Vec::new();
        let mut phase = 0.0f32;
        let mut first = true;
        for ch in morse.chars() {
            match ch {
                '.' | '-' => {
                    if !first {
                        self.silence(&mut out, 1); // inter-element gap
                    }
                    self.keyed(&mut phase, if ch == '.' { 1 } else { 3 }, &mut out);
                    first = false;
                }
                ' ' => {
                    self.silence(&mut out, 3); // inter-word/letter gap
                    first = true;
                }
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goertzel_power(sig: &[f32], freq: f32, rate: f32) -> f32 {
        let w = TAU * freq / rate;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in sig {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    /// Instantaneous phase from a real signal via analytic-free zero-cross-safe
    /// finite difference is messy; instead bound the per-sample amplitude step
    /// by the max instantaneous phase advance. With continuous phase, every
    /// step is `|sin(p+dp)-sin(p)| <= dp_max`; a phase discontinuity at a symbol
    /// boundary would produce a step larger than `dp_max`.
    #[test]
    fn cpfsk_is_phase_continuous() {
        let rate = 12000.0;
        let (base, spacing) = (1000.0f32, 50.0f32);
        let g = Gfsk::new(rate, 32, base, spacing, 2.0);
        let symbols = [0u32, 7, 0, 3, 5, 1];
        let sig = g.modulate(&symbols);
        // Max instantaneous frequency = base + 7*spacing = 1350 Hz.
        let max_tone = base + 7.0 * spacing;
        let dp_max = TAU * max_tone / rate;
        let mut max_step = 0.0f32;
        for w in sig.windows(2) {
            max_step = max_step.max((w[1] - w[0]).abs());
        }
        assert!(
            max_step <= dp_max * 1.02,
            "discontinuity: max step {max_step} exceeds dp_max {dp_max}"
        );
    }

    #[test]
    fn gfsk_tracks_symbol_tones() {
        let sps = 64;
        let g = Gfsk::new(12000.0, sps, 1000.0, 100.0, 0.0); // unshaped => clean tones
        let sig = g.modulate(&[0; 8]); // all symbol 0 => 1000 Hz
        let p1000 = goertzel_power(&sig, 1000.0, 12000.0);
        let p1100 = goertzel_power(&sig, 1100.0, 12000.0);
        assert!(p1000 > 10.0 * p1100);
    }

    #[test]
    fn mfsk_tone_correctness() {
        let m = MFsk::new(8000.0, 100, 500.0, 200.0, 4);
        let sig = m.modulate(&[2; 10]); // tone 2 => 500 + 400 = 900 Hz
        let p900 = goertzel_power(&sig, 900.0, 8000.0);
        let p500 = goertzel_power(&sig, 500.0, 8000.0);
        assert!(p900 > 10.0 * p500);
    }

    #[test]
    fn diff_psk_round_trip() {
        let d = DiffPsk::new(8000.0, 1500.0, 16, 2);
        let bits = [0u32, 1, 3, 2, 1, 0, 3, 3];
        let enc = d.diff_encode(&bits);
        let dec = d.diff_decode(&enc);
        assert_eq!(dec, bits);
    }

    #[test]
    fn diff_bpsk_round_trip_and_modulates() {
        let d = DiffPsk::new(8000.0, 1000.0, 32, 1);
        let bits = [1u32, 1, 0, 1, 0, 0, 1];
        let dec = d.diff_decode(&d.diff_encode(&bits));
        assert_eq!(dec, bits);
        let sig = d.modulate(&bits);
        assert_eq!(sig.len(), bits.len() * 32);
        // Carrier energy present.
        assert!(goertzel_power(&sig, 1000.0, 8000.0) > 1.0);
    }

    #[test]
    fn fsk2_tones_at_center_plus_minus_half_shift() {
        let f = Fsk2::new(8000.0, 100, 1700.0, 170.0); // RTTY 170 Hz shift
        assert!((f.mark_hz() - 1785.0).abs() < 1e-3);
        assert!((f.space_hz() - 1615.0).abs() < 1e-3);
        let mark = f.modulate(&[true; 20]);
        assert!(goertzel_power(&mark, 1785.0, 8000.0) > 10.0 * goertzel_power(&mark, 1615.0, 8000.0));
    }

    #[test]
    fn afsk_bell202_continuous_and_correct_tones() {
        let a = Afsk::bell202(48000.0);
        let marks = a.modulate(&[true; 30]);
        let spaces = a.modulate(&[false; 30]);
        assert!(goertzel_power(&marks, 1200.0, 48000.0) > 10.0 * goertzel_power(&marks, 2200.0, 48000.0));
        assert!(goertzel_power(&spaces, 2200.0, 48000.0) > 10.0 * goertzel_power(&spaces, 1200.0, 48000.0));
        // Continuity across a mark->space transition.
        let mixed = a.modulate(&[true, false, true, false]);
        let mut max_step = 0.0f32;
        for w in mixed.windows(2) {
            max_step = max_step.max((w[1] - w[0]).abs());
        }
        assert!(max_step < 0.4, "afsk discontinuity {max_step}");
    }

    #[test]
    fn cw_keyer_timing_and_tone() {
        let k = CwKeyer::new(8000.0, 700.0, 20.0); // 20 WPM
        // dit = 1.2/20 = 60 ms => 480 samples at 8 kHz.
        assert_eq!(k.dit_samples(), 480);
        let sig = k.modulate("-.-"); // dah dit dah (K), with gaps
        // Tone energy at 700 Hz present; near-zero between keyed elements.
        assert!(goertzel_power(&sig, 700.0, 8000.0) > 1.0);
        // The signal returns to silence (a gap) somewhere — min abs run exists.
        assert!(sig.iter().any(|&s| s.abs() < 1e-6));
    }
}
