//! Mode framework wiring: the parametric per-mode config and the registry that
//! turns a config into a boxed demodulator/modulator. Phase 3 implements only
//! `NullMode`; Phase-4 variants are present as data so the enum is stable.

pub mod registry;

use omnimodem_dsp::mode::ModeCaps;

/// Parametric per-mode configuration (design §"Mode framework model": NOT one
/// flat struct). Variants beyond `None` are data-only until Phase 4.
#[derive(Debug, Clone, PartialEq)]
pub enum ModeConfig {
    None,
    Afsk1200 { tx: bool },
    Ft8,
    Cw { wpm: u16, tone_hz: f32 },
    Rtty { baud: f32, shift_hz: f32 },
    Psk31 { center_hz: f32 },
}

impl ModeConfig {
    /// Parse the channel's `mode` string. Phase 3 only resolves "none"; unknown
    /// strings are rejected so a typo can't silently configure nothing.
    pub fn parse(s: &str) -> Option<ModeConfig> {
        match s {
            "none" | "" => Some(ModeConfig::None),
            _ => None, // Phase 4 extends this; keep strict.
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            ModeConfig::None => "none",
            ModeConfig::Afsk1200 { .. } => "afsk1200",
            ModeConfig::Ft8 => "ft8",
            ModeConfig::Cw { .. } => "cw",
            ModeConfig::Rtty { .. } => "rtty",
            ModeConfig::Psk31 { .. } => "psk31",
        }
    }
}

/// The framework fixture: a passthrough that satisfies the trait surface so the
/// registry, channel wiring, and conformance harness exercise a real demod/mod
/// without shipping an end-user mode.
pub struct NullMode;

impl omnimodem_dsp::mode::Demodulator for NullMode {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: 48_000,
            bandwidth_hz: 0.0,
            tx: false,
            duplex: omnimodem_dsp::mode::Duplex::Half,
            shape: omnimodem_dsp::mode::DemodShape::Streaming,
        }
    }
    fn feed(&mut self, _s: &[omnimodem_dsp::Sample]) -> Vec<omnimodem_dsp::Frame> {
        vec![]
    }
    fn reset(&mut self) {}
}

impl omnimodem_dsp::mode::Modulator for NullMode {
    fn caps(&self) -> ModeCaps {
        <NullMode as omnimodem_dsp::mode::Demodulator>::caps(self)
    }
    fn modulate(
        &mut self,
        _frame: &omnimodem_dsp::Frame,
    ) -> Result<Vec<omnimodem_dsp::Sample>, omnimodem_dsp::ModError> {
        // The fixture transmits silence; a real mode renders baseband audio.
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_strict() {
        assert_eq!(ModeConfig::parse("none"), Some(ModeConfig::None));
        assert_eq!(ModeConfig::parse("ft8"), None); // not until Phase 4
        assert_eq!(ModeConfig::parse("bogus"), None);
    }

    #[test]
    fn label_round_trips_none() {
        assert_eq!(ModeConfig::None.label(), "none");
        // "none" parses back to the variant whose label it is — the one
        // round-trip Phase 3 can assert end-to-end.
        assert_eq!(ModeConfig::parse(ModeConfig::None.label()), Some(ModeConfig::None));
    }

    #[test]
    fn labels_are_distinct_and_non_empty() {
        let labels = [
            ModeConfig::None.label(),
            ModeConfig::Afsk1200 { tx: false }.label(),
            ModeConfig::Ft8.label(),
            ModeConfig::Cw { wpm: 20, tone_hz: 700.0 }.label(),
            ModeConfig::Rtty { baud: 45.45, shift_hz: 170.0 }.label(),
            ModeConfig::Psk31 { center_hz: 1000.0 }.label(),
        ];
        for l in labels {
            assert!(!l.is_empty(), "mode label must be non-empty");
        }
        let unique: std::collections::BTreeSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "mode labels must be distinct");
    }
}
