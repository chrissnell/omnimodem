//! The one-module mode registry. Adding a mode in Phase 4 adds one arm here and
//! its module; nothing else in the daemon learns mode-specific details.

use super::{ModeConfig, NullMode};
use omnimodem_dsp::mode::{Demodulator, Modulator};

/// Build a streaming demodulator for a config, or `None` if the mode has no
/// streaming demod (windowed modes return their `BlockDemodulator` elsewhere).
pub fn build_demod(cfg: &ModeConfig) -> Option<Box<dyn Demodulator>> {
    match cfg {
        ModeConfig::None => Some(Box::new(NullMode)),
        // Phase 4: Afsk1200/Cw/Rtty/Psk31 return their streaming demods; Ft8
        // returns its BlockDemodulator via a separate builder.
        _ => None,
    }
}

/// Build a modulator for a config, or `None` if the mode is receive-only.
pub fn build_modulator(cfg: &ModeConfig) -> Option<Box<dyn Modulator>> {
    match cfg {
        ModeConfig::None => Some(Box::new(NullMode)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_builds_nullmode() {
        let d = build_demod(&ModeConfig::None).expect("none builds");
        assert_eq!(d.caps().native_rate, 48_000);
    }

    #[test]
    fn none_builds_modulator() {
        let m = build_modulator(&ModeConfig::None).expect("none builds mod");
        assert!(m.caps().native_rate == 48_000);
    }
}
