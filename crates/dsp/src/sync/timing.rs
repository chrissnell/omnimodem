//! Symbol-timing recovery variants.
//!
//! Three independent detectors used by different modes:
//! * [`GardnerTed`] — Gardner / early-late timing-error detector for PSK; a
//!   feedback loop drives the sampling phase to the symbol optimum.
//! * [`StartBitSync`] — asynchronous space->mark start-bit edge detector for
//!   5-bit Baudot (RTTY/NAVTEX): finds the start edge then samples the data
//!   bits at baud midpoints.
//! * [`TransitionMinimizer`] — PSK31 transition-minimum search: picks the
//!   sampling phase where successive symbol phasors differ the least.

use crate::types::Sample;

// ---------------------------------------------------------------------------
// Gardner / early-late TED (PSK)
// ---------------------------------------------------------------------------

/// Gardner timing-error detector with a proportional phase-update loop.
///
/// Fed one sample at a time; the loop keeps an interpolating phase accumulator
/// at `sps` samples/symbol. The Gardner error uses the current symbol sample,
/// the previous symbol sample, and the midpoint between them:
/// `e = mid * (prev - curr)`. The error nudges the symbol period so the strobe
/// lands at the eye center. Returns `Some(sample)` at each symbol instant.
pub struct GardnerTed {
    sps: f32,
    period: f32,
    phase: f32,
    gain: f32,
    // Previous symbol strobe and the most recent half-symbol (midpoint) value.
    prev_sym: Sample,
    mid: Sample,
    mid_taken: bool,
}

impl GardnerTed {
    pub fn new(sps: f32) -> Self {
        assert!(sps >= 2.0);
        GardnerTed {
            sps,
            period: sps,
            // Strobe at the symbol center: first fire ~half a symbol in.
            phase: sps * 0.5,
            gain: 0.02,
            prev_sym: 0.0,
            mid: 0.0,
            mid_taken: false,
        }
    }

    /// Current estimated samples-per-symbol (period), for tests/diagnostics.
    pub fn period(&self) -> f32 {
        self.period
    }

    /// Push one sample. Returns `Some(strobe)` at symbol instants.
    pub fn feed(&mut self, x: Sample) -> Option<Sample> {
        self.phase += 1.0;
        // Capture the half-symbol (midpoint) sample once per period.
        if self.phase >= self.period * 0.5 && !self.mid_taken {
            self.mid = x;
            self.mid_taken = true;
        }
        if self.phase >= self.period {
            self.phase -= self.period;
            let curr = x;
            // Gardner error: zero at optimum, signed by the timing offset.
            let err = self.mid * (self.prev_sym - curr);
            // Update the period estimate (proportional control), bounded.
            self.period -= self.gain * err;
            self.period = self.period.clamp(self.sps * 0.8, self.sps * 1.2);
            self.prev_sym = curr;
            self.mid_taken = false;
            Some(curr)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Async start-bit sync (RTTY / NAVTEX, 5-bit Baudot)
// ---------------------------------------------------------------------------

/// Asynchronous start-bit framer for 5-bit Baudot.
///
/// The line idles at `mark` (logic 1). A character begins with a `space`
/// (logic 0) start bit. On detecting the mark->space edge, the framer samples
/// the 5 data bits at baud midpoints (0.5, 1.5, ... cells after the edge) and
/// returns the 5-bit code (bit 0 first / LSB-first as on the wire).
pub struct StartBitSync {
    // Sample counter since the detected start edge; <0 means idle/searching.
    pos: f32,
    idle_level: bool, // mark
    prev: bool,
    bits: [bool; 5],
    next_bit: usize,
    // Midpoint sample offsets of the 5 data bits, measured from the start edge.
    targets: [f32; 5],
}

impl StartBitSync {
    pub fn new(sps: f32) -> Self {
        assert!(sps >= 2.0);
        // Start bit occupies cell 0; data bits 1..=5; sample at their centers.
        let targets = [1.5, 2.5, 3.5, 4.5, 5.5].map(|c: f32| c * sps);
        StartBitSync {
            pos: -1.0,
            idle_level: true,
            prev: true,
            bits: [false; 5],
            next_bit: 0,
            targets,
        }
    }

    /// Push one demodulated level (true = mark). Returns `Some([b0..b4])` when a
    /// full 5-bit character has been sampled.
    pub fn feed(&mut self, level: bool) -> Option<[bool; 5]> {
        let mut out = None;
        if self.pos < 0.0 {
            // Searching for the mark->space start edge.
            if self.prev == self.idle_level && !level {
                self.pos = 0.0;
                self.next_bit = 0;
            }
        } else {
            self.pos += 1.0;
            if self.next_bit < 5 && self.pos >= self.targets[self.next_bit] {
                self.bits[self.next_bit] = level;
                self.next_bit += 1;
                if self.next_bit == 5 {
                    out = Some(self.bits);
                    // Resume searching for the next start edge after the stop
                    // bit; reset to idle.
                    self.pos = -1.0;
                }
            }
        }
        self.prev = level;
        out
    }
}

// ---------------------------------------------------------------------------
// PSK31 transition-minimum sampling
// ---------------------------------------------------------------------------

/// PSK31 transition-minimum timing.
///
/// BPSK31 signals symbol 0 as a phase reversal and 1 as no reversal, with
/// raised-cosine envelope shaping that dips to (near) zero amplitude at symbol
/// boundaries. Sampling where the running |x| product across the symbol period
/// is *minimized* would hit the nulls; instead we track the phase bin whose
/// accumulated envelope energy is *maximal* (the eye center) over a sliding
/// histogram of `sps` bins. This locks the strobe to symbol centers without a
/// feedback loop, which suits PSK31's slow 31.25 baud rate.
pub struct TransitionMinimizer {
    sps: usize,
    bins: Vec<f32>,
    pos: usize,
    decay: f32,
}

impl TransitionMinimizer {
    pub fn new(sps: usize) -> Self {
        assert!(sps >= 2);
        TransitionMinimizer { sps, bins: vec![0.0; sps], pos: 0, decay: 0.99 }
    }

    /// Accumulate one sample's magnitude into its phase bin.
    pub fn feed(&mut self, mag: f32) {
        let b = &mut self.bins[self.pos];
        *b = *b * self.decay + mag;
        self.pos = (self.pos + 1) % self.sps;
    }

    /// Phase bin (0..sps) with maximum accumulated envelope = eye center.
    pub fn best_phase(&self) -> usize {
        let mut best = 0usize;
        let mut bv = f32::MIN;
        for (i, &v) in self.bins.iter().enumerate() {
            if v > bv {
                bv = v;
                best = i;
            }
        }
        best
    }

    /// Phase bin with minimum accumulated envelope = symbol boundary (where
    /// inter-symbol transitions occur). Sampling is avoided here.
    pub fn transition_phase(&self) -> usize {
        let mut best = 0usize;
        let mut bv = f32::MAX;
        for (i, &v) in self.bins.iter().enumerate() {
            if v < bv {
                bv = v;
                best = i;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn gardner_locks_to_bpsk_symbol_centers() {
        // Synthesize NRZ BPSK at sps=8 with a raised-ish pulse, sampled with a
        // deliberate initial phase offset. The Gardner loop should pull the
        // period so strobes track symbols; assert the period settles near the
        // true sps and strobe signs match the symbols.
        let sps = 8usize;
        let syms: Vec<f32> = (0..300)
            .map(|i| if (i * 7 + 3) % 5 < 3 { 1.0 } else { -1.0 })
            .collect();
        // Render with a half-cosine pulse so the eye has a real center.
        let mut sig = Vec::new();
        for &s in &syms {
            for n in 0..sps {
                let t = (n as f32 + 0.5) / sps as f32; // 0..1
                let env = (PI * t).sin(); // 0 at edges, 1 at center
                sig.push(s * env);
            }
        }
        let mut ted = GardnerTed::new(sps as f32);
        let mut strobes = Vec::new();
        for &x in &sig {
            if let Some(v) = ted.feed(x) {
                strobes.push(v);
            }
        }
        // Period converged near the true symbol period.
        assert!(
            (ted.period() - sps as f32).abs() < 1.0,
            "period {} far from {sps}",
            ted.period()
        );
        // Strobes have large magnitude (near eye center, not the nulls).
        let big = strobes.iter().filter(|v| v.abs() > 0.5).count();
        assert!(big as f32 > 0.6 * strobes.len() as f32, "strobes near nulls");
    }

    #[test]
    fn start_bit_sync_samples_five_data_bits() {
        let sps = 6.0;
        // Idle mark, start (space), data 1,0,1,1,0 (LSB-first), stop (mark).
        let data = [true, false, true, true, false];
        let mut levels = Vec::new();
        levels.extend(std::iter::repeat_n(true, 10)); // idle
        levels.extend(std::iter::repeat_n(false, sps as usize)); // start bit
        for &b in &data {
            levels.extend(std::iter::repeat_n(b, sps as usize));
        }
        levels.extend(std::iter::repeat_n(true, 2 * sps as usize)); // stop/idle
        let mut sync = StartBitSync::new(sps);
        let mut got = None;
        for &l in &levels {
            if let Some(c) = sync.feed(l) {
                got = Some(c);
            }
        }
        assert_eq!(got, Some(data));
    }

    #[test]
    fn start_bit_sync_decodes_two_chars() {
        let sps = 8.0;
        let render = |data: [bool; 5], v: &mut Vec<bool>| {
            v.extend(std::iter::repeat_n(false, sps as usize)); // start
            for &b in &data {
                v.extend(std::iter::repeat_n(b, sps as usize));
            }
            v.extend(std::iter::repeat_n(true, sps as usize)); // stop
        };
        let a = [false, true, false, true, true];
        let b = [true, true, false, false, false];
        let mut levels = vec![true; 8];
        render(a, &mut levels);
        render(b, &mut levels);
        levels.extend(std::iter::repeat_n(true, 8));
        let mut sync = StartBitSync::new(sps);
        let mut chars = Vec::new();
        for &l in &levels {
            if let Some(c) = sync.feed(l) {
                chars.push(c);
            }
        }
        assert_eq!(chars, vec![a, b]);
    }

    #[test]
    fn transition_minimizer_finds_eye_center() {
        // Envelope peaks at phase=sps/2 and dips at the boundary (phase 0).
        let sps = 16usize;
        let mut tm = TransitionMinimizer::new(sps);
        for sym in 0..200 {
            let _ = sym;
            for n in 0..sps {
                let t = n as f32 / sps as f32;
                // |sin| envelope: zero at n=0, max at n=sps/2.
                let mag = (PI * t).sin().abs();
                tm.feed(mag);
            }
        }
        let center = tm.best_phase();
        // Allow +-2 bins of slack.
        assert!(
            (center as i32 - (sps as i32 / 2)).abs() <= 2,
            "eye center bin {center} not near {}",
            sps / 2
        );
        assert!(tm.transition_phase() <= 1 || tm.transition_phase() >= sps - 1);
    }
}
