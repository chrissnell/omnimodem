//! RTTY mode assembly: 45.45 baud, 170 Hz shift, 5-bit Baudot/ITA2.
//!
//! TX: text -> Baudot codes -> start/stop framed bits (1 start + 5 data +
//! 1.5 stop) -> 2-FSK mark/space. RX: down-convert + FM-discriminator ->
//! start-bit sync samples 5 data bits -> Baudot decode.

use crate::frontend::detector::FmDiscriminator;
use crate::frontend::fir::{design_lowpass, Fir};
use crate::frontend::modulate::Fsk2;
use crate::frontend::nco::DownConverter;
use crate::framing::baudot::{encode as baudot_encode, Decoder as BaudotDecoder};
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::sync::timing::StartBitSync;
use crate::types::{Cplx, Frame, FrameMeta, FramePayload, Sample};

pub const RTTY_RATE: u32 = 8_000;

/// Default audio center the mark/space pair straddles (center +/- shift/2).
/// Real recordings vary — US ham RTTY commonly sits at 2125/2295 Hz (≈2210
/// center) — so both the modulator and demodulator take a configurable center;
/// this is only the default for the convenience constructors.
pub const CENTER_HZ: f32 = 1500.0;

/// Baseband channel filter: passes the mark/space pair (well inside +/- shift)
/// while rejecting the 2*center image the real->complex mix leaves behind. The
/// cutoff is wide enough to cover the ±shift the demod cares about; the center
/// is retuned by the down-converter, so the same filter works at any center.
const LPF_TAPS: usize = 31;
const LPF_CUTOFF_HZ: f32 = 300.0;

/// Mark idle cells emitted before the first character so the discriminator and
/// start-bit sync settle on the idle (mark) line before the first start edge.
const PREAMBLE_CELLS: usize = 8;

/// Carrier squelch. A real FSK tone sits inside the narrow channel filter, so
/// most of its power survives the low-pass (in-band / total ≈ 0.5); white noise
/// is spread across the whole band and the filter throws most of it away
/// (≈ 0.08). Gating on that level-independent ratio keeps the framer from
/// slicing the noise floor into Baudot junk, with no dependence on signal level.
const SQUELCH_EMA: f32 = 0.002;
const SQUELCH_OPEN: f32 = 0.3;
/// Absolute power floor so digital silence reads as "no carrier".
const SQUELCH_FLOOR: f32 = 1e-9;

pub struct RttyMod {
    baud: f32,
    shift_hz: f32,
    center_hz: f32,
}

impl RttyMod {
    pub fn new(baud: f32, shift_hz: f32) -> Self {
        Self::with_center(baud, shift_hz, CENTER_HZ)
    }

    /// Modulate at an explicit audio center (e.g. 2210 Hz for US ham RTTY).
    pub fn with_center(baud: f32, shift_hz: f32, center_hz: f32) -> Self {
        RttyMod { baud, shift_hz, center_hz }
    }
}

impl Modulator for RttyMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: RTTY_RATE,
            bandwidth_hz: self.shift_hz + 2.0 * self.baud,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("rtty needs text")),
        };
        let sps = (RTTY_RATE as f32 / self.baud).round() as usize;
        // Each character: 1 start bit (space=false), 5 data bits (LSB-first),
        // 2 stop bits (mark=true). Mark is the idle/high tone.
        let mut bits: Vec<bool> = vec![true; PREAMBLE_CELLS];
        for code in baudot_encode(text) {
            bits.push(false); // start
            for i in 0..5 {
                bits.push((code >> i) & 1 == 1); // data, LSB-first
            }
            bits.push(true); // stop
            bits.push(true); // stop (>=1.5 stop bits)
        }
        bits.extend(std::iter::repeat_n(true, PREAMBLE_CELLS)); // trailing idle
        let fsk = Fsk2::new(RTTY_RATE as f32, sps, self.center_hz, self.shift_hz);
        Ok(fsk.modulate(&bits))
    }
}

pub struct RttyDemod {
    baud: f32,
    shift_hz: f32,
    center_hz: f32,
    nco: DownConverter,
    lpf_i: Fir,
    lpf_q: Fir,
    disc: FmDiscriminator,
    // Smoothed in-band (post-filter) and total (pre-filter) power for the
    // carrier squelch; their ratio tells a tone from the noise floor.
    p_in: f32,
    p_tot: f32,
    sync: StartBitSync,
    baudot: BaudotDecoder,
    text: String,
    sample_index: u64,
}

impl RttyDemod {
    pub fn new(baud: f32, shift_hz: f32) -> Self {
        Self::with_center(baud, shift_hz, CENTER_HZ)
    }

    /// Demodulate around an explicit audio center (e.g. 2210 Hz for US ham
    /// RTTY). The down-converter retunes to `center_hz`; everything downstream
    /// works at baseband, so the rest of the chain is center-independent.
    pub fn with_center(baud: f32, shift_hz: f32, center_hz: f32) -> Self {
        let rate = RTTY_RATE as f32;
        let taps = design_lowpass(LPF_TAPS, LPF_CUTOFF_HZ, rate);
        RttyDemod {
            baud,
            shift_hz,
            center_hz,
            nco: DownConverter::new(center_hz, rate),
            lpf_i: Fir::new(taps.clone()),
            lpf_q: Fir::new(taps),
            disc: FmDiscriminator::new(),
            p_in: 0.0,
            p_tot: 0.0,
            sync: StartBitSync::new(rate / baud),
            baudot: BaudotDecoder::new(),
            text: String::new(),
            sample_index: 0,
        }
    }
}

impl Demodulator for RttyDemod {
    fn caps(&self) -> ModeCaps {
        RttyMod::new(self.baud, self.shift_hz).caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        for &x in samples {
            self.sample_index += 1;
            let bb = self.nco.push(x);
            let total = bb.norm_sqr(); // power before the channel filter
            // Channel-filter the complex baseband (kills the 2*center image) so
            // the discriminator sees a clean tone.
            let bb = Cplx::new(self.lpf_i.push(bb.re), self.lpf_q.push(bb.im));
            // Carrier squelch: smoothed in-band / total power ratio. A tone
            // survives the narrow filter, noise does not.
            self.p_in += SQUELCH_EMA * (bb.norm_sqr() - self.p_in);
            self.p_tot += SQUELCH_EMA * (total - self.p_tot);
            let open = self.p_tot > SQUELCH_FLOOR && self.p_in > SQUELCH_OPEN * self.p_tot;
            // Instantaneous frequency after down-conversion to center: positive
            // => above center => mark (high tone) => logic 1. Keep the
            // discriminator running every sample so its phase reference stays
            // continuous, but with no carrier hold the line at the idle mark so
            // the framer never syncs on a noise edge.
            let freq = self.disc.push(bb);
            let level = if open { freq > 0.0 } else { true };
            if let Some(code_bits) = self.sync.feed(level) {
                let mut code = 0u8;
                for (i, &b) in code_bits.iter().enumerate() {
                    if b {
                        code |= 1 << i; // pack LSB-first into the Baudot code
                    }
                }
                if let Some(c) = self.baudot.feed(code) {
                    self.text.push(c);
                }
            }
        }
        if self.text.is_empty() {
            return Vec::new();
        }
        vec![Frame {
            payload: FramePayload::Text(std::mem::take(&mut self.text)),
            meta: FrameMeta {
                crc_ok: true,
                sample_offset: self.sample_index,
                decoder: Some("rtty".into()),
                ..Default::default()
            },
        }]
    }

    fn reset(&mut self) {
        *self = RttyDemod::with_center(self.baud, self.shift_hz, self.center_hz);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_are_tx() {
        let m = RttyMod::new(45.45, 170.0);
        assert!(m.caps().tx);
        assert_eq!(m.caps().native_rate, RTTY_RATE);
    }

    #[test]
    fn rejects_non_text_payload() {
        let mut m = RttyMod::new(45.45, 170.0);
        let frame = Frame::packet(vec![1, 2, 3]);
        assert!(matches!(m.modulate(&frame), Err(ModError::UnsupportedPayload(_))));
    }

    #[test]
    fn modulates_text() {
        let mut m = RttyMod::new(45.45, 170.0);
        assert!(m.modulate(&Frame::text("RYRY")).unwrap().len() > 100);
    }

    #[test]
    fn loopback_recovers_text() {
        let msg = "THE QUICK BROWN FOX";
        let mut tx = RttyMod::new(45.45, 170.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = RttyDemod::new(45.45, 170.0);
        let frames = rx.feed(&samples);
        let text: String = frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(text.contains(msg), "got {text:?}");
    }

    #[test]
    fn noise_does_not_decode() {
        // White noise carries no FSK tone; the squelch must keep the framer from
        // slicing the noise floor into Baudot characters.
        let mut rng = crate::testutil::Rng::new(0xBADC0DE);
        let mut noise = vec![0.0f32; RTTY_RATE as usize * 2]; // 2 s
        crate::testutil::add_awgn(&mut noise, 0.3, &mut rng);
        let mut rx = RttyDemod::new(45.45, 170.0);
        let text: String = rx
            .feed(&noise)
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(text.len() <= 2, "noise should stay squelched, decoded {text:?}");
    }
}
