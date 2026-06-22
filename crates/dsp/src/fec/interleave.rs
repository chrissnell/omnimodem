//! Interleavers that spread burst errors across the FEC block. Each has an
//! inverse that exactly reconstructs the input. Variants: block (row-in/
//! col-out), bit-reversal (WSPR), self-synchronizing diagonal (MFSK16), and a
//! streaming depth-L convolutional interleaver (DominoEX/THOR). Generic over
//! `T: Copy + Default` so they work on hard bits or soft LLRs.
//!
//! The exact interleaver dimensions/fill orders for a given mode must be
//! confirmed against the reference (`mfsk.cxx`, `thor.cxx`, `wsprsim`) when the
//! mode is wired; these primitives provide the mechanism, the modes pick the
//! parameters.

/// Block interleaver: write `rows*cols` items row-major, read column-major.
pub fn block_interleave<T: Copy + Default>(data: &[T], rows: usize, cols: usize) -> Vec<T> {
    let mut out = vec![T::default(); rows * cols];
    for (i, &x) in data.iter().take(rows * cols).enumerate() {
        let (r, c) = (i / cols, i % cols);
        out[c * rows + r] = x;
    }
    out
}

/// Inverse block interleave: transpose back (swap rows/cols).
pub fn block_deinterleave<T: Copy + Default>(data: &[T], rows: usize, cols: usize) -> Vec<T> {
    block_interleave(data, cols, rows)
}

/// Bit-reversal interleave indices: position `i` ↔ reverse of its `ceil(log2(n))`
/// -bit index, skipping reversed values ≥ `n`. The result is a permutation of
/// `0..n`. WSPR uses this over its 162 symbols.
pub fn bit_reversal_indices(n: usize) -> Vec<usize> {
    debug_assert!(n >= 2);
    let bits = (usize::BITS - (n - 1).leading_zeros()) as usize;
    let mut idx = Vec::with_capacity(n);
    let mut j = 0usize;
    for _ in 0..(1usize << bits) {
        if j < n {
            idx.push(j);
        }
        // reverse-order increment (carry propagates from the MSB down)
        let mut bit = 1usize << (bits - 1);
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
    }
    idx
}

/// Apply a permutation: `out[k] = data[idx[k]]`.
pub fn permute<T: Copy + Default>(data: &[T], idx: &[usize]) -> Vec<T> {
    idx.iter().map(|&i| data[i]).collect()
}

/// Invert a permutation produced by [`permute`]: `out[idx[k]] = data[k]`.
pub fn unpermute<T: Copy + Default>(data: &[T], idx: &[usize]) -> Vec<T> {
    let mut out = vec![T::default(); data.len()];
    for (pos, &i) in idx.iter().enumerate() {
        out[i] = data[pos];
    }
    out
}

/// Diagonal interleaver index map over a `size`×`size` square: element written
/// at row `r`, column `c` (row-major input) is read out along shifted diagonals,
/// `read_pos = c * size + ((r + c) % size)`. Self-describing and exactly
/// invertible via [`unpermute`]. fldigi's MFSK16 interleaver is a diagonal fill
/// of this family.
pub fn diagonal_indices(size: usize) -> Vec<usize> {
    let mut idx = vec![0usize; size * size];
    for r in 0..size {
        for c in 0..size {
            let read = c * size + ((r + c) % size);
            idx[r * size + c] = read;
        }
    }
    idx
}

/// Streaming depth-L convolutional (Forney) interleaver. Branch `i` (of `rows`)
/// delays by `i * delay` elements. Pair with a [`ConvDeinterleaver`] of the same
/// shape; after a combined latency of `rows * (rows - 1) * delay` elements the
/// output reproduces the input. Used by DominoEX/THOR.
pub struct ConvInterleaver<T: Copy + Default> {
    rows: usize,
    lines: Vec<std::collections::VecDeque<T>>,
    branch: usize,
}

impl<T: Copy + Default> ConvInterleaver<T> {
    pub fn new(rows: usize, delay: usize) -> Self {
        let lines = (0..rows)
            .map(|i| std::collections::VecDeque::from(vec![T::default(); i * delay]))
            .collect();
        ConvInterleaver { rows, lines, branch: 0 }
    }

    /// Push one symbol; pop the head of the current branch's delay line.
    pub fn push(&mut self, x: T) -> T {
        let b = self.branch;
        self.lines[b].push_back(x);
        let out = self.lines[b].pop_front().unwrap();
        self.branch = (self.branch + 1) % self.rows;
        out
    }
}

/// The matching de-interleaver: branch `i` delays by `(rows-1-i) * delay`, so the
/// total delay across every branch is constant.
pub struct ConvDeinterleaver<T: Copy + Default> {
    rows: usize,
    lines: Vec<std::collections::VecDeque<T>>,
    branch: usize,
}

impl<T: Copy + Default> ConvDeinterleaver<T> {
    pub fn new(rows: usize, delay: usize) -> Self {
        let lines = (0..rows)
            .map(|i| std::collections::VecDeque::from(vec![T::default(); (rows - 1 - i) * delay]))
            .collect();
        ConvDeinterleaver { rows, lines, branch: 0 }
    }

    pub fn push(&mut self, x: T) -> T {
        let b = self.branch;
        self.lines[b].push_back(x);
        let out = self.lines[b].pop_front().unwrap();
        self.branch = (self.branch + 1) % self.rows;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_round_trips() {
        let d: Vec<u8> = (0..12).collect();
        let il = block_interleave(&d, 3, 4);
        assert_eq!(block_deinterleave(&il, 3, 4), d);
        assert_ne!(il, d, "interleave must reorder");
    }

    #[test]
    fn bit_reversal_is_a_permutation_and_round_trips() {
        for n in [8usize, 16, 162, 256] {
            let idx = bit_reversal_indices(n);
            assert_eq!(idx.len(), n);
            let mut seen = vec![false; n];
            for &i in &idx {
                assert!(i < n && !seen[i], "n={n} dup/oob {i}");
                seen[i] = true;
            }
            let d: Vec<u16> = (0..n as u16).collect();
            let il = permute(&d, &idx);
            assert_eq!(unpermute(&il, &idx), d);
        }
    }

    #[test]
    fn diagonal_round_trips_and_reorders() {
        let size = 8;
        let idx = diagonal_indices(size);
        let d: Vec<u8> = (0..(size * size) as u8).collect();
        let il = permute(&d, &idx);
        assert_eq!(unpermute(&il, &idx), d);
        assert_ne!(il, d);
        // valid permutation
        let mut seen = vec![false; size * size];
        for &i in &idx {
            assert!(!seen[i]);
            seen[i] = true;
        }
    }

    #[test]
    fn convolutional_pair_round_trips_after_latency() {
        let (rows, delay) = (4usize, 2usize);
        let mut il = ConvInterleaver::<i32>::new(rows, delay);
        let mut dl = ConvDeinterleaver::<i32>::new(rows, delay);
        let latency = rows * (rows - 1) * delay;
        let data: Vec<i32> = (1..=40).collect();
        // feed data followed by `latency` flush zeros
        let mut out = Vec::new();
        for &x in data.iter().chain(std::iter::repeat(&0).take(latency)) {
            out.push(dl.push(il.push(x)));
        }
        // output is the input delayed by `latency`
        assert_eq!(&out[latency..latency + data.len()], &data[..]);
    }
}
