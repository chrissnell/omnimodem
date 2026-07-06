//! The one-module mode registry. Adding a mode in Phase 4 adds one arm here and
//! its assembly module in `omnimodem-dsp`; nothing else in the daemon learns
//! mode-specific details.

use super::{ModeConfig, NullMode};
use omnimodem_dsp::mode::{BlockDemodulator, DemodShape, Demodulator, Modulator};
use omnimodem_dsp::modes::{
    afsk1200::{Afsk1200Demod, Afsk1200Mod},
    contestia::{ContestiaDemod, ContestiaMod},
    cw::{CwDemod, CwMod},
    dominoex::{DominoDemod, DominoMod, DominoVariant},
    fsq::{FsqDemod, FsqMod, FsqSpeed},
    fst4::{Fst4Demod, Fst4Mod},
    ft4::{Ft4Demod, Ft4Mod},
    ft8::{Ft8Demod, Ft8Mod},
    hell::{HellDemod, HellMod, HellVariant},
    ifkp::{IfkpDemod, IfkpMod, IfkpSpeed},
    jt65::{Jt65Demod, Jt65Mod},
    jt9::{Jt9Demod, Jt9Mod},
    mfsk::{MfskDemod, MfskMod, MfskVariant},
    mt63::{Mt63Demod, Mt63Mod, Mt63Variant},
    olivia::{OliviaDemod, OliviaMod},
    psk::{PskDemod, PskMod, PskVariant},
    rtty::{RttyDemod, RttyMod},
    thor::{ThorDemod, ThorMod, ThorVariant},
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
        ModeConfig::Psk { submode, center_hz } => {
            let v = PskVariant::from_label(submode).expect("validated by ModeConfig::parse");
            DemodKind::Streaming(Box::new(PskDemod::new(v, *center_hz)))
        }
        ModeConfig::DominoEx { submode, center_hz } => {
            let v = DominoVariant::from_label(submode).expect("validated by ModeConfig::parse");
            DemodKind::Streaming(Box::new(DominoDemod::new(v, *center_hz)))
        }
        ModeConfig::Thor { submode, center_hz } => {
            let v = ThorVariant::from_label(submode).expect("validated by ModeConfig::parse");
            DemodKind::Streaming(Box::new(ThorDemod::new(v, *center_hz)))
        }
        ModeConfig::Ifkp { speed, center_hz } => {
            let s = IfkpSpeed::from_label(speed).expect("validated by ModeConfig::parse");
            DemodKind::Streaming(Box::new(IfkpDemod::new(s, *center_hz)))
        }
        ModeConfig::Fsq { speed, center_hz, mycall, .. } => {
            let s = FsqSpeed::from_label(speed).expect("validated by ModeConfig::parse");
            DemodKind::Streaming(Box::new(FsqDemod::new(s, *center_hz, mycall.clone())))
        }
        ModeConfig::Hell { submode, center_hz } => {
            let v = HellVariant::from_label(submode).expect("validated by ModeConfig::parse");
            DemodKind::Streaming(Box::new(HellDemod::new(v, *center_hz)))
        }
        ModeConfig::Mfsk { submode, center_hz } => {
            let v = MfskVariant::from_label(submode).expect("validated by ModeConfig::parse");
            DemodKind::Streaming(Box::new(MfskDemod::new(v, *center_hz)))
        }
        ModeConfig::Mt63 { submode, center_hz } => {
            let v = Mt63Variant::from_label(submode).expect("validated by ModeConfig::parse");
            DemodKind::Streaming(Box::new(Mt63Demod::new(v, *center_hz)))
        }
        ModeConfig::Contestia { tones, bandwidth_hz } => {
            DemodKind::Streaming(Box::new(ContestiaDemod::new(*tones, *bandwidth_hz)))
        }
        ModeConfig::Ft8 => windowed(Box::new(Ft8Demod::new())),
        ModeConfig::Ft4 => windowed(Box::new(Ft4Demod::new())),
        ModeConfig::Jt65 => windowed(Box::new(Jt65Demod::new())),
        ModeConfig::Jt9 => windowed(Box::new(Jt9Demod::new())),
        ModeConfig::Wspr => windowed(Box::new(WsprDemod::new())),
        ModeConfig::Fst4 { tr_s } => windowed(Box::new(Fst4Demod::new(*tr_s as u32))),
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
        ModeConfig::Psk { submode, center_hz } => {
            let v = PskVariant::from_label(submode).expect("validated by ModeConfig::parse");
            Some(Box::new(PskMod::new(v, *center_hz)))
        }
        ModeConfig::DominoEx { submode, center_hz } => {
            let v = DominoVariant::from_label(submode).expect("validated by ModeConfig::parse");
            Some(Box::new(DominoMod::new(v, *center_hz)))
        }
        ModeConfig::Thor { submode, center_hz } => {
            let v = ThorVariant::from_label(submode).expect("validated by ModeConfig::parse");
            Some(Box::new(ThorMod::new(v, *center_hz)))
        }
        ModeConfig::Ifkp { speed, center_hz } => {
            let s = IfkpSpeed::from_label(speed).expect("validated by ModeConfig::parse");
            Some(Box::new(IfkpMod::new(s, *center_hz)))
        }
        ModeConfig::Fsq { speed, center_hz, mycall, directed } => {
            let s = FsqSpeed::from_label(speed).expect("validated by ModeConfig::parse");
            Some(Box::new(FsqMod::new(s, *center_hz, mycall.clone(), *directed)))
        }
        ModeConfig::Hell { submode, center_hz } => {
            let v = HellVariant::from_label(submode).expect("validated by ModeConfig::parse");
            Some(Box::new(HellMod::new(v, *center_hz)))
        }
        ModeConfig::Mfsk { submode, center_hz } => {
            let v = MfskVariant::from_label(submode).expect("validated by ModeConfig::parse");
            Some(Box::new(MfskMod::new(v, *center_hz)))
        }
        ModeConfig::Mt63 { submode, center_hz } => {
            let v = Mt63Variant::from_label(submode).expect("validated by ModeConfig::parse");
            Some(Box::new(Mt63Mod::new(v, *center_hz)))
        }
        ModeConfig::Contestia { tones, bandwidth_hz } => {
            Some(Box::new(ContestiaMod::new(*tones, *bandwidth_hz)))
        }
        ModeConfig::Ft8 => Some(Box::new(Ft8Mod::new())),
        ModeConfig::Ft4 => Some(Box::new(Ft4Mod::new())),
        ModeConfig::Jt65 => Some(Box::new(Jt65Mod::new())),
        ModeConfig::Jt9 => Some(Box::new(Jt9Mod::new())),
        ModeConfig::Wspr => Some(Box::new(WsprMod::new())),
        ModeConfig::Fst4 { tr_s } => Some(Box::new(Fst4Mod::new(*tr_s as u32))),
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
    fn fst4_is_windowed_with_a_modulator_per_tr_period() {
        for (tr, win) in [(15u16, 15.0f32), (60, 60.0), (120, 120.0)] {
            let cfg = ModeConfig::Fst4 { tr_s: tr };
            assert!(
                matches!(demod_kind(&cfg), DemodKind::Windowed(_, w) if (w - win).abs() < 0.5),
                "fst4 tr={tr} not windowed @ {win}"
            );
            assert!(build_modulator(&cfg).is_some(), "no modulator for fst4 tr={tr}");
            assert_eq!(tx_slot_s(&cfg), Some(win));
        }
        // Parses and round-trips through the canonical mode string.
        assert_eq!(ModeConfig::parse("fst4"), Some(ModeConfig::Fst4 { tr_s: 15 }));
        assert_eq!(ModeConfig::parse("fst4:tr=120"), Some(ModeConfig::Fst4 { tr_s: 120 }));
        assert_eq!(ModeConfig::Fst4 { tr_s: 60 }.to_mode_string(), "fst4:tr=60");
    }

    #[test]
    fn dominoex_family_is_streaming_with_modulators() {
        for label in ["dominoexmicro", "dominoex4", "dominoex16", "dominoex88"] {
            let cfg = ModeConfig::DominoEx { submode: label.into(), center_hz: 1500.0 };
            assert!(matches!(demod_kind(&cfg), DemodKind::Streaming(_)), "{label} not streaming");
            assert!(build_modulator(&cfg).is_some(), "no modulator for {label}");
            assert_eq!(tx_slot_s(&cfg), None);
        }
        // The native RX rate follows the submode (8 kHz vs 11.025 kHz).
        assert_eq!(
            native_rate(&ModeConfig::DominoEx { submode: "dominoex16".into(), center_hz: 1500.0 }),
            Some(8000)
        );
        assert_eq!(
            native_rate(&ModeConfig::DominoEx { submode: "dominoex22".into(), center_hz: 1500.0 }),
            Some(11025)
        );
    }

    #[test]
    fn ifkp_family_is_streaming_with_modulators() {
        for label in ["ifkp", "ifkp-slow", "ifkp-fast"] {
            let cfg = ModeConfig::parse(label).expect("ifkp parses");
            assert!(matches!(demod_kind(&cfg), DemodKind::Streaming(_)), "{label} not streaming");
            assert!(build_modulator(&cfg).is_some(), "no modulator for {label}");
            assert_eq!(tx_slot_s(&cfg), None);
            assert_eq!(native_rate(&cfg), Some(16000));
        }
        assert_eq!(
            ModeConfig::parse("ifkp"),
            Some(ModeConfig::Ifkp { speed: "ifkp".into(), center_hz: 1500.0 })
        );
    }

    #[test]
    fn fsq_family_is_streaming_with_modulators() {
        for label in ["fsq", "fsq-1.5", "fsq-2", "fsq-4.5", "fsq-6"] {
            let cfg = ModeConfig::parse(label).expect("fsq parses");
            assert!(matches!(demod_kind(&cfg), DemodKind::Streaming(_)), "{label} not streaming");
            assert!(build_modulator(&cfg).is_some(), "no modulator for {label}");
            assert_eq!(tx_slot_s(&cfg), None);
            assert_eq!(native_rate(&cfg), Some(12000));
        }
        // The directed header params parse and round-trip.
        let cfg = ModeConfig::parse("fsq:mycall=k1abc,directed=true").expect("fsq parses");
        assert_eq!(
            cfg,
            ModeConfig::Fsq {
                speed: "fsq".into(),
                center_hz: 1500.0,
                mycall: "k1abc".into(),
                directed: true,
            }
        );
    }

    #[test]
    fn thor_family_is_streaming_with_modulators() {
        for label in ["thormicro", "thor4", "thor16", "thor25x4", "thor100"] {
            let cfg = ModeConfig::Thor { submode: label.into(), center_hz: 1500.0 };
            assert!(matches!(demod_kind(&cfg), DemodKind::Streaming(_)), "{label} not streaming");
            assert!(build_modulator(&cfg).is_some(), "no modulator for {label}");
            assert_eq!(tx_slot_s(&cfg), None);
        }
        // The native RX rate follows the submode (8 kHz vs 11.025 kHz).
        assert_eq!(
            native_rate(&ModeConfig::Thor { submode: "thor16".into(), center_hz: 1500.0 }),
            Some(8000)
        );
        assert_eq!(
            native_rate(&ModeConfig::Thor { submode: "thor22".into(), center_hz: 1500.0 }),
            Some(11025)
        );
    }

    #[test]
    fn hell_family_is_streaming_with_modulators() {
        for label in ["feldhell", "slowhell", "hellx5", "hellx9", "hell80"] {
            let cfg = ModeConfig::Hell { submode: label.into(), center_hz: 1500.0 };
            assert!(matches!(demod_kind(&cfg), DemodKind::Streaming(_)), "{label} not streaming");
            assert!(build_modulator(&cfg).is_some(), "no modulator for {label}");
            assert_eq!(tx_slot_s(&cfg), None);
            assert_eq!(native_rate(&cfg), Some(8000));
        }
    }

    #[test]
    fn mfsk_family_is_streaming_with_modulators() {
        for label in ["mfsk4", "mfsk8", "mfsk16", "mfsk31", "mfsk128", "mfsk64l"] {
            let cfg = ModeConfig::Mfsk { submode: label.into(), center_hz: 1500.0 };
            assert!(matches!(demod_kind(&cfg), DemodKind::Streaming(_)), "{label} not streaming");
            assert!(build_modulator(&cfg).is_some(), "no modulator for {label}");
            assert_eq!(tx_slot_s(&cfg), None);
        }
        // The native RX rate follows the submode (8 kHz vs 11.025 kHz).
        assert_eq!(
            native_rate(&ModeConfig::Mfsk { submode: "mfsk16".into(), center_hz: 1500.0 }),
            Some(8000)
        );
        assert_eq!(
            native_rate(&ModeConfig::Mfsk { submode: "mfsk11".into(), center_hz: 1500.0 }),
            Some(11025)
        );
    }

    #[test]
    fn contestia_grid_is_streaming_with_modulators() {
        for (t, bw) in [(4u16, 250u16), (8, 500), (16, 1000), (32, 1000), (64, 2000)] {
            let cfg = ModeConfig::Contestia { tones: t, bandwidth_hz: bw };
            assert!(matches!(demod_kind(&cfg), DemodKind::Streaming(_)), "{t}/{bw} not streaming");
            assert!(build_modulator(&cfg).is_some(), "no modulator for {t}/{bw}");
            assert_eq!(tx_slot_s(&cfg), None);
        }
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
            ModeConfig::Psk { submode: "psk31".into(), center_hz: 1000.0 },
        ] {
            assert!(build_modulator(&cfg).is_some(), "no modulator for {cfg:?}");
        }
    }

    #[test]
    fn only_ft8_has_a_tx_slot() {
        assert_eq!(tx_slot_s(&ModeConfig::Ft8), Some(15.0));
        assert_eq!(tx_slot_s(&ModeConfig::Afsk1200 { tx: true }), None);
        assert_eq!(tx_slot_s(&ModeConfig::Psk { submode: "psk31".into(), center_hz: 1000.0 }), None);
    }
}
