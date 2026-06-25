//! The one-module mode registry. Adding a mode in Phase 4 adds one arm here and
//! its assembly module in `omnimodem-dsp`; nothing else in the daemon learns
//! mode-specific details.

use super::{ModeConfig, NullMode};
use omnimodem_dsp::mode::{BlockDemodulator, DemodShape, Demodulator, Modulator};
use omnimodem_dsp::modes::{
    afsk1200::{Afsk1200Demod, Afsk1200Mod},
    cw::{CwDemod, CwMod},
    ft4::{Ft4Demod, Ft4Mod},
    ft8::{Ft8Demod, Ft8Mod},
    jt65::{Jt65Demod, Jt65Mod},
    jt9::{Jt9Demod, Jt9Mod},
    olivia::{OliviaDemod, OliviaMod},
    psk31::{Psk31Demod, Psk31Mod},
    rtty::{RttyDemod, RttyMod},
    wspr::{WsprDemod, WsprMod},
};

/// What kind of demod a mode needs the RX worker to drive.
pub enum DemodKind {
    /// No real demod (the `NullMode` fixture); the capture is held idle.
    None,
    /// A continuous streaming demod.
    Streaming(Box<dyn Demodulator>),
    /// A windowed block demod plus its window length in seconds.
    Windowed(Box<dyn BlockDemodulator>, f32),
}

/// Wrap a block demod as a `Windowed` kind, reading its window length from caps.
fn windowed(bd: Box<dyn BlockDemodulator>) -> DemodKind {
    let window_s = match bd.caps().shape {
        DemodShape::Windowed { window_s, .. } => window_s,
        _ => 15.0,
    };
    DemodKind::Windowed(bd, window_s)
}

/// Classify a mode config into the demod the RX worker should run.
pub fn demod_kind(cfg: &ModeConfig) -> DemodKind {
    match cfg {
        ModeConfig::None => DemodKind::None,
        ModeConfig::Afsk1200 { .. } => DemodKind::Streaming(Box::new(Afsk1200Demod::ensemble(9))),
        ModeConfig::Cw { wpm, tone_hz } => DemodKind::Streaming(Box::new(CwDemod::new(*wpm, *tone_hz))),
        ModeConfig::Rtty { baud, shift_hz, center_hz, reverse } => DemodKind::Streaming(Box::new(
            RttyDemod::with_center(*baud, *shift_hz, *center_hz).reversed(*reverse),
        )),
        ModeConfig::Psk31 { center_hz } => {
            DemodKind::Streaming(Box::new(Psk31Demod::new(*center_hz)))
        }
        ModeConfig::Ft8 => windowed(Box::new(Ft8Demod::new())),
        ModeConfig::Ft4 => windowed(Box::new(Ft4Demod::new())),
        ModeConfig::Jt65 => windowed(Box::new(Jt65Demod::new())),
        ModeConfig::Jt9 => windowed(Box::new(Jt9Demod::new())),
        ModeConfig::Wspr => windowed(Box::new(WsprDemod::new())),
        ModeConfig::Olivia { tones, bandwidth_hz } => {
            DemodKind::Streaming(Box::new(OliviaDemod::new(*tones, *bandwidth_hz)))
        }
    }
}

/// The RX demod's native sample rate for a mode, or `None` for `ModeConfig::None`
/// (no RX worker runs, so there is nothing to tap a spectrum from). This is the
/// rate of the samples the spectrum FFT sees — post-resample, mode-specific.
pub fn native_rate(cfg: &ModeConfig) -> Option<u32> {
    match demod_kind(cfg) {
        DemodKind::None => None,
        DemodKind::Streaming(d) => Some(d.caps().native_rate),
        DemodKind::Windowed(d, _) => Some(d.caps().native_rate),
    }
}

/// Build a modulator for a config, or `None` if the mode is receive-only.
pub fn build_modulator(cfg: &ModeConfig) -> Option<Box<dyn Modulator>> {
    match cfg {
        ModeConfig::None => Some(Box::new(NullMode)),
        ModeConfig::Afsk1200 { .. } => Some(Box::new(Afsk1200Mod::new())),
        ModeConfig::Cw { wpm, tone_hz } => Some(Box::new(CwMod::new(*wpm, *tone_hz))),
        ModeConfig::Rtty { baud, shift_hz, center_hz, .. } => {
            Some(Box::new(RttyMod::with_center(*baud, *shift_hz, *center_hz)))
        }
        ModeConfig::Psk31 { center_hz } => Some(Box::new(Psk31Mod::new(*center_hz))),
        ModeConfig::Ft8 => Some(Box::new(Ft8Mod::new())),
        ModeConfig::Ft4 => Some(Box::new(Ft4Mod::new())),
        ModeConfig::Jt65 => Some(Box::new(Jt65Mod::new())),
        ModeConfig::Jt9 => Some(Box::new(Jt9Mod::new())),
        ModeConfig::Wspr => Some(Box::new(WsprMod::new())),
        ModeConfig::Olivia { tones, bandwidth_hz } => {
            Some(Box::new(OliviaMod::new(*tones, *bandwidth_hz)))
        }
    }
}

/// The windowed-TX slot period for a mode, if it transmits on a time grid
/// (FT8's 15 s slots). `None` for streaming modes (transmit as soon as queued).
pub fn tx_slot_s(cfg: &ModeConfig) -> Option<f32> {
    match build_modulator(cfg)?.caps().shape {
        DemodShape::Windowed { period_s, .. } => Some(period_s),
        DemodShape::Streaming => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demod_kind_classifies_modes() {
        assert!(matches!(demod_kind(&ModeConfig::None), DemodKind::None));
        assert!(matches!(
            demod_kind(&ModeConfig::Afsk1200 { tx: true }),
            DemodKind::Streaming(_)
        ));
        assert!(matches!(
            demod_kind(&ModeConfig::Cw { wpm: 20, tone_hz: 700.0 }),
            DemodKind::Streaming(_)
        ));
        assert!(matches!(
            demod_kind(&ModeConfig::Ft8),
            DemodKind::Windowed(_, w) if (w - 15.0).abs() < 0.01
        ));
    }

    #[test]
    fn wsjtx_breadth_modes_are_windowed_with_modulators() {
        for (cfg, win) in [
            (ModeConfig::Ft4, 7.5f32),
            (ModeConfig::Jt65, 60.0),
            (ModeConfig::Jt9, 60.0),
            (ModeConfig::Wspr, 120.0),
        ] {
            assert!(
                matches!(demod_kind(&cfg), DemodKind::Windowed(_, w) if (w - win).abs() < 0.5),
                "{cfg:?} not windowed @ {win}"
            );
            assert!(build_modulator(&cfg).is_some(), "no modulator for {cfg:?}");
        }
        assert_eq!(tx_slot_s(&ModeConfig::Wspr), Some(120.0));
        assert_eq!(tx_slot_s(&ModeConfig::Ft4), Some(7.5));
    }

    #[test]
    fn olivia_is_streaming_with_a_modulator() {
        let cfg = ModeConfig::Olivia { tones: 32, bandwidth_hz: 1000 };
        assert!(matches!(demod_kind(&cfg), DemodKind::Streaming(_)));
        assert!(build_modulator(&cfg).is_some());
        assert_eq!(tx_slot_s(&cfg), None);
    }

    #[test]
    fn modulators_build_for_every_mode() {
        for cfg in [
            ModeConfig::None,
            ModeConfig::Afsk1200 { tx: true },
            ModeConfig::Ft8,
            ModeConfig::Cw { wpm: 20, tone_hz: 700.0 },
            ModeConfig::Rtty { baud: 45.45, shift_hz: 170.0, center_hz: 2210.0, reverse: false },
            ModeConfig::Psk31 { center_hz: 1000.0 },
        ] {
            assert!(build_modulator(&cfg).is_some(), "no modulator for {cfg:?}");
        }
    }

    #[test]
    fn only_ft8_has_a_tx_slot() {
        assert_eq!(tx_slot_s(&ModeConfig::Ft8), Some(15.0));
        assert_eq!(tx_slot_s(&ModeConfig::Afsk1200 { tx: true }), None);
        assert_eq!(tx_slot_s(&ModeConfig::Psk31 { center_hz: 1000.0 }), None);
    }
}
