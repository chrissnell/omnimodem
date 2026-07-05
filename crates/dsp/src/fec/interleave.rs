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

/// fldigi's MFSK/PSK-R diagonal (de)interleaver (`size = 2`): a cascade of
/// `depth` 2×2 delay stages. Each `symbols` call feeds one pair through all
/// stages in place; in the forward direction one bit of the pair passes through
/// while the other is delayed by `depth` pairs, and the reverse direction swaps
/// which is delayed, so a forward→reverse round-trip recovers the input delayed
/// by `depth`. Generic over the cell type so TX runs it on bits (`u8`) and RX on
/// soft LLRs (`f32`, `fill = 0.0` = erasure). ref: fldigi src/mfsk/interleave.cxx.
pub struct DiagInterleaver<T: Copy> {
    depth: usize,
    fwd: bool,
    table: Vec<T>, // depth * 2 * 2, indexed [k][i][j] = 4k + 2i + j
}

impl<T: Copy> DiagInterleaver<T> {
    /// `fill` seeds the delay cells (fldigi: 0 for TX, PUNCTURE for RX). `fwd`
    /// selects interleave (TX) vs de-interleave (RX).
    pub fn new(depth: usize, fwd: bool, fill: T) -> Self {
        DiagInterleaver { depth, fwd, table: vec![fill; depth * 4] }
    }

    /// Interleave (or de-interleave) one pair in place. ref: interleave::symbols.
    pub fn symbols(&mut self, psyms: &mut [T; 2]) {
        for k in 0..self.depth {
            let base = k * 4;
            // Shift each 2-cell row left, then insert the input into the last col.
            self.table[base] = self.table[base + 1];
            self.table[base + 2] = self.table[base + 3];
            self.table[base + 1] = psyms[0];
            self.table[base + 3] = psyms[1];
            // Read out on the (forward) anti-diagonal / (reverse) main diagonal.
            if self.fwd {
                psyms[0] = self.table[base + 1]; // row 0, col 1-0
                psyms[1] = self.table[base + 2]; // row 1, col 0
            } else {
                psyms[0] = self.table[base]; // row 0, col 0
                psyms[1] = self.table[base + 3]; // row 1, col 1
            }
        }
    }
}

impl DiagInterleaver<u8> {
    /// Interleave the two low bits of `pbits` (bit1 = MSB, bit0 = LSB) in place,
    /// matching fldigi `interleave::bits`. ref: interleave.cxx:78-88.
    pub fn bits(&mut self, pbits: &mut u32) {
        let mut syms = [((*pbits >> 1) & 1) as u8, (*pbits & 1) as u8];
        self.symbols(&mut syms);
        *pbits = ((syms[0] as u32) << 1) | syms[1] as u32;
    }
}

/// fldigi's MFSK diagonal (de)interleaver — the `size`-parametric generalisation
/// of [`DiagInterleaver`] (which is this with `size == 2`, PSK-R). One instance
/// holds a `size × size × depth` table of delay cells; each `symbols` call shifts
/// every `depth` block left one column, inserts the input as the new last column,
/// and reads out the forward anti-diagonal (`tab(k,i,size-i-1)`) or reverse main
/// diagonal (`tab(k,i,i)`). The MFSK family sets `size == symbits` (3/4/5) and
/// `depth` per submode (5/10/20/400/800). A forward→reverse round-trip recovers
/// the input delayed by the interleaver's fill latency. Generic over the cell
/// type so TX runs on hard bits (`u8`) and a soft RX could run on LLRs (`f32`).
/// ref: fldigi src/mfsk/interleave.cxx:57-90, src/include/interleave.h:41-43.
pub struct MfskInterleaver<T: Copy> {
    size: usize,
    depth: usize,
    fwd: bool,
    table: Vec<T>, // size*size*depth, indexed [k][i][j] = size*size*k + size*i + j
}

impl<T: Copy> MfskInterleaver<T> {
    /// `fill` seeds the delay cells (fldigi: 0 for TX, PUNCTURE for RX). `fwd`
    /// selects interleave (TX) vs de-interleave (RX).
    pub fn new(size: usize, depth: usize, fwd: bool, fill: T) -> Self {
        MfskInterleaver { size, depth, fwd, table: vec![fill; size * size * depth] }
    }

    /// Interleave (or de-interleave) one column of `size` symbols in place.
    /// ref: interleave::symbols (interleave.cxx:57-76).
    pub fn symbols(&mut self, psyms: &mut [T]) {
        let size = self.size;
        debug_assert_eq!(psyms.len(), size);
        for k in 0..self.depth {
            let base = k * size * size;
            // Shift each of the `size` rows left one column.
            for i in 0..size {
                let row = base + i * size;
                for j in 0..size - 1 {
                    self.table[row + j] = self.table[row + j + 1];
                }
            }
            // Insert the input as the new last column.
            for i in 0..size {
                self.table[base + i * size + (size - 1)] = psyms[i];
            }
            // Read out on the (forward) anti-diagonal / (reverse) main diagonal.
            for i in 0..size {
                let col = if self.fwd { size - i - 1 } else { i };
                psyms[i] = self.table[base + i * size + col];
            }
        }
    }
}

impl MfskInterleaver<u8> {
    /// Interleave the low `size` bits of `pbits` (bit `size-1` = MSB … bit 0 =
    /// LSB) in place, matching fldigi `interleave::bits`. ref: interleave.cxx:78-90.
    pub fn bits(&mut self, pbits: &mut u32) {
        let size = self.size;
        let mut syms = vec![0u8; size];
        for (i, s) in syms.iter_mut().enumerate() {
            *s = ((*pbits >> (size - i - 1)) & 1) as u8;
        }
        self.symbols(&mut syms);
        *pbits = 0;
        for &s in &syms {
            *pbits = (*pbits << 1) | s as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-exact: `MfskInterleaver` reproduces fldigi's `interleave(symbits,depth)`
    /// symbol stream. Re-derives the golden `symbols` field from the golden `coded`
    /// bits for every representative submode, isolating the interleaver from the
    /// varicode/FEC/gray stages. Provenance: `tests/vectors/mfsk.json` (fldigi
    /// 4.1.23 @ 61b97f413, driver `scratch/refvectors/build_mfsk.sh`).
    #[test]
    fn mfsk_interleaver_matches_fldigi_vector() {
        let raw = include_str!("../../tests/vectors/mfsk.json");
        for line in raw.lines().filter(|l| l.contains("\"tones\"")) {
            let field = |k: &str| {
                let i = line.find(k).unwrap() + k.len();
                line[i..line[i..].find('"').unwrap() + i].to_string()
            };
            let num = |k: &str| -> usize {
                let i = line.find(k).unwrap() + k.len();
                line[i..].split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap()
            };
            let symbits = num("\"symbits\":");
            let depth = num("\"depth\":");
            let coded: Vec<u8> = field("\"coded\":\"").bytes().map(|c| c - b'0').collect();
            let want: Vec<u32> =
                field("\"symbols\":\"").split(' ').map(|s| s.parse().unwrap()).collect();

            let mut il = MfskInterleaver::new(symbits, depth, true, 0u8);
            let mut got = Vec::new();
            let (mut shreg, mut state) = (0u32, 0usize);
            for &cb in &coded {
                shreg = (shreg << 1) | cb as u32;
                state += 1;
                if state == symbits {
                    il.bits(&mut shreg);
                    got.push(shreg);
                    shreg = 0;
                    state = 0;
                }
            }
            assert_eq!(got, want, "mfsk interleaver differs from fldigi (symbits={symbits})");
        }
    }

    /// Bit-exact: `DiagInterleaver` forward output reproduces fldigi's
    /// `interleave(2, 40, FWD).bits()` sequence byte-for-byte, and a
    /// forward→reverse round-trip recovers the input delayed by `depth`.
    /// Provenance: `tests/vectors/psk_interleave.json` (fldigi 4.1.23 @
    /// 61b97f413, driver `scratch/refvectors/build_psk_interleave.sh`).
    #[test]
    fn diag_interleaver_matches_fldigi_vector() {
        let raw = include_str!("../../tests/vectors/psk_interleave.json");
        let line = raw.lines().find(|l| l.contains("\"fwd\"")).unwrap();
        let field = |k: &str| {
            let i = line.find(k).unwrap() + k.len();
            line[i..line[i..].find('"').unwrap() + i].to_string()
        };
        let parse = |s: String| -> Vec<u32> { s.split(' ').map(|x| x.parse().unwrap()).collect() };
        let input = parse(field("\"in\":\""));
        let want_fwd = parse(field("\"fwd\":\""));

        let mut fwd = DiagInterleaver::new(40, true, 0u8);
        let got: Vec<u32> = input
            .iter()
            .map(|&v| {
                let mut b = v;
                fwd.bits(&mut b);
                b
            })
            .collect();
        assert_eq!(got, want_fwd, "forward interleave differs from fldigi");

        // Round-trip: forward then reverse recovers the input delayed by depth.
        let mut f = DiagInterleaver::new(40, true, 0u8);
        let mut r = DiagInterleaver::new(40, false, 0u8);
        let out: Vec<u32> = input
            .iter()
            .map(|&v| {
                let mut b = v;
                f.bits(&mut b);
                r.bits(&mut b);
                b
            })
            .collect();
        assert_eq!(&out[40..], &input[..input.len() - 40], "round-trip delay != depth");
    }

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
        for x in data.iter().copied().chain(std::iter::repeat_n(0, latency)) {
            out.push(dl.push(il.push(x)));
        }
        // output is the input delayed by `latency`
        assert_eq!(&out[latency..latency + data.len()], &data[..]);
    }
}
