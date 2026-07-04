//! N parallel PSK carriers for the fldigi multi-carrier (nX) robust modes.
//!
//! Ports fldigi's carrier-spacing and per-carrier NCO summation out of the
//! monolithic modem loop into a reusable block. TX up-converts each carrier's
//! symbol phasor stream and sums them (scaled by 1/N); RX runs one down-
//! converter per carrier and returns per-carrier baseband symbol samples
//! (integrate-and-dump matched filter).
//! ref: fldigi/src/psk/psk.cxx:1057-1061 (carrier spacing), 2008-2013 (rx freqs).

use crate::frontend::nco::DownConverter;
use crate::types::{Cplx, Sample};
use std::f32::consts::TAU;

/// fldigi's carrier separation factor: inter-carrier spacing is `1.4 * sc_bw`
/// where `sc_bw = samplerate / symbollen`. ref: psk.cxx (`separation`).
const SEPARATION: f32 = 1.4;

/// A bank of `numcarriers` evenly spaced PSK carriers centered on `center_hz`.
pub struct MultiCarrier {
    rate: f32,
    freqs: Vec<f32>,
    down: Vec<DownConverter>,
    /// Per-carrier TX phase accumulators (fldigi `phaseacc[car]`).
    tx_phase: Vec<f32>,
}

impl MultiCarrier {
    pub fn new(rate: f32, center_hz: f32, symbollen: usize, numcarriers: usize) -> Self {
        // At least one carrier: `modulate_symbols` scales by 1/N, so N=0 would
        // emit NaN audio rather than fail loudly.
        assert!(numcarriers >= 1, "MultiCarrier needs at least one carrier");
        let sc_bw = rate / symbollen as f32;
        let inter = SEPARATION * sc_bw;
        // f[0] = center + ((-N)+1) * inter/2; carriers step by `inter`, centered.
        let f0 = center_hz + ((-(numcarriers as f32)) + 1.0) * inter / 2.0;
        let freqs: Vec<f32> = (0..numcarriers).map(|k| f0 + k as f32 * inter).collect();
        let down = freqs.iter().map(|&f| DownConverter::new(f, rate)).collect();
        MultiCarrier { rate, tx_phase: vec![0.0; numcarriers], freqs, down }
    }

    pub fn num_carriers(&self) -> usize {
        self.freqs.len()
    }

    pub fn carrier_hz(&self, k: usize) -> f32 {
        self.freqs[k]
    }

    /// Modulate a per-symbol slice of per-carrier phasors into audio, summed
    /// across carriers and scaled by 1/N. `symbols[s][car]` is the phasor for
    /// carrier `car` at symbol `s`; `sps` samples per symbol. Each symbol's
    /// phasor is held constant over its `sps` samples, so `demodulate` recovers
    /// it via integrate-and-dump.
    ///
    /// This block is the carrier bank only. fldigi's per-symbol TX pulse shaping
    /// (the raised-cosine `tx_shape` amplitude taper that limits phase-reversal
    /// bandwidth) is applied by the mode assembly on the phasor stream, where
    /// bit-for-bit fidelity is enforced against fldigi by the cross-decode gate —
    /// keeping it out of the block leaves this primitive cleanly invertible.
    pub fn modulate_symbols(&mut self, symbols: &[Vec<Cplx>], sps: usize) -> Vec<Sample> {
        let n = self.freqs.len();
        let mut out = vec![0.0f32; symbols.len() * sps];
        for (s, syms) in symbols.iter().enumerate() {
            // Indexes syms/freqs/tx_phase in lockstep; a range loop reads clearest.
            #[allow(clippy::needless_range_loop)]
            for car in 0..n {
                let cur = syms[car];
                let dphi = TAU * self.freqs[car] / self.rate;
                for i in 0..sps {
                    let ph = self.tx_phase[car];
                    out[s * sps + i] += (cur.re * ph.cos() + cur.im * ph.sin()) / n as f32;
                    self.tx_phase[car] += dphi;
                    if self.tx_phase[car] > TAU {
                        self.tx_phase[car] -= TAU;
                    }
                }
            }
        }
        out
    }

    /// Down-convert to per-carrier baseband and integrate-and-dump each symbol,
    /// returning `out[car][symbol]` matched-filter samples. `sps` samples/symbol.
    pub fn demodulate(&mut self, audio: &[Sample], sps: usize) -> Vec<Vec<Cplx>> {
        let n = self.freqs.len();
        let mut out = vec![Vec::with_capacity(audio.len() / sps.max(1)); n];
        let mut acc = vec![Cplx::new(0.0, 0.0); n];
        for (i, &x) in audio.iter().enumerate() {
            // Indexes acc/down in lockstep; a range loop reads clearest.
            #[allow(clippy::needless_range_loop)]
            for car in 0..n {
                acc[car] += self.down[car].push(x);
            }
            if (i + 1) % sps == 0 {
                for car in 0..n {
                    out[car].push(acc[car]);
                    acc[car] = Cplx::new(0.0, 0.0);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Cplx;

    #[test]
    fn carrier_frequencies_match_fldigi_layout() {
        // fldigi: sc_bw = samplerate/symbollen; inter = 1.4*sc_bw;
        // f[0] = center + ((-N)+1)*inter/2; f[k] = f[k-1] + inter.
        let mc = MultiCarrier::new(8000.0, 1500.0, 128, 4);
        let sc_bw = 8000.0 / 128.0; // 62.5
        let inter = 1.4 * sc_bw; // 87.5
        let f0 = 1500.0 + ((-4.0) + 1.0) * inter / 2.0; // 1500 - 131.25
        let want = [f0, f0 + inter, f0 + 2.0 * inter, f0 + 3.0 * inter];
        for (i, &w) in want.iter().enumerate() {
            assert!(
                (mc.carrier_hz(i) - w).abs() < 1e-3,
                "carrier {i}: {} != {w}",
                mc.carrier_hz(i)
            );
        }
    }

    #[test]
    fn single_carrier_up_down_roundtrips_phase() {
        // With N=1 the block is a pass-through NCO pair: a run of symbol phasors
        // modulated up then down-converted recovers the same de-rotated phase sign.
        let mut mc = MultiCarrier::new(8000.0, 1000.0, 256, 1);
        let syms = vec![vec![Cplx::new(1.0, 0.0)], vec![Cplx::new(-1.0, 0.0)]];
        let audio = mc.modulate_symbols(&syms, 256);
        let bb = mc.demodulate(&audio, 256);
        assert_eq!(bb.len(), 1);
        // consecutive symbols reversed => their dot product is negative.
        let a = bb[0][0];
        let b = bb[0][1];
        assert!(a.re * b.re + a.im * b.im < 0.0, "reversal not preserved");
    }
}
