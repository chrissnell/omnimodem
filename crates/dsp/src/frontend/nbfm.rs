//! Narrowband-FM receiver façade: raw IQ at the capture rate to demodulated
//! audio at the channel rate. Composes the NCO channel-select, complex
//! decimation, FM discriminator, and power squelch so the daemon backend calls
//! one object. `retune` moves the listening offset within the captured band;
//! `set_squelch` swaps the squelch at runtime.

use crate::frontend::detector::FmDiscriminator;
use crate::frontend::nco::DownConverter;
use crate::frontend::resample::ComplexResampler;
use crate::frontend::squelch::PowerSquelch;
use crate::types::{Cplx, Sample};

pub struct NbfmReceiver {
    nco: DownConverter,
    decim: ComplexResampler,
    disc: FmDiscriminator,
    squelch: PowerSquelch,
    gain: f32,
}

impl NbfmReceiver {
    /// `offset_hz` is the signal's offset from the captured band center.
    /// `deviation_hz` sets the audio scaling (±deviation → ~±1.0 full scale).
    pub fn new(
        capture_rate: u32,
        channel_rate: u32,
        offset_hz: f32,
        deviation_hz: f32,
        squelch: PowerSquelch,
    ) -> Self {
        NbfmReceiver {
            nco: DownConverter::new(offset_hz, capture_rate as f32),
            decim: ComplexResampler::new(capture_rate, channel_rate, 16),
            disc: FmDiscriminator::new(),
            squelch,
            gain: channel_rate as f32 / (std::f32::consts::TAU * deviation_hz),
        }
    }

    pub fn retune(&mut self, offset_hz: f32) {
        self.nco.retune(offset_hz);
    }

    pub fn set_squelch(&mut self, squelch: PowerSquelch) {
        self.squelch = squelch;
    }

    /// IQ at capture rate -> mono audio at channel rate. Silent while squelched.
    pub fn push_iq(&mut self, iq: &[Cplx]) -> Vec<Sample> {
        let mixed: Vec<Cplx> = iq.iter().map(|&z| self.nco.push_cplx(z)).collect();
        let chan = self.decim.process(&mixed);
        let open = self.squelch.observe(&chan);
        let mut audio: Vec<Sample> =
            chan.iter().map(|&z| self.disc.push(z) * self.gain).collect();
        if !open {
            for s in audio.iter_mut() {
                *s = 0.0;
            }
        }
        audio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build IQ of an FM carrier at `offset_hz` modulated by a `tone_hz` sine.
    fn fm_iq(
        capture_rate: f32,
        offset_hz: f32,
        tone_hz: f32,
        dev_hz: f32,
        n: usize,
    ) -> Vec<Cplx> {
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let t = k as f32 / capture_rate;
            let inst = offset_hz + dev_hz * (std::f32::consts::TAU * tone_hz * t).sin();
            phase += std::f32::consts::TAU * inst / capture_rate;
            out.push(Cplx::new(phase.cos(), phase.sin()));
        }
        out
    }

    #[test]
    fn recovers_modulating_tone() {
        let capture = 240_000.0;
        let channel = 48_000;
        let tone = 1200.0;
        let mut rx = NbfmReceiver::new(
            capture as u32,
            channel,
            30_000.0, // signal 30 kHz off center
            5_000.0,
            PowerSquelch::disabled(),
        );
        let iq = fm_iq(capture, 30_000.0, tone, 5_000.0, 48_000);
        let audio = rx.push_iq(&iq);
        // Zero-crossing count → dominant frequency ≈ tone_hz.
        let mut crossings = 0;
        for w in audio.windows(2) {
            if w[0] <= 0.0 && w[1] > 0.0 {
                crossings += 1;
            }
        }
        let secs = audio.len() as f32 / channel as f32;
        let est_hz = crossings as f32 / secs;
        assert!((est_hz - tone).abs() < 150.0, "estimated {est_hz} Hz, want {tone}");
    }

    #[test]
    fn squelch_silences_weak_input() {
        let mut rx = NbfmReceiver::new(
            240_000,
            48_000,
            30_000.0,
            5_000.0,
            PowerSquelch::new(-20.0, 6.0), // needs a strong signal to open
        );
        // Near-silent IQ (tiny noise) → squelch stays closed → all-zero audio.
        let iq: Vec<Cplx> = (0..48_000).map(|_| Cplx::new(1e-4, -1e-4)).collect();
        let audio = rx.push_iq(&iq);
        assert!(audio.iter().all(|&s| s == 0.0));
    }
}
