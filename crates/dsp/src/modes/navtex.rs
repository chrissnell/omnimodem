//! NAVTEX / SITOR-B: 100-baud narrow-shift FSK maritime broadcast text with
//! CCIR-476 FEC-B (time-diversity) forward error correction.
//!
//! Port of fldigi's NAVTEX modem (`fldigi/src/navtex/navtex.cxx`, upstream
//! w1hkj/fldigi 4.1.23 @ `61b97f413`). The bit-domain codec — the CCIR-476
//! 4-of-7 code and the FEC-B diversity stream — lives in [`crate::fec::ccir476`]
//! and is bit-exact vs the reference. This module wires it to the on-air layer:
//! a 100-baud 2-FSK carrier at ±85 Hz around a 1000 Hz center (`deviation_f`,
//! navtex.cxx:824; `m_baud_rate`, :924; mark = center+85 / space = center−85,
//! :954-955), sent LSB-first per codeword (`send_bit`/`send_string`,
//! navtex.cxx:1782-1804).
//!
//! **TX** (`NavtexMod`): `ccir476::encode` → `create_fec` → bits → `Fsk2`.
//! **RX** (`NavtexDemod`): down-convert + channel-filter + FM-discriminate as
//! RTTY does, a `GardnerTed` 100-baud symbol clock recovers one soft bit per
//! symbol, and `ccir476::FecBDecoder` performs character sync + the
//! direct/repeat/soft-combine/bit-flip FEC-B ladder → text.
//!
//! NAVTEX and SITOR-B share this identical on-air codec (fldigi's modem is one
//! class with an `m_only_sitor_b` flag that changes only the message-list
//! segmentation, not the decoded character stream, navtex.cxx:1000-1044); the
//! two [`NavtexVariant`]s select the same demod and differ only in decoder
//! label. The audio is FP (Doctrine §3): the gate is a loopback that recovers
//! the text, plus the bit-exact CCIR-476 KAT; cross-decode vs the fldigi binary
//! is `#[ignore]`-gated.

use crate::fec::ccir476::{codes_to_bits, create_fec, Ccir476, FecBDecoder};
use crate::frontend::detector::FmDiscriminator;
use crate::frontend::fir::{design_lowpass, Fir};
use crate::frontend::modulate::Fsk2;
use crate::frontend::nco::DownConverter;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::sync::timing::GardnerTed;
use crate::types::{Cplx, Frame, FrameMeta, FramePayload, Sample};

/// Native working rate. 100 baud → exactly 80 samples/bit at 8 kHz.
pub const NAVTEX_RATE: u32 = 8_000;
pub const BAUD: f32 = 100.0;
/// Full mark-to-space shift: ±85 Hz deviation (navtex.cxx:824).
pub const SHIFT_HZ: f32 = 170.0;
/// Default audio center (navtex.cxx:826, `dflt_center_freq`).
pub const CENTER_HZ: f32 = 1000.0;

/// The two selectable modes. Both use the identical CCIR-476 FEC-B / FSK codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavtexVariant {
    /// 518 kHz international NAVTEX broadcast (ZCZC…NNNN framed on the air).
    Navtex,
    /// Raw SITOR mode B (Amateur AMTOR FEC), the same codec without framing.
    SitorB,
}

impl NavtexVariant {
    pub fn from_label(s: &str) -> Option<Self> {
        match s {
            "navtex" => Some(NavtexVariant::Navtex),
            "sitorb" => Some(NavtexVariant::SitorB),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            NavtexVariant::Navtex => "navtex",
            NavtexVariant::SitorB => "sitorb",
        }
    }
    pub fn all() -> &'static [NavtexVariant] {
        &[NavtexVariant::Navtex, NavtexVariant::SitorB]
    }
}

/// Baseband channel filter cutoff — passes the ±85 Hz mark/space pair while
/// rejecting the 2·center image the real→complex mix leaves behind.
const LPF_TAPS: usize = 63;
const LPF_CUTOFF_HZ: f32 = 200.0;
/// Idle-mark settling cells sent before the FEC stream so the RX channel
/// filter, discriminator and symbol clock lock before the payload arrives.
const PREAMBLE_BITS: usize = 32;

// Carrier squelch (level-independent in-band/total power ratio), as RTTY.
const SQUELCH_EMA: f32 = 0.002;
const SQUELCH_OPEN: f32 = 0.3;
const SQUELCH_FLOOR: f32 = 1e-9;

pub struct NavtexMod {
    variant: NavtexVariant,
    center_hz: f32,
    ccir: Ccir476,
}

impl NavtexMod {
    pub fn new(variant: NavtexVariant, center_hz: f32) -> Self {
        NavtexMod { variant, center_hz, ccir: Ccir476::new() }
    }
}

impl Modulator for NavtexMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: NAVTEX_RATE,
            bandwidth_hz: SHIFT_HZ + 2.0 * BAUD,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("navtex needs text")),
        };
        let codes = self.ccir.encode(text);
        if codes.is_empty() {
            return Ok(Vec::new());
        }
        let fec = create_fec(&codes);
        // Idle mark preamble (bit 1) + the FEC bit stream + a short idle tail.
        let mut bits = vec![true; PREAMBLE_BITS];
        bits.extend(codes_to_bits(&fec));
        bits.extend(std::iter::repeat_n(true, PREAMBLE_BITS));
        let sps = (NAVTEX_RATE as f32 / BAUD).round() as usize;
        // Mark = center + shift/2 = center + 85; bit 1 → mark (navtex.cxx:1782-1784).
        let fsk = Fsk2::new(NAVTEX_RATE as f32, sps, self.center_hz, SHIFT_HZ);
        // NAVTEX and SITOR-B share the identical CCIR-476 FEC-B wire codec, so the
        // variant does not change the transmitted bits (only the decoder label).
        let _ = self.variant;
        Ok(fsk.modulate(&bits))
    }
}

pub struct NavtexDemod {
    variant: NavtexVariant,
    center_hz: f32,
    nco: DownConverter,
    lpf_i: Fir,
    lpf_q: Fir,
    disc: FmDiscriminator,
    ted: GardnerTed,
    p_in: f32,
    p_tot: f32,
    fec: FecBDecoder,
    text: String,
    sample_index: u64,
}

impl NavtexDemod {
    pub fn new(variant: NavtexVariant, center_hz: f32) -> Self {
        let rate = NAVTEX_RATE as f32;
        let taps = design_lowpass(LPF_TAPS, LPF_CUTOFF_HZ, rate);
        NavtexDemod {
            variant,
            center_hz,
            nco: DownConverter::new(center_hz, rate),
            lpf_i: Fir::new(taps.clone()),
            lpf_q: Fir::new(taps),
            disc: FmDiscriminator::new(),
            ted: GardnerTed::new(rate / BAUD),
            p_in: 0.0,
            p_tot: 0.0,
            fec: FecBDecoder::new(),
            text: String::new(),
            sample_index: 0,
        }
    }

    fn drain_text(&mut self) -> Vec<Frame> {
        if self.text.is_empty() {
            return Vec::new();
        }
        vec![Frame {
            payload: FramePayload::Text(std::mem::take(&mut self.text)),
            meta: FrameMeta {
                crc_ok: true,
                sample_offset: self.sample_index,
                decoder: Some(self.variant.label().into()),
                ..Default::default()
            },
        }]
    }
}

impl Demodulator for NavtexDemod {
    fn caps(&self) -> ModeCaps {
        NavtexMod::new(self.variant, self.center_hz).caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        for &x in samples {
            self.sample_index += 1;
            let bb = self.nco.push(x);
            let total = bb.norm_sqr();
            let bb = Cplx::new(self.lpf_i.push(bb.re), self.lpf_q.push(bb.im));
            self.p_in += SQUELCH_EMA * (bb.norm_sqr() - self.p_in);
            self.p_tot += SQUELCH_EMA * (total - self.p_tot);
            let open = self.p_tot > SQUELCH_FLOOR && self.p_in > SQUELCH_OPEN * self.p_tot;
            // Instantaneous frequency after down-conversion to center: mark
            // (center+85) is a positive baseband frequency ⇒ soft bit > 0 = 1.
            let freq = self.disc.push(bb);
            let level = if open { freq } else { 0.0 };
            if let Some(soft) = self.ted.feed(level) {
                for &c in self.fec.feed_bit(soft) {
                    self.text.push(c as char);
                }
            }
        }
        self.drain_text()
    }

    fn reset(&mut self) {
        *self = NavtexDemod::new(self.variant, self.center_hz);
    }

    fn flush(&mut self) -> Vec<Frame> {
        self.drain_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_text(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn caps_are_tx_at_native_rate() {
        let m = NavtexMod::new(NavtexVariant::SitorB, CENTER_HZ);
        assert!(m.caps().tx);
        assert_eq!(m.caps().native_rate, NAVTEX_RATE);
    }

    #[test]
    fn rejects_non_text_payload() {
        let mut m = NavtexMod::new(NavtexVariant::Navtex, CENTER_HZ);
        assert!(matches!(
            m.modulate(&Frame::packet(vec![1, 2, 3])),
            Err(ModError::UnsupportedPayload(_))
        ));
    }

    #[test]
    fn variant_labels_round_trip() {
        for &v in NavtexVariant::all() {
            assert_eq!(NavtexVariant::from_label(v.label()), Some(v));
        }
        assert_eq!(NavtexVariant::from_label("bogus"), None);
    }

    #[test]
    fn loopback_recovers_text_both_variants() {
        let msg = "CQ DE K1ABC";
        for &v in NavtexVariant::all() {
            let mut tx = NavtexMod::new(v, CENTER_HZ);
            let audio = tx.modulate(&Frame::text(msg)).unwrap();
            let mut rx = NavtexDemod::new(v, CENTER_HZ);
            let mut frames = Vec::new();
            for chunk in audio.chunks(512) {
                frames.extend(rx.feed(chunk));
            }
            frames.extend(rx.flush());
            let text = all_text(&frames);
            assert!(text.contains(msg), "{}: decoded {text:?}", v.label());
        }
    }

    #[test]
    fn loopback_recovers_figures_and_shifts() {
        let msg = "SECURITE 12 SHIPS";
        let mut tx = NavtexMod::new(NavtexVariant::SitorB, CENTER_HZ);
        let audio = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = NavtexDemod::new(NavtexVariant::SitorB, CENTER_HZ);
        let mut frames = Vec::new();
        for chunk in audio.chunks(512) {
            frames.extend(rx.feed(chunk));
        }
        frames.extend(rx.flush());
        assert!(all_text(&frames).contains(msg), "decoded {:?}", all_text(&frames));
    }

    #[test]
    fn noise_stays_squelched() {
        let mut rng = crate::testutil::Rng::new(0x0051_705B);
        let mut noise = vec![0.0f32; NAVTEX_RATE as usize];
        crate::testutil::add_awgn(&mut noise, 0.3, &mut rng);
        let mut rx = NavtexDemod::new(NavtexVariant::SitorB, CENTER_HZ);
        let text = all_text(&rx.feed(&noise));
        assert!(text.len() <= 4, "noise decoded {text:?}");
    }
}
