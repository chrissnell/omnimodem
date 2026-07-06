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
