//! Candidate finder: sweep a passband over a short-time spectrum and return
//! sync-metric-sorted candidates.
//!
//! Wideband decoders (FT8, multi-signal packet) first locate where in the
//! time/frequency plane signals sit, then hand each candidate to a per-signal
//! demodulator. This module is deliberately self-contained — it computes its
//! own short-time bin energies with a Goertzel scan rather than depending on
//! `crate::frontend::stft`, so the `sync` module builds and tests in isolation.
//!
//! The metric for each (time, frequency) cell is its energy normalized by the
//! local per-frame noise floor (a robust median-ish estimate), i.e. an SNR
//! proxy. Candidates are de-duplicated to local maxima and returned sorted by
//! descending metric, so injected tones come back ranked by SNR.

use crate::types::Sample;

/// A located signal candidate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    /// Center frequency, Hz.
    pub freq: f32,
    /// Start time of the frame the candidate was found in, seconds.
    pub time: f32,
    /// Sync metric (SNR proxy, higher is stronger).
    pub metric: f32,
}

/// Configuration for the candidate sweep.
pub struct CandidateFinder {
    fs: f32,
    frame_len: usize,
    hop: usize,
    /// Passband edges in Hz.
    f_lo: f32,
    f_hi: f32,
    /// Frequency bin spacing in Hz.
    df: f32,
    /// Minimum metric to report.
    min_metric: f32,
}

impl CandidateFinder {
    pub fn new(fs: f32, frame_len: usize, hop: usize, f_lo: f32, f_hi: f32, df: f32) -> Self {
        assert!(frame_len >= 8 && hop >= 1 && df > 0.0 && f_hi > f_lo);
        CandidateFinder { fs, frame_len, hop, f_lo, f_hi, df, min_metric: 3.0 }
    }

    pub fn with_min_metric(mut self, m: f32) -> Self {
        self.min_metric = m;
        self
    }

    fn bins(&self) -> Vec<f32> {
        let n = ((self.f_hi - self.f_lo) / self.df).floor() as usize + 1;
        (0..n).map(|i| self.f_lo + i as f32 * self.df).collect()
    }

    /// Goertzel power of `freq` over one frame (Hann-windowed).
    fn goertzel(&self, frame: &[Sample], freq: f32) -> f32 {
        let n = frame.len();
        let k = freq / self.fs; // cycles/sample
        let w = std::f32::consts::TAU * k;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for (i, &x) in frame.iter().enumerate() {
            // Hann window to control leakage.
            let win = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
            let s0 = coeff * s1 - s2 + x * win;
            s2 = s1;
            s1 = s0;
        }
        // Power = |X|^2.
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)
    }

    /// Find candidates across `signal`. Returns local maxima sorted by
    /// descending metric.
    pub fn find(&self, signal: &[Sample]) -> Vec<Candidate> {
        let bins = self.bins();
        if signal.len() < self.frame_len {
            return Vec::new();
        }
        // energy[frame][bin]
        let mut grid: Vec<Vec<f32>> = Vec::new();
        let mut start = 0usize;
        while start + self.frame_len <= signal.len() {
            let frame = &signal[start..start + self.frame_len];
            let row: Vec<f32> = bins.iter().map(|&f| self.goertzel(frame, f)).collect();
            grid.push(row);
            start += self.hop;
        }

        // Per-frame noise floor = median of that frame's bin powers; the metric
        // is bin_power / noise_floor (SNR proxy).
        let mut cands = Vec::new();
        for (fi, row) in grid.iter().enumerate() {
            let mut sorted = row.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let floor = sorted[sorted.len() / 2].max(1e-12);
            for (bi, &p) in row.iter().enumerate() {
                let metric = p / floor;
                if metric < self.min_metric {
                    continue;
                }
                // Local maximum in frequency (and not a window-leakage skirt).
                let left = if bi > 0 { row[bi - 1] } else { 0.0 };
                let right = if bi + 1 < row.len() { row[bi + 1] } else { 0.0 };
                if p >= left && p >= right {
                    cands.push(Candidate {
                        freq: bins[bi],
                        time: fi as f32 * self.hop as f32 / self.fs,
                        metric,
                    });
                }
            }
        }

        // Collapse candidates that are adjacent in time at the same frequency
        // to a single strongest entry, then sort by metric.
        cands.sort_by(|a, b| {
            b.metric
                .partial_cmp(&a.metric)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut deduped: Vec<Candidate> = Vec::new();
        for c in cands {
            if !deduped
                .iter()
                .any(|d| (d.freq - c.freq).abs() < self.df * 0.5)
            {
                deduped.push(c);
            }
        }
        deduped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn tone(buf: &mut [Sample], fs: f32, freq: f32, amp: f32) {
        for (i, s) in buf.iter_mut().enumerate() {
            *s += amp * (TAU * freq * i as f32 / fs).sin();
        }
    }

    fn awgn(buf: &mut [Sample], sigma: f32, seed: u64) {
        // Reuse the crate's reproducible Gaussian source for proper white noise.
        let mut rng = crate::testutil::Rng::new(seed);
        crate::testutil::add_awgn(buf, sigma, &mut rng);
    }

    #[test]
    fn finds_injected_tones_ranked_by_snr() {
        let fs = 8000.0;
        let n = 8192;
        let mut sig = vec![0.0f32; n];
        // Three tones in the passband with distinct amplitudes.
        tone(&mut sig, fs, 1000.0, 0.25); // weakest
        tone(&mut sig, fs, 1500.0, 0.6); // strongest
        tone(&mut sig, fs, 2000.0, 0.4); // middle
        awgn(&mut sig, 0.02, 1);

        let finder = CandidateFinder::new(fs, 1024, 512, 500.0, 2500.0, 25.0)
            .with_min_metric(4.0);
        let cands = finder.find(&sig);
        assert!(cands.len() >= 3, "expected >=3 candidates, got {}", cands.len());

        // The three strongest candidates should be our injected tones.
        let top3 = &cands[..3];
        let near = |f: f32| top3.iter().any(|c| (c.freq - f).abs() <= 30.0);
        assert!(near(1000.0) && near(1500.0) && near(2000.0), "missing tone: {top3:?}");

        // Ranked by SNR: 1500 (amp .6) > 2000 (.4) > 1000 (.25).
        let rank = |f: f32| {
            top3.iter()
                .position(|c| (c.freq - f).abs() <= 30.0)
                .unwrap()
        };
        assert!(rank(1500.0) < rank(2000.0));
        assert!(rank(2000.0) < rank(1000.0));
    }

    #[test]
    fn empty_on_short_input() {
        let finder = CandidateFinder::new(8000.0, 1024, 512, 500.0, 2500.0, 25.0);
        assert!(finder.find(&[0.0; 100]).is_empty());
    }

    #[test]
    fn rejects_pure_noise() {
        let mut sig = vec![0.0f32; 8192];
        awgn(&mut sig, 0.1, 7);
        // With no narrowband tone, no bin's SNR-over-median metric reaches the
        // high threshold an actual tone clears (tones in the sibling test sit
        // well above 30x). White noise peak/median stays modest.
        let finder = CandidateFinder::new(8000.0, 1024, 512, 500.0, 2500.0, 25.0)
            .with_min_metric(15.0);
        let cands = finder.find(&sig);
        assert!(cands.len() <= 2, "noise produced {} candidates", cands.len());
    }
}
