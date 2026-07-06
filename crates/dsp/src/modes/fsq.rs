//! FSQ (FSQCALL) — 33-tone IFK MFSK with the self-framing FSQ Varicode, a
//! CRC8-keyed directed-message protocol, and five keyboard speeds.
//!
//! Port of fldigi's FSQ modem (`fldigi/src/fsq/{fsq.cxx,fsq_varicode.cxx}`,
//! `include/{fsq.h,crc8.h}`, upstream 4.2.x @ ../fldigi). FSQ is IFK over 33
//! tones with `tone = (prev + sym + 1) % 33` (fsq.cxx:1352-1356), text carried by
//! the FSQ Varicode (`framing::fsq_varicode`). `SR = 12000`, `FSQ_SYMLEN = 4096`
//! (the fixed tone-spacing reference), `spacing = 3` bins → `8.79 Hz`; the five
//! speeds fix only the emitted samples/symbol (fsq.cxx:312-327).
//!
//! A transmission is framed `" " + FSQBOL + mycall + ":" + crc8(mycall) + body +
//! FSQEOT` in directed mode (fsq.cxx:1482-1521); the receiver decodes the monitor
//! character stream and, on EOT, the `directed` submodule parses the header +
//! addressing + trigger (`parse_rx_text`, fsq.cxx:436-623).
//!
//! Symbols and IFK tone indices are asserted **bit-exact** vs the fldigi golden
//! vector (`tests/vectors/fsq_varicode.json`); audio is loopback/AWGN-gated only
//! (Doctrine §3). FSQ's asynchronous station services (sounder, heard-list aging,
//! relayed/delayed store-and-forward transmit) and the picture sub-protocol
//! (`fsq-pic.cxx`, Phase 15) are out of scope — see the phase plan's "Deferred".

pub mod directed;

use crate::framing::fsq_varicode::{encode_str, Framer, OFFSET};
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::ifk33::{syms_to_tones, IfkDemod, IfkGeom, NUMTONES};
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

const FSQ_SR: u32 = 12000; // ref: fsq.h:40
const FSQ_SYMLEN: usize = 4096; // ref: fsq.h:42 (fixed tone-spacing reference)
const FSQ_SPACING: u32 = 3; // ref: fsq.cxx:181

/// FSQBOL / FSQEOT / FSQEOL framing markers. ref: fsq.cxx:69-71.
pub const FSQBOL: &str = " \n";
pub const FSQEOT: &str = "  \x08  ";
pub const FSQEOL: &str = "\n ";

/// The FSQ speed selector. Fixes only the emitted samples/symbol; tone spacing is
/// constant. ref: fsq.cxx:312-327.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsqSpeed {
    S1_5,
    S2,
    S3,
    S4_5,
    S6,
}

impl FsqSpeed {
    /// Emitted samples per symbol. ref: fsq.cxx:316-326.
    pub fn symlen(self) -> usize {
        match self {
            FsqSpeed::S1_5 => 8192,
            FsqSpeed::S2 => 6144,
            FsqSpeed::S3 => 4096,
            FsqSpeed::S4_5 => 3072,
            FsqSpeed::S6 => 2048,
        }
    }

    pub fn baud(self) -> f32 {
        FSQ_SR as f32 / self.symlen() as f32
    }

    pub fn from_label(s: &str) -> Option<FsqSpeed> {
        Some(match s {
            "fsq-1.5" => FsqSpeed::S1_5,
            "fsq-2" => FsqSpeed::S2,
            "fsq" | "fsq-3" => FsqSpeed::S3,
            "fsq-4.5" => FsqSpeed::S4_5,
            "fsq-6" => FsqSpeed::S6,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            FsqSpeed::S1_5 => "fsq-1.5",
            FsqSpeed::S2 => "fsq-2",
            FsqSpeed::S3 => "fsq-3",
            FsqSpeed::S4_5 => "fsq-4.5",
            FsqSpeed::S6 => "fsq-6",
        }
    }

    pub fn all() -> &'static [FsqSpeed] {
        &[FsqSpeed::S1_5, FsqSpeed::S2, FsqSpeed::S3, FsqSpeed::S4_5, FsqSpeed::S6]
    }
}

/// Fixed tone spacing: `FSQ_SPACING · SR / FSQ_SYMLEN`. ref: fsq.cxx:1334,252.
pub fn tone_spacing() -> f32 {
    FSQ_SPACING as f32 * FSQ_SR as f32 / FSQ_SYMLEN as f32
}

/// The IFK tone-index sequence for a raw on-air string (no BOT/EOT framing).
/// Asserted bit-exact vs the golden vector. ref: fsq.cxx:1367-1382.
pub fn raw_tones(onair: &str) -> Vec<u32> {
    syms_to_tones(&encode_str(onair), OFFSET)
}

/// Assemble the on-air transmission string exactly as `tx_process` does.
/// `directed` appends the CRC8(mycall) header and uses the `FSQEOT` trailer; the
/// non-directed form omits the CRC and uses `FSQEOL`. ref: fsq.cxx:1486-1521.
pub fn build_tx(mycall: &str, body: &str, directed: bool) -> String {
    let mut s = String::new();
    s.push(' ');
    s.push_str(FSQBOL);
    s.push_str(mycall);
    s.push(':');
    if directed {
        s.push_str(&crate::framing::fsq_varicode::crc8_hex(mycall));
    }
    s.push_str(body);
    s.push_str(if directed { FSQEOT } else { FSQEOL });
    s
}

fn geom(speed: FsqSpeed, center_hz: f32) -> IfkGeom {
    IfkGeom { rate: FSQ_SR as f32, symlen: speed.symlen(), center_hz, spacing_hz: tone_spacing() }
}

fn caps() -> ModeCaps {
    ModeCaps {
        native_rate: FSQ_SR,
        bandwidth_hz: NUMTONES as f32 * tone_spacing(),
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

/// The FSQ modulator. `mycall`/`directed` frame each transmission; an empty
/// `mycall` sends the body verbatim (raw keyboard mode) for loopback/testing.
pub struct FsqMod {
    speed: FsqSpeed,
    center_hz: f32,
    mycall: String,
    directed: bool,
}

impl FsqMod {
    pub fn new(speed: FsqSpeed, center_hz: f32, mycall: impl Into<String>, directed: bool) -> Self {
        FsqMod { speed, center_hz, mycall: mycall.into(), directed }
    }

    fn onair(&self, body: &str) -> String {
        if self.mycall.is_empty() {
            body.to_string()
        } else {
            build_tx(&self.mycall, body, self.directed)
        }
    }
}

impl Modulator for FsqMod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("fsq needs text")),
        };
        let onair = self.onair(text);
        let tones = raw_tones(&onair);
        let g = geom(self.speed, self.center_hz);
        let mfsk = MFsk::new(g.rate, g.symlen, g.base_hz(), g.spacing_hz, NUMTONES);
        Ok(mfsk.modulate(&tones))
    }
}

/// The FSQ demodulator. Emits the monitor character stream as per-character
/// `Text` frames (fldigi's monitor view), and accumulates whole transmissions so
/// the `directed` submodule can parse them. Call [`FsqDemod::take_directed`] to
/// drain parsed directed messages and [`FsqDemod::heard`] for the heard list.
pub struct FsqDemod {
    demod: IfkDemod,
    framer: Framer,
    /// Accumulated monitor text of the in-progress transmission (between BOT and
    /// EOT). ref: fsq.cxx `rx_text`.
    rx_text: String,
    /// Rolling 3-char window for BOT/EOT/EOL detection. ref: fsq.cxx:992-1015.
    marker: Vec<u8>,
    mycall: String,
    heard: directed::HeardList,
    parsed: Vec<directed::DirectedMessage>,
}

impl FsqDemod {
    pub fn new(speed: FsqSpeed, center_hz: f32, mycall: impl Into<String>) -> Self {
        FsqDemod {
            demod: IfkDemod::new(geom(speed, center_hz), OFFSET),
            framer: Framer::new(),
            rx_text: String::new(),
            marker: Vec::with_capacity(3),
            mycall: mycall.into(),
            heard: directed::HeardList::new(),
            parsed: Vec::new(),
        }
    }

    /// Drain and return the directed messages parsed since the last call.
    pub fn take_directed(&mut self) -> Vec<directed::DirectedMessage> {
        std::mem::take(&mut self.parsed)
    }

    /// The stations heard so far.
    pub fn heard(&self) -> &directed::HeardList {
        &self.heard
    }

    fn on_char(&mut self, ch: u8, out: &mut Vec<Frame>) {
        // Monitor stream: emit every decoded character.
        out.push(Frame {
            payload: FramePayload::Text((ch as char).to_string()),
            meta: FrameMeta { crc_ok: true, decoder: Some("fsq".into()), ..Default::default() },
        });

        // BOT/EOT/EOL detection over a rolling window. ref: fsq.cxx:992-1015.
        self.marker.push(ch);
        if self.marker.len() > 3 {
            self.marker.remove(0);
        }
        let m = &self.marker;
        let eot = m.len() == 3
            && m[0] == FSQEOT.as_bytes()[0]
            && m[1] == FSQEOT.as_bytes()[1]
            && m[2] == FSQEOT.as_bytes()[2]; // SP SP BS
        let bol = m.len() >= 2
            && m[m.len() - 2] == FSQBOL.as_bytes()[0]
            && m[m.len() - 1] == FSQBOL.as_bytes()[1]; // SP LF

        if bol {
            self.rx_text.clear();
        }
        self.rx_text.push(ch as char);
        if eot {
            if let Some(msg) = directed::parse_rx_text(&self.rx_text, &self.mycall, &mut self.heard)
            {
                self.parsed.push(msg);
            }
            self.rx_text.clear();
        }
    }

    fn drain(&mut self, out: &mut Vec<Frame>) {
        let syms = self.demod.drain();
        for s in syms {
            if let Some(ch) = self.framer.push(s) {
                self.on_char(ch, out);
            }
        }
    }
}

impl Demodulator for FsqDemod {
    fn caps(&self) -> ModeCaps {
        caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.demod.push_samples(samples);
        let mut out = Vec::new();
        self.drain(&mut out);
        out
    }

    fn flush(&mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        self.drain(&mut out);
        if let Some(ch) = self.framer.push(28) {
            self.on_char(ch, &mut out);
        }
        out
    }

    fn reset(&mut self) {
        self.demod.reset();
        self.framer.reset();
        self.rx_text.clear();
        self.marker.clear();
        self.parsed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_match_reference_text_frame() {
        // ref: fsq_varicode.json "text" frame tones.
        let want: Vec<u32> = vec![
            21, 30, 3, 4, 22, 11, 21, 25, 4, 5, 8, 27, 10, 1, 16, 17, 24, 7, 32, 0, 5, 11, 12, 3, 5,
            3, 12, 24, 2,
        ];
        assert_eq!(raw_tones("the quick brown fox de w1hkj"), want);
    }

    #[test]
    fn build_tx_matches_reference_directed_frame() {
        // ref: fsq_varicode.json "directed" frame onair string.
        assert_eq!(build_tx("w1hkj", "k1abc test", true), "  \nw1hkj:efk1abc test  \x08  ");
        // and its IFK tones.
        let want: Vec<u32> = vec![
            1, 2, 31, 22, 24, 22, 31, 10, 21, 13, 11, 17, 24, 3, 5, 3, 5, 8, 12, 13, 1, 7, 27, 15,
            16, 17, 12, 11, 12, 13,
        ];
        assert_eq!(raw_tones(&build_tx("w1hkj", "k1abc test", true)), want);
    }

    #[test]
    fn params_derive_spacing_and_baud() {
        assert!((tone_spacing() - 8.7890625).abs() < 1e-4);
        assert!((FsqSpeed::S3.baud() - 2.9296875).abs() < 1e-4);
        assert!((FsqSpeed::S6.baud() - 5.859375).abs() < 1e-4);
    }

    #[test]
    fn labels_round_trip() {
        for &s in FsqSpeed::all() {
            assert_eq!(FsqSpeed::from_label(s.label()), Some(s));
        }
        assert_eq!(FsqSpeed::from_label("fsq"), Some(FsqSpeed::S3));
        assert_eq!(FsqSpeed::from_label("nope"), None);
    }

    // ---- loopback gates -----------------------------------------------------

    fn monitor(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn loopback_recovers_raw_text_all_speeds() {
        // Raw keyboard mode (no directed header): the monitor stream recovers the
        // body (plus FSQ's faithful leading-space seed artifact).
        let msg = "cq cq de w1hkj k";
        for &s in FsqSpeed::all() {
            let mut tx = FsqMod::new(s, 1500.0, "", false);
            let samples = tx.modulate(&Frame::text(msg)).unwrap();
            let mut rx = FsqDemod::new(s, 1500.0, "");
            let mut frames = rx.feed(&samples);
            frames.extend(rx.flush());
            let got = monitor(&frames);
            assert!(got.contains(msg), "speed {} got {:?}", s.label(), got);
        }
    }

    #[test]
    fn loopback_directed_frame_parses_and_is_heard() {
        // w1hkj sends a directed message to our station (k1abc); the demod parses
        // the header (CRC-verified), registers w1hkj as heard, and surfaces the
        // directed message addressed to us.
        let mut tx = FsqMod::new(FsqSpeed::S3, 1500.0, "w1hkj", true);
        let samples = tx.modulate(&Frame::text("k1abc test message")).unwrap();
        let mut rx = FsqDemod::new(FsqSpeed::S3, 1500.0, "k1abc");
        let mut frames = rx.feed(&samples);
        frames.extend(rx.flush());
        let msgs = rx.take_directed();
        assert_eq!(msgs.len(), 1, "monitor: {:?}", monitor(&frames));
        let m = &msgs[0];
        assert_eq!(m.from, "w1hkj");
        assert!(m.crc_ok);
        assert!(rx.heard().contains("w1hkj"));
    }
}
