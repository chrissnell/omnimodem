//! Layer-1 conformance: modulator golden snapshots (design §"Modulator golden
//! snapshots"). Each modulator renders a *fixed* input; we snapshot a quantized
//! fingerprint (first 256 samples as i16) so any change to on-air output is
//! caught in review. Regenerate intentionally with `INSTA_UPDATE=always`.

use omnimodem_dsp::frontend::modulate::{Afsk, CwKeyer, DiffPsk, Fsk2, Gfsk, MFsk};

/// Quantize the leading `n` samples to i16 for a stable, reviewable fingerprint.
fn fingerprint(samples: &[f32], n: usize) -> Vec<i16> {
    samples
        .iter()
        .take(n)
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect()
}

macro_rules! snap {
    ($name:literal, $fp:expr) => {
        insta::with_settings!({ snapshot_path => "vectors", prepend_module_to_snapshot => false }, {
            insta::assert_json_snapshot!($name, $fp);
        });
    };
}

#[test]
fn gfsk_8fsk_fingerprint() {
    // FT8-like 8-FSK, Gaussian BT=2.0, 32 samples/symbol at 12 kHz.
    let m = Gfsk::new(12_000.0, 32, 500.0, 6.25, 2.0);
    let fp = fingerprint(&m.modulate(&[0, 3, 1, 4, 0, 6, 5, 2]), 256);
    snap!("gfsk_8fsk", &fp);
}

#[test]
fn mfsk_tone_bank_fingerprint() {
    let m = MFsk::new(8_000.0, 16, 1000.0, 15.625, 16);
    let fp = fingerprint(&m.modulate(&[0, 5, 10, 15, 7, 2, 11, 1, 14, 3, 9, 6, 0, 12, 8, 4]), 256);
    snap!("mfsk16", &fp);
}

#[test]
fn fsk2_rtty_fingerprint() {
    // RTTY-like 2-FSK, 170 Hz shift.
    let m = Fsk2::new(8_000.0, 32, 1500.0, 170.0);
    let fp = fingerprint(&m.modulate(&[true, false, true, true, false, false, true, false]), 256);
    snap!("fsk2_rtty", &fp);
}

#[test]
fn afsk_bell202_fingerprint() {
    let m = Afsk::bell202(48_000.0);
    let fp = fingerprint(&m.modulate(&[false, true, false, true, true, false]), 256);
    snap!("afsk_bell202", &fp);
}

#[test]
fn diff_bpsk_fingerprint() {
    // PSK31-like differential BPSK.
    let m = DiffPsk::new(8_000.0, 1000.0, 32, 1);
    let fp = fingerprint(&m.modulate(&[0, 1, 1, 0, 1, 0, 0, 1]), 256);
    snap!("diff_bpsk", &fp);
}

#[test]
fn cw_keyer_fingerprint() {
    // "CQ" in Morse elements: C = -.-.  Q = --.-  (space = letter gap).
    let m = CwKeyer::new(8_000.0, 700.0, 30.0);
    let fp = fingerprint(&m.modulate("-.-. --.-"), 256);
    snap!("cw_cq", &fp);
}

// --- Phase-4 mode-level modulator fingerprints --------------------------------
// These snapshot the *assembly*-level on-air output (mode framing + FEC +
// modulation), not just the raw Phase-3 modulator blocks above, so any change to
// a mode's encoding is caught in review.

use omnimodem_dsp::mode::Modulator;
use omnimodem_dsp::modes::{
    afsk1200::Afsk1200Mod, cw::CwMod, ft8::Ft8Mod, psk31::Psk31Mod, rtty::RttyMod,
};
use omnimodem_dsp::types::Frame;

#[test]
fn afsk1200_frame_fingerprint() {
    use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
    let ax = Ax25Frame {
        dest: Address::new("APRS", 0),
        source: Address::new("K1ABC", 0),
        digipeaters: vec![],
        info: b"snap".to_vec(),
    };
    let fp = fingerprint(&Afsk1200Mod::new().modulate(&Frame::packet(ax.encode())).unwrap(), 256);
    snap!("afsk1200_frame", &fp);
}

#[test]
fn psk31_message_fingerprint() {
    let fp = fingerprint(&Psk31Mod::new(1000.0).modulate(&Frame::text("CQ")).unwrap(), 256);
    snap!("psk31_message", &fp);
}

#[test]
fn rtty_message_fingerprint() {
    let fp = fingerprint(&RttyMod::new(45.45, 170.0).modulate(&Frame::text("RYRY")).unwrap(), 256);
    snap!("rtty_message", &fp);
}

#[test]
fn cw_message_fingerprint() {
    let fp = fingerprint(&CwMod::new(20, 700.0).modulate(&Frame::text("CQ TEST")).unwrap(), 256);
    snap!("cw_message", &fp);
}

#[test]
fn ft8_message_fingerprint() {
    let fp = fingerprint(&Ft8Mod::new().modulate(&Frame::text("CQ K1ABC FN42")).unwrap(), 256);
    snap!("ft8_message", &fp);
}
