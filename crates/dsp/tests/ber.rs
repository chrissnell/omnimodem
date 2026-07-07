//! Layer-2 performance: AWGN decode-rate sweeps with committed thresholds.
//! These are the BER/decode-rate curves the exit criterion requires; the
//! reference-oracle comparison (equal-or-better at every SNR) is an `#[ignore]`
//! gate in `kat.rs` pending the reference binaries. Thresholds here are CI
//! floors set just under the observed rate — their job is to catch regressions.
#![cfg(feature = "testutil")]

use omnimodem_dsp::mode::{BlockDemodulator, Demodulator, Modulator};
use omnimodem_dsp::modes::{
    afsk1200::{Afsk1200Demod, Afsk1200Mod},
    cw::{CwDemod, CwMod},
    fsq::{FsqDemod, FsqMod, FsqSpeed},
    ft8::{Ft8Demod, Ft8Mod, FT8_RATE, FT8_WINDOW_S},
    ifkp::{IfkpDemod, IfkpMod, IfkpSpeed},
    psk31::{Psk31Demod, Psk31Mod},
    rtty::{RttyDemod, RttyMod},
};
use omnimodem_dsp::modes::picture::PictureCodec;
use omnimodem_dsp::modes::{ifkp_pic, mfsk_pic};
use omnimodem_dsp::testutil::{add_awgn, decode_rate, Rng, WattersonChannel};
use omnimodem_dsp::types::{Frame, FramePayload};

fn has_text(frames: &[Frame], msg: &str) -> bool {
    frames.iter().any(|f| matches!(&f.payload, FramePayload::Text(t) if t.contains(msg)))
}

#[test]
fn afsk1200_decode_rate() {
    use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
    let ax = Ax25Frame {
        dest: Address::new("APRS", 0),
        source: Address::new("N0CALL", 1),
        digipeaters: vec![],
        info: b"ber sweep".to_vec(),
    };
    let want = ax.encode();
    let rate = decode_rate(20, |seed| {
        let mut s = Afsk1200Mod::new().modulate(&Frame::packet(want.clone())).unwrap();
        let mut rng = Rng::new(1 + seed as u64);
        add_awgn(&mut s, 0.20, &mut rng);
        Afsk1200Demod::ensemble(9)
            .feed(&s)
            .iter()
            .any(|f| matches!(&f.payload, FramePayload::Packet(b) if b == &want))
    });
    eprintln!("AFSK1200 decode rate @ sigma=0.20: {rate}");
    assert!(rate >= 0.85, "AFSK1200 decode rate {rate} below floor 0.85");
}

#[test]
fn psk31_decode_rate() {
    let msg = "CQ DE K1ABC";
    let rate = decode_rate(20, |seed| {
        let mut s = Psk31Mod::new(1000.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(100 + seed as u64);
        add_awgn(&mut s, 0.02, &mut rng);
        has_text(&Psk31Demod::new(1000.0).feed(&s), msg)
    });
    eprintln!("PSK31 decode rate @ sigma=0.02: {rate}");
    assert!(rate >= 0.9, "PSK31 decode rate {rate} below floor 0.9");
}

#[test]
fn rtty_decode_rate() {
    let msg = "CQ TEST DE N0CALL";
    let rate = decode_rate(20, |seed| {
        let mut s = RttyMod::new(45.45, 170.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(200 + seed as u64);
        add_awgn(&mut s, 0.05, &mut rng);
        has_text(&RttyDemod::new(45.45, 170.0).feed(&s), msg)
    });
    eprintln!("RTTY decode rate @ sigma=0.05: {rate}");
    assert!(rate >= 0.85, "RTTY decode rate {rate} below floor 0.85");
}

#[test]
fn cw_decode_rate() {
    let msg = "CQ TEST";
    let rate = decode_rate(20, |seed| {
        let mut s = CwMod::new(20, 700.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(300 + seed as u64);
        let mut lead = vec![0.0f32; 1600];
        add_awgn(&mut lead, 0.01, &mut rng);
        add_awgn(&mut s, 0.01, &mut rng);
        let mut rx = CwDemod::new(20, 700.0);
        // Words emit live during `feed` (trailing gap) or at finish — collect both.
        let mut frames = rx.feed(&lead);
        frames.extend(rx.feed(&s));
        frames.extend(rx.finish_text());
        let text: String = frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.to_uppercase()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        text.contains("CQ") && text.contains("TEST")
    });
    eprintln!("CW decode rate @ sigma=0.01: {rate}");
    assert!(rate >= 0.9, "CW decode rate {rate} below floor 0.9");
}

#[test]
fn ft8_decode_rate() {
    let msg = "CQ K1ABC FN42";
    let rate = decode_rate(8, |seed| {
        let wave = Ft8Mod::new().modulate(&Frame::text(msg)).unwrap();
        let mut win = vec![0.0f32; (FT8_RATE as f32 * FT8_WINDOW_S) as usize];
        win[..wave.len()].copy_from_slice(&wave);
        let mut rng = Rng::new(400 + seed as u64);
        add_awgn(&mut win, 0.3, &mut rng);
        has_text(&Ft8Demod::new().decode_window(&win, 0), msg)
    });
    eprintln!("FT8 decode rate @ sigma=0.30: {rate}");
    assert!(rate >= 0.85, "FT8 decode rate {rate} below floor 0.85");
}

/// Concatenate the per-character text frames a streaming keyboard demod emits.
fn joined(frames: &[Frame]) -> String {
    frames
        .iter()
        .filter_map(|f| match &f.payload {
            FramePayload::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn ifkp_decode_rate() {
    let msg = "CQ DE K1ABC";
    let rate = decode_rate(20, |seed| {
        let mut s = IfkpMod::new(IfkpSpeed::Normal, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0x1F00 + seed as u64);
        add_awgn(&mut s, 0.03, &mut rng);
        let mut rx = IfkpDemod::new(IfkpSpeed::Normal, 1500.0);
        let mut f = rx.feed(&s);
        f.extend(rx.flush());
        joined(&f).contains(msg)
    });
    eprintln!("IFKP decode rate @ sigma=0.03: {rate}");
    assert!(rate >= 0.85, "IFKP decode rate {rate} below floor 0.85");
}

#[test]
fn fsq_decode_rate() {
    let msg = "CQ DE K1ABC";
    let rate = decode_rate(20, |seed| {
        let mut s = FsqMod::new(FsqSpeed::S3, 1500.0, "", false).modulate(&Frame::text(msg)).unwrap();
        let mut rng = Rng::new(0xF500 + seed as u64);
        add_awgn(&mut s, 0.03, &mut rng);
        let mut rx = FsqDemod::new(FsqSpeed::S3, 1500.0, "");
        let mut f = rx.feed(&s);
        f.extend(rx.flush());
        joined(&f).contains(msg)
    });
    eprintln!("FSQ decode rate @ sigma=0.03: {rate}");
    assert!(rate >= 0.85, "FSQ decode rate {rate} below floor 0.85");
}

// ---------------------------------------------------------------------------
// Picture sub-protocols (Phase 15): raster fidelity vs SNR.
//
// The picture modes carry a raw pixel-FSK raster with no FEC, so "decode rate"
// is a per-pixel fidelity, not a frame CRC. This sweep encodes a grey ramp,
// adds AWGN at a spread of noise levels, and measures the fraction of pixels the
// noisy decode recovers within tolerance of the clean (noiseless) decode — the
// reference, since the FM discriminator's own quantisation is already in it.
// Committed floors are CI regression catches, one rung under the observed rate.
// ---------------------------------------------------------------------------

/// A grey ramp `w*h` raster as interleaved RGB (each pixel `v,v,v`).
fn grey_ramp(w: usize, h: usize) -> Vec<u8> {
    let total = w * h;
    let mut rgb = Vec::with_capacity(total * 3);
    for i in 0..total {
        let v = (i * 255 / (total - 1)) as u8;
        rgb.extend_from_slice(&[v, v, v]);
    }
    rgb
}

fn image_pixels(codec: &PictureCodec, audio: &[f32], w: usize, h: usize, spp: usize) -> Vec<u8> {
    match codec.decode(audio, w, h, false, spp).payload {
        FramePayload::Image { pixels, .. } => pixels,
        _ => panic!("picture decode did not yield an Image"),
    }
}

/// Mean fraction of pixels within `tol` of the clean decode, over `trials`
/// noisy realisations at noise std `sigma`.
fn raster_match_rate(
    codec: &PictureCodec,
    dims: (usize, usize, usize), // (width, height, samples-per-pixel)
    sigma: f32,
    tol: i32,
    seed0: u64,
    trials: usize,
) -> f64 {
    let (w, h, spp) = dims;
    let rgb = grey_ramp(w, h);
    let clean = image_pixels(codec, &codec.encode(&rgb, w, h, false, spp), w, h, spp);
    let mut acc = 0.0;
    for t in 0..trials {
        let mut audio = codec.encode(&rgb, w, h, false, spp);
        add_awgn(&mut audio, sigma, &mut Rng::new(seed0 + t as u64));
        let noisy = image_pixels(codec, &audio, w, h, spp);
        let matched = noisy
            .iter()
            .zip(&clean)
            .filter(|(a, b)| (**a as i32 - **b as i32).abs() <= tol)
            .count();
        acc += matched as f64 / clean.len() as f64;
    }
    acc / trials as f64
}

#[test]
fn mfsk_pic_raster_fidelity_vs_snr() {
    // MFSK16-class: 8 kHz, 1500 Hz carrier, ~316 Hz occupied band, spp=8.
    let codec = mfsk_pic::codec(1500.0, 316.0, 8000.0, false);
    let (w, h, spp) = (32usize, 8usize, 8usize);
    for &sigma in &[0.01f32, 0.05, 0.1, 0.2] {
        let rate = raster_match_rate(&codec, (w, h, spp), sigma, 16, 0xA10 + (sigma * 1000.0) as u64, 12);
        eprintln!("MFSK-pic raster match-rate @ sigma={sigma}: {rate:.3}");
        // Observed ~ 1.00 / 0.90 / 0.61 / 0.36; floors sit a rung under (CI catch).
        let floor = match sigma {
            s if s <= 0.01 => 0.99,
            s if s <= 0.05 => 0.85,
            s if s <= 0.1 => 0.55,
            _ => 0.30,
        };
        assert!(rate >= floor, "MFSK-pic match-rate {rate} @ sigma={sigma} below floor {floor}");
    }
}

#[test]
fn ifkp_pic_raster_fidelity_vs_snr() {
    // IFKP: 16 kHz, 1500 Hz carrier, ~386 Hz occupied band, spp=8 — the rate the
    // analytic front-end unblocked. Verifies the image-free discriminator holds
    // up under noise at the higher sample rate too.
    let codec = ifkp_pic::codec(1500.0, 386.0, false);
    let (w, h, spp) = (32usize, 8usize, ifkp_pic::SPP);
    for &sigma in &[0.01f32, 0.05, 0.1, 0.2] {
        let rate = raster_match_rate(&codec, (w, h, spp), sigma, 16, 0x1F10 + (sigma * 1000.0) as u64, 12);
        eprintln!("IFKP-pic raster match-rate @ sigma={sigma}: {rate:.3}");
        // Observed ~ 1.00 / 0.68 / 0.42 / 0.25 (noisier: 8 spp at 16 kHz); floors
        // a rung under. Still monotone + graceful, which is the conformance point.
        let floor = match sigma {
            s if s <= 0.01 => 0.99,
            s if s <= 0.05 => 0.58,
            s if s <= 0.1 => 0.35,
            _ => 0.20,
        };
        assert!(rate >= floor, "IFKP-pic match-rate {rate} @ sigma={sigma} below floor {floor}");
    }
}

/// Channel simulators, not just AWGN (design §"Channel simulators"): the modes
/// target fading HF channels, so AWGN-only testing overstates performance. This
/// exercises the seedable Watterson HF-fading fixture end-to-end on the robust
/// AFSK ensemble (amplitude fading + a delayed second path + light AWGN). It
/// makes the required fading fixture an actual gate rather than dead code.
#[test]
fn afsk1200_decode_rate_watterson_fading() {
    use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
    let ax = Ax25Frame {
        dest: Address::new("APRS", 0),
        source: Address::new("N0CALL", 2),
        digipeaters: vec![],
        info: b"watterson".to_vec(),
    };
    let want = ax.encode();
    let chan = WattersonChannel::ccir_good(48_000.0);
    let rate = decode_rate(20, |seed| {
        let clean = Afsk1200Mod::new().modulate(&Frame::packet(want.clone())).unwrap();
        let mut rng = Rng::new(500 + seed as u64);
        let mut faded = chan.apply(&clean, &mut rng);
        add_awgn(&mut faded, 0.05, &mut rng);
        Afsk1200Demod::ensemble(9)
            .feed(&faded)
            .iter()
            .any(|f| matches!(&f.payload, FramePayload::Packet(b) if b == &want))
    });
    eprintln!("AFSK1200 decode rate over CCIR-good Watterson fading: {rate}");
    assert!(rate >= 0.8, "AFSK1200 fading decode rate {rate} below floor 0.8");
}
