//! Costas *array* generation and correlation (distinct from the Costas loop).
//!
//! A Costas array is a permutation used as a frequency-hopping sync pattern
//! with an ideally thumbtack autocorrelation. FT8 uses the canonical 7-tone
//! array `[3,1,4,0,6,5,2]` repeated three times (symbols 0-6, 36-42, 72-78 of
//! a 79-symbol frame); FT4 uses a 4-tone array repeated four times. The
//! [`CostasCorrelator`] slides the expected hop pattern over an 8-FSK
//! tone-energy matrix and reports the time/frequency offset where the sync
//! metric peaks.

/// The canonical FT8 7×7 Costas array (WSJT-X / ft8_lib `constants.c`).
pub fn ft8_costas() -> [usize; 7] {
    [3, 1, 4, 0, 6, 5, 2]
}

/// The FT4 4×4 Costas array used at the frame corners (WSJT-X).
pub fn ft4_costas() -> [usize; 4] {
    [0, 1, 3, 2]
}

/// Generate a Costas array of order `n` via the Welch construction.
///
/// Requires `p = n + 1` to be prime. Pick a primitive root `g` mod `p`; the
/// array of length `n` defined by `a[i] = (g^(i+1) mod p) - 1` is a Costas
/// permutation of `0..n`. Returns `None` when `n + 1` is not prime (e.g.
/// order 7 has `p = 8`, so no Welch array of that order exists).
pub fn welch_costas(n: usize) -> Option<Vec<usize>> {
    // Welch array has length p-1 for prime p; so order n requires p = n+1 prime.
    let p = n + 1;
    if !is_prime(p as u64) {
        return None;
    }
    let g = primitive_root(p as u64)?;
    let mut arr = Vec::with_capacity(n);
    let mut acc = 1u64;
    for _ in 0..n {
        acc = (acc * g) % p as u64;
        arr.push((acc as usize) - 1);
    }
    Some(arr)
}

fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    let mut i = 2u64;
    while i * i <= n {
        if n.is_multiple_of(i) {
            return false;
        }
        i += 1;
    }
    true
}

fn primitive_root(p: u64) -> Option<u64> {
    if p == 2 {
        return Some(1);
    }
    let phi = p - 1;
    let factors = prime_factors(phi);
    (2..p).find(|&g| {
        factors
            .iter()
            .all(|&f| mod_pow(g, phi / f, p) != 1)
    })
}

fn prime_factors(mut n: u64) -> Vec<u64> {
    let mut f = Vec::new();
    let mut d = 2u64;
    while d * d <= n {
        if n.is_multiple_of(d) {
            f.push(d);
            while n.is_multiple_of(d) {
                n /= d;
            }
        }
        d += 1;
    }
    if n > 1 {
        f.push(n);
    }
    f
}

fn mod_pow(mut base: u64, mut exp: u64, modu: u64) -> u64 {
    let mut r = 1u64;
    base %= modu;
    while exp > 0 {
        if exp & 1 == 1 {
            r = r * base % modu;
        }
        exp >>= 1;
        base = base * base % modu;
    }
    r
}

/// Correlator for a repeated Costas array over an M-FSK tone-energy matrix.
///
/// The matrix is `energy[t][k]`: the energy of tone `k` at symbol slot `t`.
/// The pattern is the Costas array placed at each of `group_starts` (in symbol
/// units). [`CostasCorrelator::best`] sweeps candidate time shifts and tone
/// (frequency) shifts and returns the `(time_shift, freq_shift, metric)` with
/// the peak summed energy.
pub struct CostasCorrelator {
    array: Vec<usize>,
    group_starts: Vec<usize>,
    n_tones: usize,
}

impl CostasCorrelator {
    pub fn new(array: Vec<usize>, group_starts: Vec<usize>, n_tones: usize) -> Self {
        assert!(!array.is_empty());
        assert!(*array.iter().max().unwrap() < n_tones);
        CostasCorrelator { array, group_starts, n_tones }
    }

    /// FT8: 7-tone array at symbol groups {0, 36, 72}, 8 tones.
    pub fn ft8() -> Self {
        CostasCorrelator::new(ft8_costas().to_vec(), vec![0, 36, 72], 8)
    }

    /// FT4: 4-tone array at the four frame corners (4 groups), 4 tones.
    pub fn ft4(group_starts: Vec<usize>) -> Self {
        CostasCorrelator::new(ft4_costas().to_vec(), group_starts, 4)
    }

    /// Sync metric for a given integer time/frequency shift over `energy`.
    /// `energy[t][k]`, `t` indexing symbol slots, `k` indexing the `n_tones`
    /// bins. Out-of-range lookups contribute zero.
    pub fn metric(&self, energy: &[Vec<f32>], time_shift: isize, freq_shift: isize) -> f32 {
        let n_t = energy.len() as isize;
        let mut sum = 0.0f32;
        for &start in &self.group_starts {
            for (i, &tone) in self.array.iter().enumerate() {
                let t = start as isize + i as isize + time_shift;
                let k = tone as isize + freq_shift;
                if t >= 0 && t < n_t && k >= 0 && (k as usize) < self.n_tones {
                    sum += energy[t as usize][k as usize];
                }
            }
        }
        sum
    }

    /// Sweep time shifts in `t_range` and tone shifts in `f_range`; return the
    /// `(time_shift, freq_shift, metric)` of the peak.
    pub fn best(
        &self,
        energy: &[Vec<f32>],
        t_range: std::ops::RangeInclusive<isize>,
        f_range: std::ops::RangeInclusive<isize>,
    ) -> (isize, isize, f32) {
        let mut best = (0isize, 0isize, f32::MIN);
        for ts in t_range.clone() {
            for fs in f_range.clone() {
                let m = self.metric(energy, ts, fs);
                if m > best.2 {
                    best = (ts, fs, m);
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ft8_array_is_canonical() {
        assert_eq!(ft8_costas(), [3, 1, 4, 0, 6, 5, 2]);
    }

    #[test]
    fn welch_construction_is_a_valid_costas_array() {
        // Order 6 (p=7) is the classic Welch example.
        let a = welch_costas(6).expect("p=7 prime");
        assert_eq!(a.len(), 6);
        // Permutation of 0..6.
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3, 4, 5]);
        // Costas property: all displacement vectors distinct (no repeated
        // (dt, df)).
        let mut seen = std::collections::HashSet::new();
        for dt in 1..a.len() {
            for i in 0..(a.len() - dt) {
                let df = a[i + dt] as isize - a[i] as isize;
                assert!(seen.insert((dt, df)), "repeated displacement");
            }
            seen.clear();
        }
        // Non-prime order returns None.
        assert!(welch_costas(8).is_none());
    }

    /// Build an 8-FSK energy matrix of `n_sym` slots with the FT8 Costas groups
    /// injected at `(true_t, true_f)`, then add a flat noise floor.
    fn synth_ft8_energy(n_sym: usize, true_t: isize, true_f: isize, noise: f32) -> Vec<Vec<f32>> {
        let mut e = vec![vec![noise; 8]; n_sym];
        let arr = ft8_costas();
        for &start in &[0usize, 36, 72] {
            for (i, &tone) in arr.iter().enumerate() {
                let t = start as isize + i as isize + true_t;
                let k = tone as isize + true_f;
                if t >= 0 && (t as usize) < n_sym && (0..8).contains(&k) {
                    e[t as usize][k as usize] += 10.0; // strong tone
                }
            }
        }
        e
    }

    #[test]
    fn ft8_correlator_peaks_at_true_offset() {
        let true_t = 2isize;
        let true_f = 1isize;
        // 79 symbols + room for the time shift.
        let energy = synth_ft8_energy(85, true_t, true_f, 0.3);
        let corr = CostasCorrelator::ft8();
        let (ts, fs, _m) = corr.best(&energy, -3..=5, -2..=3);
        assert_eq!((ts, fs), (true_t, true_f));
    }

    #[test]
    fn ft8_metric_strictly_higher_at_truth_than_neighbors() {
        let energy = synth_ft8_energy(85, 0, 0, 0.3);
        let corr = CostasCorrelator::ft8();
        let truth = corr.metric(&energy, 0, 0);
        assert!(truth > corr.metric(&energy, 1, 0));
        assert!(truth > corr.metric(&energy, 0, 1));
        assert!(truth > corr.metric(&energy, -1, 0));
    }

    #[test]
    fn ft4_correlator_peaks_at_true_offset() {
        // 4-tone array at 4 corners of a short frame.
        let starts = vec![0usize, 10, 20, 30];
        let arr = ft4_costas();
        let n_sym = 40;
        let (tt, tf) = (1isize, 1isize);
        let mut e = vec![vec![0.5f32; 4]; n_sym];
        for &s in &starts {
            for (i, &tone) in arr.iter().enumerate() {
                let t = s as isize + i as isize + tt;
                let k = tone as isize + tf;
                if t >= 0 && (t as usize) < n_sym && (0..4).contains(&k) {
                    e[t as usize][k as usize] += 8.0;
                }
            }
        }
        let corr = CostasCorrelator::ft4(starts);
        let (ts, fs, _) = corr.best(&e, -2..=3, -1..=2);
        assert_eq!((ts, fs), (tt, tf));
    }
}
