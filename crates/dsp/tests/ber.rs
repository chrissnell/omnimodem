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
    ft8::{Ft8Demod, Ft8Mod, FT8_RATE, FT8_WINDOW_S},
    psk31::{Psk31Demod, Psk31Mod},
    rtty::{RttyDemod, RttyMod},
};
use omnimodem_dsp::testutil::{add_awgn, decode_rate, Rng};
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
        rx.feed(&lead);
        rx.feed(&s);
        has_text(&rx.finish_text(), msg)
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
