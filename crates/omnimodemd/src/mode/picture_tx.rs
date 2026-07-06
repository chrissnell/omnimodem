//! Daemon picture-send dispatch (Phase 15 T6). Maps a channel's configured
//! [`ModeConfig`] plus a requested raster onto the matching per-family
//! picture-send assembler in `omnimodem-dsp`, returning the complete on-air audio
//! (in-band header + pixel-FSK) the core keys the rig and plays. This is the one
//! place the daemon learns which modes can carry a picture; the RX side already
//! emits typed `Image` frames, so this closes the symmetric TX surface.
//!
//! MFSK carries an explicit `W×H` in its header, so any raster size is valid;
//! THOR / IFKP / FSQ select from a fixed size table, so the request's dimensions
//! must match one of the mode's sizes (and, for FSQ, its colour).

use super::ModeConfig;
use omnimodem_dsp::modes::picture::RasterRef;
use omnimodem_dsp::modes::{fsq, fsq_pic, ifkp, ifkp_pic, mfsk, mfsk_pic, thor, thor_pic};

/// A picture-send request: the raster plus the options a picture TX needs beyond
/// what the channel's `ModeConfig` already fixes (submode, carrier, callsign).
#[derive(Debug, Clone, PartialEq)]
pub struct PictureSend {
    /// Row-major interleaved RGB (`R,G,B,…`), `width*height*3` bytes.
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Send colour (3 planes) vs grey (one luma byte per pixel).
    pub color: bool,
    /// MFSK samples-per-pixel selector (8 default, or the faster 4 / 2); ignored
    /// by the fixed-timing THOR / IFKP / FSQ families.
    pub txspp: u8,
}

/// Why a picture send could not be assembled for a channel's mode.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PictureError {
    #[error("mode '{0}' does not support picture transmit")]
    Unsupported(String),
    #[error("'{0}' is not a valid picture submode")]
    BadSubmode(String),
    #[error("image {w}x{h} (colour={color}) is not a supported size for this mode")]
    BadSize { w: u32, h: u32, color: bool },
    #[error("image byte length {got} != width*height*3 ({want})")]
    BadLength { got: usize, want: usize },
}

/// Build the complete picture-send audio for `cfg`'s configured mode, returning
/// `(audio, native_rate_hz)` (the worker plays the samples at that rate), or an
/// error if the mode / submode / size cannot carry this image.
pub fn build(cfg: &ModeConfig, send: &PictureSend) -> Result<(Vec<f32>, u32), PictureError> {
    let want = send.width as usize * send.height as usize * 3;
    if send.rgb.len() != want {
        return Err(PictureError::BadLength { got: send.rgb.len(), want });
    }
    let grey = !send.color;
    let bad_size = || PictureError::BadSize { w: send.width, h: send.height, color: send.color };

    match cfg {
        ModeConfig::Mfsk { submode, center_hz } => {
            let v = mfsk::MfskVariant::from_label(submode)
                .ok_or_else(|| PictureError::BadSubmode(submode.clone()))?;
            // MFSK advertises W×H in the header, so any size is valid; only 8/4/2
            // spp exist (any other value falls back to 8, as the RX parser does).
            let spp = match send.txspp {
                2 => 2,
                4 => 4,
                _ => 8,
            };
            let img = RasterRef { rgb: &send.rgb, width: send.width, height: send.height };
            Ok((mfsk_pic::build_tx(v, *center_hz, img, send.color, spp, false), v.samplerate()))
        }
        ModeConfig::Thor { submode, center_hz } => {
            let v = thor::ThorVariant::from_label(submode)
                .ok_or_else(|| PictureError::BadSubmode(submode.clone()))?;
            let size = thor_pic::ThorPicSize::from_dims(send.width, send.height).ok_or_else(bad_size)?;
            Ok((thor_pic::build_tx(v, *center_hz, size, &send.rgb, grey, false), v.samplerate()))
        }
        ModeConfig::Ifkp { speed, center_hz } => {
            let s = ifkp::IfkpSpeed::from_label(speed)
                .ok_or_else(|| PictureError::BadSubmode(speed.clone()))?;
            let size = ifkp_pic::IfkpPicSize::from_dims(send.width, send.height).ok_or_else(bad_size)?;
            Ok((ifkp_pic::build_tx(s, *center_hz, size, &send.rgb, grey, false), ifkp_pic::SAMPLE_RATE as u32))
        }
        ModeConfig::Fsq { speed, center_hz, mycall, .. } => {
            let s = fsq::FsqSpeed::from_label(speed)
                .ok_or_else(|| PictureError::BadSubmode(speed.clone()))?;
            let mode = fsq_pic::FsqPicMode::from_dims(send.width, send.height, grey)
                .ok_or_else(bad_size)?;
            Ok((fsq_pic::build_tx(s, *center_hz, mycall, mode, &send.rgb), fsq_pic::SAMPLE_RATE as u32))
        }
        other => Err(PictureError::Unsupported(other.to_mode_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey_ramp(w: u32, h: u32) -> Vec<u8> {
        let total = (w * h) as usize;
        let mut rgb = Vec::with_capacity(total * 3);
        for i in 0..total {
            let g = (i * 255 / total.max(2).saturating_sub(1).max(1)) as u8;
            rgb.extend_from_slice(&[g, g, g]);
        }
        rgb
    }

    fn send(w: u32, h: u32, color: bool) -> PictureSend {
        PictureSend { rgb: grey_ramp(w, h), width: w, height: h, color, txspp: 8 }
    }

    #[test]
    fn mfsk_builds_any_size() {
        let cfg = ModeConfig::Mfsk { submode: "mfsk16".into(), center_hz: 1500.0 };
        let (audio, rate) = build(&cfg, &send(16, 4, false)).expect("mfsk picture builds");
        assert!(!audio.is_empty());
        assert_eq!(rate, 8000, "mfsk16 native rate");
    }

    #[test]
    fn mfsk_rejects_bad_submode() {
        let cfg = ModeConfig::Mfsk { submode: "mfsk999".into(), center_hz: 1500.0 };
        assert_eq!(
            build(&cfg, &send(16, 4, false)),
            Err(PictureError::BadSubmode("mfsk999".into()))
        );
    }

    #[test]
    fn thor_builds_a_table_size_and_rejects_others() {
        let cfg = ModeConfig::Thor { submode: "thor16".into(), center_hz: 1500.0 };
        // 59×74 is the THOR "Thumb" size.
        assert!(build(&cfg, &send(59, 74, false)).is_ok());
        // 17×3 is not in the table.
        assert_eq!(
            build(&cfg, &send(17, 3, false)),
            Err(PictureError::BadSize { w: 17, h: 3, color: false })
        );
    }

    #[test]
    fn ifkp_builds_a_table_size() {
        let cfg = ModeConfig::Ifkp { speed: "ifkp".into(), center_hz: 1500.0 };
        assert!(build(&cfg, &send(59, 74, false)).is_ok());
    }

    #[test]
    fn fsq_builds_a_table_size_matching_colour() {
        let cfg = ModeConfig::Fsq {
            speed: "fsq".into(),
            center_hz: 1500.0,
            mycall: "k1abc".into(),
            directed: true,
        };
        // 120×150 grey is FSQ MiniGrey; the same dims in colour is MiniColor.
        assert!(build(&cfg, &send(120, 150, false)).is_ok());
        assert!(build(&cfg, &send(120, 150, true)).is_ok());
        // 59×74 is not an FSQ size.
        assert_eq!(
            build(&cfg, &send(59, 74, false)),
            Err(PictureError::BadSize { w: 59, h: 74, color: false })
        );
    }

    #[test]
    fn unsupported_mode_is_rejected() {
        let cfg = ModeConfig::Cw { wpm: 20, tone_hz: 700.0 };
        match build(&cfg, &send(16, 4, false)) {
            Err(PictureError::Unsupported(m)) => assert!(m.starts_with("cw"), "got {m:?}"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn wrong_byte_length_is_rejected() {
        let cfg = ModeConfig::Mfsk { submode: "mfsk16".into(), center_hz: 1500.0 };
        let bad = PictureSend { rgb: vec![0; 10], width: 16, height: 4, color: false, txspp: 8 };
        assert_eq!(build(&cfg, &bad), Err(PictureError::BadLength { got: 10, want: 192 }));
    }
}
