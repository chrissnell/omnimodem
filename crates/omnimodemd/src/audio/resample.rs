//! Rational sample-rate conversion. Linear-interpolation fractional resampler:
//! deterministic, streaming-stateful, exact length ratio. Runs downstream of
//! the rate-capped capture; a polyphase windowed-sinc upgrade is a Phase-3
//! battery. Operates on i16 with internal f32 accumulation.

/// Streaming resampler from `in_rate` to `out_rate`.
pub struct RationalResampler {
    in_rate: u32,
    out_rate: u32,
    /// Fractional input position carried across `process` calls.
    pos: f64,
    /// Last input sample of the previous block, for cross-block interpolation.
    last: f32,
    primed: bool,
}

impl RationalResampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        assert!(in_rate > 0 && out_rate > 0);
        RationalResampler { in_rate, out_rate, pos: 0.0, last: 0.0, primed: false }
    }

    /// Identity fast-path predicate.
    pub fn is_passthrough(&self) -> bool {
        self.in_rate == self.out_rate
    }

    /// Resample one block. Output length is approximately
    /// `input.len() * out_rate / in_rate`.
    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        if self.is_passthrough() {
            return input.to_vec();
        }
        let step = self.in_rate as f64 / self.out_rate as f64;
        let mut out = Vec::with_capacity(
            (input.len() as u64 * self.out_rate as u64 / self.in_rate as u64) as usize + 1,
        );
        if !self.primed {
            self.last = input.first().copied().unwrap_or(0) as f32;
            self.primed = true;
        }
        // `pos` is measured in input samples relative to the start of `input`,
        // offset by carry from the previous block (negative => use `last`).
        let mut pos = self.pos;
        while pos < input.len() as f64 {
            let i = pos.floor();
            let frac = (pos - i) as f32;
            let i = i as isize;
            let a = if i < 0 { self.last } else { input[i as usize] as f32 };
            let b = if (i + 1) < 0 {
                self.last
            } else if ((i + 1) as usize) < input.len() {
                input[(i + 1) as usize] as f32
            } else {
                // Need a sample past this block; stop and carry `pos` over.
                break;
            };
            let s = a + (b - a) * frac;
            out.push(s.round().clamp(-32768.0, 32767.0) as i16);
            pos += step;
        }
        // Carry the leftover fractional position into the next block.
        self.pos = pos - input.len() as f64;
        if let Some(&l) = input.last() {
            self.last = l as f32;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_is_identity() {
        let mut r = RationalResampler::new(48_000, 48_000);
        assert!(r.is_passthrough());
        let input: Vec<i16> = (0..100).collect();
        assert_eq!(r.process(&input), input);
    }

    #[test]
    fn upsample_2x_doubles_length() {
        let mut r = RationalResampler::new(24_000, 48_000);
        let input = vec![0i16; 1000];
        let out = r.process(&input);
        // ~2x within a couple samples of boundary carry.
        assert!((out.len() as i64 - 2000).abs() <= 2, "len was {}", out.len());
    }

    #[test]
    fn downsample_2x_halves_length() {
        let mut r = RationalResampler::new(48_000, 24_000);
        let input = vec![0i16; 1000];
        let out = r.process(&input);
        assert!((out.len() as i64 - 500).abs() <= 2, "len was {}", out.len());
    }

    #[test]
    fn preserves_a_dc_level() {
        let mut r = RationalResampler::new(48_000, 16_000);
        let input = vec![1000i16; 2000];
        let out = r.process(&input);
        // Linear interp of a constant is the constant.
        assert!(out.iter().all(|&s| s == 1000), "DC not preserved");
    }

    #[test]
    fn streaming_blocks_match_one_big_block_length() {
        // Feeding two halves yields about the same total as one whole block.
        let whole: Vec<i16> = (0..2000).map(|i| (i % 100) as i16).collect();
        let mut r1 = RationalResampler::new(48_000, 32_000);
        let one = r1.process(&whole);

        let mut r2 = RationalResampler::new(48_000, 32_000);
        let mut split = r2.process(&whole[..1000]);
        split.extend(r2.process(&whole[1000..]));

        assert!((one.len() as i64 - split.len() as i64).abs() <= 2);
    }
}
