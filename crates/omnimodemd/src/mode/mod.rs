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
    Rtty { baud: f32, shift_hz: f32, center_hz: f32 },
    Psk31 { center_hz: f32 },
    // Phase 5 WSJT-X breadth modes.
    Ft4,
    Jt65,
    Jt9,
    Wspr,
    // Phase 5 fldigi breadth modes.
    Olivia { tones: u16, bandwidth_hz: u16 },
}

impl ModeConfig {
    /// Parse the channel's `mode` string into a parametric config.
    ///
    /// Two forms, both accepted (the second is the channel-configuration protocol
    /// expansion): a bare label (`"cw"`) resolves with default parameters, and a
    /// parametric form (`"cw:wpm=25,tone=600"`) overrides individual params via a
    /// `:`-separated `key=value,…` tail. Missing or unparseable keys fall back to
    /// the mode default, so a partial spec is always valid; an unknown *mode* is
    /// still rejected so a typo can't silently configure nothing. This is the
    /// canonical persisted form — [`ModeConfig::to_mode_string`] round-trips it.
    pub fn parse(s: &str) -> Option<ModeConfig> {
        let (mode, tail) = match s.split_once(':') {
            Some((m, t)) => (m, Some(t)),
            None => (s, None),
        };
        let kv = parse_params(tail);
        let f = |k: &str, d: f32| kv.get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        let u = |k: &str, d: u16| kv.get(k).and_then(|v| v.parse::<u16>().ok()).unwrap_or(d);
        match mode {
            "none" | "" => Some(ModeConfig::None),
            "afsk1200" => Some(ModeConfig::Afsk1200 { tx: true }),
            "ft8" => Some(ModeConfig::Ft8),
            "cw" => Some(ModeConfig::Cw { wpm: u("wpm", 20), tone_hz: f("tone", 700.0) }),
            "rtty" => Some(ModeConfig::Rtty {
                baud: f("baud", 45.45),
                shift_hz: f("shift", 170.0),
                center_hz: f("center", omnimodem_dsp::modes::rtty::CENTER_HZ),
            }),
            "psk31" => Some(ModeConfig::Psk31 { center_hz: f("center", 1000.0) }),
            "ft4" => Some(ModeConfig::Ft4),
            "jt65" => Some(ModeConfig::Jt65),
            "jt9" => Some(ModeConfig::Jt9),
            "wspr" => Some(ModeConfig::Wspr),
            "olivia" => {
                Some(ModeConfig::Olivia { tones: u("tones", 32), bandwidth_hz: u("bw", 1000) })
            }
            _ => None,
        }
    }

    /// Canonical mode string: bare label for parameterless modes, `label:k=v,…`
    /// for parametric ones. Round-trips through [`ModeConfig::parse`], so it is the
    /// form the daemon persists when a client supplies structured mode params.
    pub fn to_mode_string(&self) -> String {
        match self {
            ModeConfig::Cw { wpm, tone_hz } => format!("cw:wpm={wpm},tone={tone_hz}"),
            ModeConfig::Rtty { baud, shift_hz, center_hz } => {
                format!("rtty:baud={baud},shift={shift_hz},center={center_hz}")
            }
            ModeConfig::Psk31 { center_hz } => format!("psk31:center={center_hz}"),
            ModeConfig::Olivia { tones, bandwidth_hz } => {
                format!("olivia:tones={tones},bw={bandwidth_hz}")
            }
            other => other.label().to_string(),
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
            ModeConfig::Ft4 => "ft4",
            ModeConfig::Jt65 => "jt65",
            ModeConfig::Jt9 => "jt9",
            ModeConfig::Wspr => "wspr",
            ModeConfig::Olivia { .. } => "olivia",
        }
    }
}

/// Parse a `key=value,key=value` parameter tail into a lookup. Empty or absent
/// tails yield an empty map; malformed entries (no `=`) are skipped.
fn parse_params(tail: Option<&str>) -> std::collections::HashMap<&str, &str> {
    tail.unwrap_or("")
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect()
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
    fn parse_resolves_phase4_modes_with_defaults() {
        assert_eq!(ModeConfig::parse("afsk1200"), Some(ModeConfig::Afsk1200 { tx: true }));
        assert_eq!(ModeConfig::parse("ft8"), Some(ModeConfig::Ft8));
        assert_eq!(ModeConfig::parse("cw"), Some(ModeConfig::Cw { wpm: 20, tone_hz: 700.0 }));
        assert_eq!(
            ModeConfig::parse("rtty"),
            Some(ModeConfig::Rtty { baud: 45.45, shift_hz: 170.0, center_hz: 2210.0 })
        );
        assert_eq!(ModeConfig::parse("psk31"), Some(ModeConfig::Psk31 { center_hz: 1000.0 }));
        assert_eq!(ModeConfig::parse("none"), Some(ModeConfig::None));
        assert_eq!(ModeConfig::parse(""), Some(ModeConfig::None));
        assert_eq!(ModeConfig::parse("bogus"), None);
    }

    #[test]
    fn parse_resolves_wsjtx_breadth_modes() {
        assert_eq!(ModeConfig::parse("ft4"), Some(ModeConfig::Ft4));
        assert_eq!(ModeConfig::parse("jt65"), Some(ModeConfig::Jt65));
        assert_eq!(ModeConfig::parse("jt9"), Some(ModeConfig::Jt9));
        assert_eq!(ModeConfig::parse("wspr"), Some(ModeConfig::Wspr));
    }

    #[test]
    fn parse_resolves_olivia_with_defaults() {
        assert_eq!(
            ModeConfig::parse("olivia"),
            Some(ModeConfig::Olivia { tones: 32, bandwidth_hz: 1000 })
        );
    }

    #[test]
    fn parse_accepts_parametric_strings() {
        assert_eq!(
            ModeConfig::parse("cw:wpm=25,tone=600"),
            Some(ModeConfig::Cw { wpm: 25, tone_hz: 600.0 })
        );
        assert_eq!(
            ModeConfig::parse("rtty:baud=75,shift=850"),
            Some(ModeConfig::Rtty { baud: 75.0, shift_hz: 850.0, center_hz: 2210.0 })
        );
        assert_eq!(
            ModeConfig::parse("rtty:baud=45.45,shift=170,center=2125"),
            Some(ModeConfig::Rtty { baud: 45.45, shift_hz: 170.0, center_hz: 2125.0 })
        );
        assert_eq!(
            ModeConfig::parse("psk31:center=1500"),
            Some(ModeConfig::Psk31 { center_hz: 1500.0 })
        );
        assert_eq!(
            ModeConfig::parse("olivia:tones=16,bw=500"),
            Some(ModeConfig::Olivia { tones: 16, bandwidth_hz: 500 })
        );
    }

    #[test]
    fn parse_partial_or_bad_params_fall_back_to_defaults() {
        assert_eq!(
            ModeConfig::parse("cw:wpm=30"),
            Some(ModeConfig::Cw { wpm: 30, tone_hz: 700.0 })
        );
        assert_eq!(
            ModeConfig::parse("cw:wpm=abc,tone=550"),
            Some(ModeConfig::Cw { wpm: 20, tone_hz: 550.0 })
        );
        assert_eq!(ModeConfig::parse("bogus:x=1"), None);
    }

    #[test]
    fn to_mode_string_round_trips_through_parse() {
        let cases = [
            ModeConfig::None,
            ModeConfig::Afsk1200 { tx: true },
            ModeConfig::Ft8,
            ModeConfig::Cw { wpm: 25, tone_hz: 600.0 },
            ModeConfig::Rtty { baud: 75.0, shift_hz: 850.0, center_hz: 2125.0 },
            ModeConfig::Psk31 { center_hz: 1500.0 },
            ModeConfig::Olivia { tones: 16, bandwidth_hz: 500 },
            ModeConfig::Wspr,
        ];
        for c in cases {
            assert_eq!(ModeConfig::parse(&c.to_mode_string()), Some(c.clone()), "round-trip {c:?}");
        }
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
            ModeConfig::Rtty { baud: 45.45, shift_hz: 170.0, center_hz: 2210.0 }.label(),
            ModeConfig::Psk31 { center_hz: 1000.0 }.label(),
            ModeConfig::Ft4.label(),
            ModeConfig::Jt65.label(),
            ModeConfig::Jt9.label(),
            ModeConfig::Wspr.label(),
            ModeConfig::Olivia { tones: 32, bandwidth_hz: 1000 }.label(),
        ];
        for l in labels {
            assert!(!l.is_empty(), "mode label must be non-empty");
        }
        let unique: std::collections::BTreeSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "mode labels must be distinct");
    }
}
