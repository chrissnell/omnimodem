//! Mode-agnostic SDR receiver: raw IQ at the capture rate to demodulated audio at
//! the channel rate, for every selectable demod mode (NBFM/AM/WFM/USB/LSB). It
//! generalises Phase A's `NbfmReceiver` — one shared front-end (NCO channel-select
//! → complex decimate → power squelch) feeding a per-mode back-end — so the daemon
//! dispatches on the demod mode instead of duplicating the front-end per receiver.
//!
//! Back-ends operate on the decimated complex channel (tuned signal at DC):
//!   - **NBFM/WFM** — FM discriminator × gain, optional de-emphasis (default off).
//!   - **AM** — envelope detect + DC block.
//!   - **USB/LSB** — one-sided complex band-pass (sideband select) → real part.
//!
//! WFM needs a wider pre-detection bandwidth than the 48 kHz audio channel, so it
//! demodulates at a wide `if_rate` and resamples the recovered audio down; the
//! other modes demodulate straight at the channel rate.

use crate::frontend::detector::{DcBlock, Deemphasis, FmDiscriminator};
use crate::frontend::fir::design_lowpass;
use crate::frontend::nco::DownConverter;
use crate::frontend::resample::{ComplexResampler, Resampler};
use crate::frontend::squelch::PowerSquelch;
use crate::types::{Cplx, Sample};
use std::f32::consts::TAU;

/// The selectable demodulators. DSP-local mirror of the daemon's `DemodMode`; the
/// daemon maps its enum onto this so the DSP crate stays free of daemon types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemodKind {
    Nbfm,
    Am,
    Wfm,
    Usb,
    Lsb,
}

/// Broadcast-FM peak deviation used to scale the WFM discriminator (±75 kHz).
const WFM_DEVIATION_HZ: f32 = 75_000.0;
/// Target WFM IF (pre-detection) rate; the real rate is snapped to a multiple of
/// the channel rate that is ≤ the capture rate.
const WFM_TARGET_IF_HZ: u32 = 180_000;
/// SSB audio bandwidth (Hz); the sideband-select band-pass passes one 0..BW side.
const SSB_BANDWIDTH_HZ: f32 = 2_800.0;

/// Pick the WFM IF rate: the largest multiple of `channel_rate` that is ≤
/// `capture_rate` and at least `WFM_TARGET_IF_HZ`, falling back to the capture
/// rate. Keeping it a multiple of the audio rate makes the audio decimation a
/// clean integer ratio.
fn wfm_if_rate(capture_rate: u32, channel_rate: u32) -> u32 {
    let want_mult = WFM_TARGET_IF_HZ.div_ceil(channel_rate).max(1);
    let cap_mult = (capture_rate / channel_rate).max(1);
    let mult = want_mult.min(cap_mult);
    (channel_rate * mult).min(capture_rate)
}

/// A one-sided complex band-pass for SSB sideband selection: a real low-pass
/// prototype frequency-shifted to sit over `[0, BW]` (USB, `sign = +1`) or
/// `[-BW, 0]` (LSB, `sign = -1`). Convolving the complex channel with it isolates
/// one sideband; the real part is then the recovered audio.
struct SidebandFilter {
    /// Complex taps (prototype × e^{j·sign·2π·(BW/2)/rate·k}).
    taps: Vec<Cplx>,
    hist: Vec<Cplx>,
    pos: usize,
}

impl SidebandFilter {
    fn new(rate: f32, sign: f32) -> Self {
        // Low-pass prototype at BW/2, shifted up by BW/2 → passband [0, BW].
        let num_taps = 129;
        let proto = design_lowpass(num_taps, SSB_BANDWIDTH_HZ / 2.0, rate);
        let center = sign * SSB_BANDWIDTH_HZ / 2.0;
        let taps: Vec<Cplx> = proto
            .iter()
            .enumerate()
            .map(|(k, &h)| {
                let ph = TAU * center * k as f32 / rate;
                Cplx::new(h * ph.cos(), h * ph.sin())
            })
            .collect();
        let n = taps.len();
        SidebandFilter { taps, hist: vec![Cplx::new(0.0, 0.0); n], pos: 0 }
    }

    /// Push one channel sample; returns the real part of the filtered (sideband-
    /// isolated) output — the demodulated audio.
    fn push(&mut self, z: Cplx) -> Sample {
        let n = self.taps.len();
        self.hist[self.pos] = z;
        self.pos = (self.pos + 1) % n;
        let mut acc = Cplx::new(0.0, 0.0);
        for (k, &t) in self.taps.iter().enumerate() {
            let idx = (self.pos + n - 1 - k) % n;
            acc += t * self.hist[idx];
        }
        acc.re
    }
}

/// Per-mode back-end operating on the decimated complex channel.
enum Backend {
    /// FM discriminator × gain, with an optional de-emphasis stage.
    Fm { disc: FmDiscriminator, gain: f32, deemph: Option<Deemphasis> },
    /// Envelope detect + DC block.
    Am { dc: DcBlock },
    /// One-sided complex band-pass → real part.
    Ssb { filt: SidebandFilter },
}

impl Backend {
    fn push(&mut self, z: Cplx) -> Sample {
        match self {
            Backend::Fm { disc, gain, deemph } => {
                let a = disc.push(z) * *gain;
                match deemph {
                    Some(d) => d.push(a),
                    None => a,
                }
            }
            Backend::Am { dc } => dc.push(z.norm()),
            Backend::Ssb { filt } => filt.push(z),
        }
    }
}

/// One receiver, any mode. `push_iq` takes IQ at the capture rate and returns mono
/// audio at the channel rate, silent while squelched.
pub struct SdrDemod {
    nco: DownConverter,
    front: ComplexResampler, // capture_rate → if_rate
    squelch: PowerSquelch,
    backend: Backend,
    /// Pre-detection (back-end) rate; equals the channel rate except for WFM.
    if_rate: u32,
    /// if_rate → channel_rate for the recovered audio (only non-trivial for WFM).
    audio_decim: Option<Resampler>,
}

impl SdrDemod {
    /// `offset_hz` is the tuned signal's offset from the captured band centre;
    /// `deviation_hz` scales the NBFM discriminator output.
    pub fn new(
        kind: DemodKind,
        capture_rate: u32,
        channel_rate: u32,
        offset_hz: f32,
        deviation_hz: f32,
        squelch: PowerSquelch,
    ) -> Self {
        let if_rate = match kind {
            DemodKind::Wfm => wfm_if_rate(capture_rate, channel_rate),
            _ => channel_rate,
        };
        let backend = match kind {
            DemodKind::Nbfm => Backend::Fm {
                disc: FmDiscriminator::new(),
                gain: if_rate as f32 / (TAU * deviation_hz),
                deemph: None,
            },
            DemodKind::Wfm => Backend::Fm {
                disc: FmDiscriminator::new(),
                gain: if_rate as f32 / (TAU * WFM_DEVIATION_HZ),
                deemph: None,
            },
            DemodKind::Am => Backend::Am { dc: DcBlock::default() },
            DemodKind::Usb => Backend::Ssb { filt: SidebandFilter::new(if_rate as f32, 1.0) },
            DemodKind::Lsb => Backend::Ssb { filt: SidebandFilter::new(if_rate as f32, -1.0) },
        };
        let audio_decim = if if_rate != channel_rate {
            Some(Resampler::new(if_rate, channel_rate, 16))
        } else {
            None
        };
        SdrDemod {
            nco: DownConverter::new(offset_hz, capture_rate as f32),
            front: ComplexResampler::new(capture_rate, if_rate, 16),
            squelch,
            backend,
            if_rate,
            audio_decim,
        }
    }

    /// Enable de-emphasis on the FM back-ends (NBFM/WFM voice), with time constant
    /// `tau_us` (75 µs US, 50 µs EU). No-op for AM/SSB. Default is **off** so the
    /// data/APRS NBFM path preserves AFSK twist. Builder-style: apply after `new`.
    pub fn with_deemphasis(mut self, tau_us: f32) -> Self {
        if let Backend::Fm { deemph, .. } = &mut self.backend {
            *deemph = Some(Deemphasis::new(tau_us, self.if_rate as f32));
        }
        self
    }

    pub fn retune(&mut self, offset_hz: f32) {
        self.nco.retune(offset_hz);
    }

    pub fn set_squelch(&mut self, squelch: PowerSquelch) {
        self.squelch = squelch;
    }

    /// IQ at capture rate → mono audio at channel rate. Silent while squelched.
    pub fn push_iq(&mut self, iq: &[Cplx]) -> Vec<Sample> {
        let mixed: Vec<Cplx> = iq.iter().map(|&z| self.nco.push_cplx(z)).collect();
        let chan = self.front.process(&mixed);
        let open = self.squelch.observe(&chan);
        let mut audio: Vec<Sample> = chan.iter().map(|&z| self.backend.push(z)).collect();
        if let Some(dec) = self.audio_decim.as_mut() {
            audio = dec.process(&audio);
        }
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

    /// Dominant-frequency estimate by counting rising zero crossings.
    fn dominant_hz(audio: &[Sample], rate: f32) -> f32 {
        let mut crossings = 0;
        for w in audio.windows(2) {
            if w[0] <= 0.0 && w[1] > 0.0 {
                crossings += 1;
            }
        }
        crossings as f32 / (audio.len() as f32 / rate)
    }

    /// FM carrier at `offset_hz` modulated by a `tone_hz` sine.
    fn fm_iq(rate: f32, offset_hz: f32, tone_hz: f32, dev_hz: f32, n: usize) -> Vec<Cplx> {
        let mut phase = 0.0f32;
        (0..n)
            .map(|k| {
                let t = k as f32 / rate;
                let inst = offset_hz + dev_hz * (TAU * tone_hz * t).sin();
                phase += TAU * inst / rate;
                Cplx::new(phase.cos(), phase.sin())
            })
            .collect()
    }

    #[test]
    fn nbfm_recovers_modulating_tone() {
        let capture = 240_000.0;
        let channel = 48_000;
        let tone = 1_200.0;
        let mut rx = SdrDemod::new(
            DemodKind::Nbfm,
            capture as u32,
            channel,
            30_000.0,
            5_000.0,
            PowerSquelch::disabled(),
        );
        let iq = fm_iq(capture, 30_000.0, tone, 5_000.0, 48_000);
        let audio = rx.push_iq(&iq);
        let est = dominant_hz(&audio, channel as f32);
        assert!((est - tone).abs() < 150.0, "estimated {est} Hz, want {tone}");
    }

    /// AM carrier at `offset_hz`: `(1 + m·sin(tone))` envelope on a complex tone.
    fn am_iq(rate: f32, offset_hz: f32, tone_hz: f32, m: f32, n: usize) -> Vec<Cplx> {
        (0..n)
            .map(|k| {
                let t = k as f32 / rate;
                let env = 1.0 + m * (TAU * tone_hz * t).sin();
                let carr = TAU * offset_hz * t;
                Cplx::new(env * carr.cos(), env * carr.sin())
            })
            .collect()
    }

    #[test]
    fn am_recovers_tone_without_dc() {
        let capture = 240_000.0;
        let channel = 48_000;
        let tone = 1_000.0;
        let mut rx = SdrDemod::new(
            DemodKind::Am,
            capture as u32,
            channel,
            30_000.0,
            5_000.0,
            PowerSquelch::disabled(),
        );
        let iq = am_iq(capture, 30_000.0, tone, 0.5, 48_000);
        let audio = rx.push_iq(&iq);
        let steady = &audio[2_000..];
        // The DC pedestal is blocked: mean ≈ 0.
        let mean = steady.iter().sum::<f32>() / steady.len() as f32;
        assert!(mean.abs() < 0.02, "AM DC term should be blocked, mean {mean}");
        // The modulating tone is recovered.
        let est = dominant_hz(steady, channel as f32);
        assert!((est - tone).abs() < 100.0, "estimated {est} Hz, want {tone}");
    }

    #[test]
    fn wfm_if_rate_snaps_to_channel_multiple() {
        // 48 kHz audio inside a 240 kHz capture → 4× = 192 kHz IF (≥180 kHz target).
        assert_eq!(wfm_if_rate(240_000, 48_000), 192_000);
        // A capture too narrow for the target falls back to the largest multiple.
        assert_eq!(wfm_if_rate(96_000, 48_000), 96_000);
    }

    #[test]
    fn wfm_recovers_wideband_tone() {
        // Wide deviation (±50 kHz) that would clip a 48 kHz channel but fits the
        // 192 kHz WFM IF; the tone must survive the wide-IF demod + audio decimate.
        let capture = 240_000.0;
        let channel = 48_000;
        let tone = 3_000.0;
        let mut rx = SdrDemod::new(
            DemodKind::Wfm,
            capture as u32,
            channel,
            30_000.0,
            5_000.0,
            PowerSquelch::disabled(),
        );
        let iq = fm_iq(capture, 30_000.0, tone, 50_000.0, 96_000);
        let audio = rx.push_iq(&iq);
        let est = dominant_hz(&audio[400..], channel as f32);
        assert!((est - tone).abs() < 200.0, "estimated {est} Hz, want {tone}");
    }

    /// A single-sideband complex tone: suppressed carrier at `offset_hz`, audio
    /// `tone_hz` mapped `sign` above (+1 = USB) or below (−1 = LSB) the carrier.
    fn ssb_iq(rate: f32, offset_hz: f32, tone_hz: f32, sign: f32, n: usize) -> Vec<Cplx> {
        (0..n)
            .map(|k| {
                let ph = TAU * (offset_hz + sign * tone_hz) * k as f32 / rate;
                Cplx::new(ph.cos(), ph.sin())
            })
            .collect()
    }

    fn audio_rms(audio: &[Sample]) -> f32 {
        (audio.iter().map(|&s| s * s).sum::<f32>() / audio.len() as f32).sqrt()
    }

    #[test]
    fn ssb_selects_sideband() {
        let capture = 240_000.0;
        let channel = 48_000;
        let tone = 1_500.0;
        let offset = 30_000.0;

        // USB demod on a USB tone recovers it; on an LSB tone it stays quiet.
        let mut usb = SdrDemod::new(
            DemodKind::Usb, capture as u32, channel, offset, 5_000.0, PowerSquelch::disabled(),
        );
        let usb_on_usb = usb.push_iq(&ssb_iq(capture, offset, tone, 1.0, 48_000));
        let mut usb2 = SdrDemod::new(
            DemodKind::Usb, capture as u32, channel, offset, 5_000.0, PowerSquelch::disabled(),
        );
        let usb_on_lsb = usb2.push_iq(&ssb_iq(capture, offset, tone, -1.0, 48_000));

        let est = dominant_hz(&usb_on_usb[400..], channel as f32);
        assert!((est - tone).abs() < 100.0, "USB should recover {tone} Hz, got {est}");
        let pass = audio_rms(&usb_on_usb[400..]);
        let reject = audio_rms(&usb_on_lsb[400..]);
        assert!(reject < pass * 0.1, "opposite sideband should be rejected: pass {pass}, reject {reject}");

        // Symmetric check: LSB demod recovers an LSB tone and rejects a USB tone.
        let mut lsb = SdrDemod::new(
            DemodKind::Lsb, capture as u32, channel, offset, 5_000.0, PowerSquelch::disabled(),
        );
        let lsb_on_lsb = lsb.push_iq(&ssb_iq(capture, offset, tone, -1.0, 48_000));
        let mut lsb2 = SdrDemod::new(
            DemodKind::Lsb, capture as u32, channel, offset, 5_000.0, PowerSquelch::disabled(),
        );
        let lsb_on_usb = lsb2.push_iq(&ssb_iq(capture, offset, tone, 1.0, 48_000));
        let est_l = dominant_hz(&lsb_on_lsb[400..], channel as f32);
        assert!((est_l - tone).abs() < 100.0, "LSB should recover {tone} Hz, got {est_l}");
        assert!(
            audio_rms(&lsb_on_usb[400..]) < audio_rms(&lsb_on_lsb[400..]) * 0.1,
            "LSB should reject the upper sideband"
        );
    }

    #[test]
    fn deemphasis_rolls_off_high_fm_audio() {
        // With de-emphasis on, a high modulating tone comes out weaker than the
        // same tone with de-emphasis off (default). Same deviation, same tone.
        let capture = 240_000.0;
        let channel = 48_000;
        let tone = 5_000.0;
        let iq = fm_iq(capture, 30_000.0, tone, 5_000.0, 48_000);

        let mut plain = SdrDemod::new(
            DemodKind::Nbfm, capture as u32, channel, 30_000.0, 5_000.0, PowerSquelch::disabled(),
        );
        let a_plain = plain.push_iq(&iq);

        let mut de = SdrDemod::new(
            DemodKind::Nbfm, capture as u32, channel, 30_000.0, 5_000.0, PowerSquelch::disabled(),
        )
        .with_deemphasis(75.0);
        let a_de = de.push_iq(&iq);

        let rms = |a: &[Sample]| (a.iter().map(|&s| s * s).sum::<f32>() / a.len() as f32).sqrt();
        assert!(
            rms(&a_de[400..]) < rms(&a_plain[400..]) * 0.7,
            "de-emphasis should attenuate the 5 kHz tone"
        );
    }

    #[test]
    fn nbfm_squelch_silences_weak_input() {
        let mut rx = SdrDemod::new(
            DemodKind::Nbfm,
            240_000,
            48_000,
            30_000.0,
            5_000.0,
            PowerSquelch::new(-20.0, 6.0),
        );
        let iq: Vec<Cplx> = (0..48_000).map(|_| Cplx::new(1e-4, -1e-4)).collect();
        let audio = rx.push_iq(&iq);
        assert!(audio.iter().all(|&s| s == 0.0));
    }
}
