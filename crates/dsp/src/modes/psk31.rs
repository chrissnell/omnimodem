//! Back-compat shim: PSK31 is `PskVariant::Psk31` in the parametric `modes::psk`
//! family. This preserves the original `Psk31Mod`/`Psk31Demod` API (constructed
//! from a bare `center_hz`) for existing callers — the daemon registry, wavtool,
//! and the KAT/BER/loopback/snapshot suites — while the implementation lives in
//! `modes::psk`. New code should use `modes::psk` directly.

use crate::mode::{Demodulator, ModError, ModeCaps, Modulator};
use crate::modes::psk::{PskDemod, PskMod, PskVariant};
use crate::types::{Frame, Sample};

pub const PSK31_RATE: u32 = crate::modes::psk::PSK_RATE;
pub const PSK31_BAUD: f32 = 31.25;

pub struct Psk31Mod(PskMod);

impl Psk31Mod {
    pub fn new(center_hz: f32) -> Self {
        Psk31Mod(PskMod::new(PskVariant::Psk31, center_hz))
    }
}

impl Modulator for Psk31Mod {
    fn caps(&self) -> ModeCaps {
        self.0.caps()
    }
    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        self.0.modulate(frame)
    }
}

pub struct Psk31Demod(PskDemod);

impl Psk31Demod {
    pub fn new(center_hz: f32) -> Self {
        Psk31Demod(PskDemod::new(PskVariant::Psk31, center_hz))
    }
}

impl Demodulator for Psk31Demod {
    fn caps(&self) -> ModeCaps {
        self.0.caps()
    }
    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.0.feed(samples)
    }
    fn reset(&mut self) {
        self.0.reset()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FramePayload;

    fn recovered_text(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn loopback_recovers_text_via_shim() {
        let msg = "CQ DE K1ABC";
        let s = Psk31Mod::new(1000.0).modulate(&Frame::text(msg)).unwrap();
        assert!(recovered_text(&Psk31Demod::new(1000.0).feed(&s)).contains(msg));
    }

    #[test]
    fn rejects_packet_payload() {
        let mut m = Psk31Mod::new(1000.0);
        assert!(matches!(
            m.modulate(&Frame::packet(vec![1, 2])).unwrap_err(),
            ModError::UnsupportedPayload(_)
        ));
    }
}
