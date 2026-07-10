//! GRA-257 regression: the weak-signal modes (JT65/JT9/FST4/WSPR) reject a
//! NONSTANDARD callsign such as the TUI's default placeholder `N0CALL` (4-letter
//! suffix), which is why a fresh/unconfigured station saw them go silent while
//! FT8/FT4 (whose total packer tolerates it) transmitted. A VALID call encodes in
//! every mode, even with the default `AA00` grid.
use omnimodem_dsp::mode::Modulator;
use omnimodem_dsp::modes::{fst4::Fst4Mod, ft4::Ft4Mod, ft8::Ft8Mod, jt65::Jt65Mod, jt9::Jt9Mod, wspr::WsprMod};
use omnimodem_dsp::types::Frame;

fn encodes(m: &mut dyn Modulator, msg: &str) -> bool {
    m.modulate(&Frame::text(msg)).is_ok()
}

#[test]
fn valid_call_encodes_in_every_mode_with_aa00_grid() {
    assert!(encodes(&mut Ft8Mod::new(), "CQ K7RA AA00"));
    assert!(encodes(&mut Ft4Mod::new(), "CQ K7RA AA00"));
    assert!(encodes(&mut Jt65Mod::new(), "CQ K7RA AA00"));
    assert!(encodes(&mut Jt9Mod::new(), "CQ K7RA AA00"));
    assert!(encodes(&mut Fst4Mod::new(15), "CQ K7RA AA00"));
    assert!(encodes(&mut WsprMod::new(), "K7RA AA00 37"));
}

#[test]
fn default_placeholder_call_is_rejected_by_weak_signal_modes() {
    // FT8/FT4 tolerate the nonstandard N0CALL (total packer)...
    assert!(encodes(&mut Ft8Mod::new(), "CQ N0CALL AA00"));
    assert!(encodes(&mut Ft4Mod::new(), "CQ N0CALL AA00"));
    // ...but the weak-signal modes cannot encode it. Their TX worker now reports
    // TransmitFailed instead of going silent (see omnimodemd tx_worker tests).
    assert!(!encodes(&mut Jt65Mod::new(), "CQ N0CALL AA00"));
    assert!(!encodes(&mut Jt9Mod::new(), "CQ N0CALL AA00"));
    assert!(!encodes(&mut Fst4Mod::new(15), "CQ N0CALL AA00"));
    assert!(!encodes(&mut WsprMod::new(), "N0CALL AA00 37"));
}
