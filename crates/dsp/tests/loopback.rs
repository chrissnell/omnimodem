//! Layer-1 conformance: each mode's own modulator → demodulator round-trip, as
//! a single CI-visible target. The decisive *cross*-decode against reference
//! binaries is in `kat.rs` (gated `#[ignore]`); this proves internal
//! self-consistency: a frame we transmit, we decode back exactly.

use omnimodem_dsp::mode::{BlockDemodulator, Demodulator, Modulator};
use omnimodem_dsp::modes::{
    afsk1200::{Afsk1200Demod, Afsk1200Mod},
    cw::{CwDemod, CwMod},
    ft8::{Ft8Demod, Ft8Mod, FT8_RATE, FT8_WINDOW_S},
    psk31::{Psk31Demod, Psk31Mod},
    rtty::{RttyDemod, RttyMod},
};
use omnimodem_dsp::types::{Frame, FramePayload};

fn texts(frames: &[Frame]) -> String {
    frames
        .iter()
        .filter_map(|f| match &f.payload {
            FramePayload::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn afsk1200_loopback() {
    use omnimodem_dsp::framing::ax25::{Address, Ax25Frame};
    let ax = Ax25Frame {
        dest: Address::new("APRS", 0),
        source: Address::new("K1ABC", 7),
        digipeaters: vec![],
        info: b"loopback".to_vec(),
    };
    let s = Afsk1200Mod::new().modulate(&Frame::packet(ax.encode())).unwrap();
    let frames = Afsk1200Demod::ensemble(9).feed(&s);
    assert!(
        frames.iter().any(|f| matches!(&f.payload, FramePayload::Packet(b) if b == &ax.encode())),
        "AFSK1200 loopback did not recover the frame"
    );
}

#[test]
fn psk31_loopback() {
    let msg = "CQ DE K1ABC";
    let s = Psk31Mod::new(1000.0).modulate(&Frame::text(msg)).unwrap();
    let frames = Psk31Demod::new(1000.0).feed(&s);
    assert!(texts(&frames).contains(msg), "PSK31 loopback failed");
}

#[test]
fn rtty_loopback() {
    let msg = "THE QUICK BROWN FOX";
    let s = RttyMod::new(45.45, 170.0).modulate(&Frame::text(msg)).unwrap();
    let frames = RttyDemod::new(45.45, 170.0).feed(&s);
    assert!(texts(&frames).contains(msg), "RTTY loopback failed");
}

#[test]
fn cw_loopback() {
    use omnimodem_dsp::testutil::{add_awgn, Rng};
    let msg = "CQ TEST";
    let mut s = CwMod::new(20, 700.0).modulate(&Frame::text(msg)).unwrap();
    // The adaptive squelch needs a noise floor to gate against.
    let mut rng = Rng::new(1);
    let mut lead = vec![0.0f32; 1600];
    add_awgn(&mut lead, 0.02, &mut rng);
    add_awgn(&mut s, 0.02, &mut rng);
    let mut rx = CwDemod::new(20, 700.0);
    rx.feed(&lead);
    rx.feed(&s);
    let frames = rx.finish_text();
    assert!(texts(&frames).to_uppercase().contains(msg), "CW loopback failed");
}

#[test]
fn ft8_loopback() {
    let msg = "CQ K1ABC FN42";
    let wave = Ft8Mod::new().modulate(&Frame::text(msg)).unwrap();
    let mut win = vec![0.0f32; (FT8_RATE as f32 * FT8_WINDOW_S) as usize];
    win[..wave.len()].copy_from_slice(&wave);
    let decodes = Ft8Demod::new().decode_window(&win, 0);
    assert!(texts(&decodes).contains(msg), "FT8 loopback failed");
}
