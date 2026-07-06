//! JT4 mode assembly: 4-FSK, K=32 r=1/2 Fano-decoded convolutional code, 72-bit
//! legacy message, 60 s on the minute. WSJT-X's legacy EME/weak-signal mode,
//! submodes A–G differing only in tone spacing (`nch` multiplier of the keying
//! rate). Building blocks: `frontend::modulate::MFsk`, `fec::fano::FanoCode`
//! (the shared K=32 Layland–Lushbaugh code — JT4's `encode232`), the JT4
//! interleave + `npr` sync vector, `framing::message77::legacy::{pack72,
//! unpack72}`, and the shared `modes::fsk_util` tone detector.
//!
//! Reference: WSJT-X `wsjtx/lib/{jt4.f90,gen4.f90,encode4.f90,encode232.f90,
//! interleave4.f90,jt4_decode.f90}` @ ccdfaf3c1. The bit-domain TX path
//! (Fano-encode → JT4 interleave → `npr` sync into 4-FSK symbols) is bit-exact
//! to `gen4` and KAT-gated against `vectors/jt4_tones.json`; the exact packjt
//! 72-bit message layout is the `#[ignore]` cross-decode gate (as for JT9),
//! since `pack72` here is a self-consistent NBASE-relative port, not WSJT-X's.
//!
//! Channel symbols: `tone(i) = 2*data(i) + npr(i+1)` — the interleaved code bit
//! is the MSB, the fixed pseudo-random `npr` sync bit is the LSB, so the four
//! tones carry (data,sync) = (0,0),(0,1),(1,0),(1,1).

use crate::fec::fano::FanoCode;
use crate::framing::message77::legacy::{pack72, unpack72};
use crate::frontend::modulate::MFsk;
use crate::mode::{BlockDemodulator, DemodShape, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::tone_powers;
use crate::types::{Frame, FrameMeta, FramePayload, Llr, Sample};

pub const JT4_RATE: u32 = 12_000;
pub const JT4_SPS: usize = 2742; // 12000/4.375, truncated as in jt4sim.f90
pub const JT4_BAUD: f32 = JT4_RATE as f32 / JT4_SPS as f32; // ≈ 4.376 baud
pub const JT4_TONES: u32 = 4;
pub const JT4_BASE_HZ: f32 = 1000.0; // reference single-signal f0 (jt4sim.f90)
pub const JT4_WINDOW_S: f32 = 60.0;
const MSG_BITS: usize = 72;
/// Channel symbols: (72 data + 31 Fano tail) × 2 code bits = 206.
pub const JT4_SYMBOLS: usize = (MSG_BITS + 31) * 2;

/// JT4 pseudo-random sync vector `npr`, transcribed verbatim.
/// ref: wsjtx/lib/jt4.f90:18-25 (207 values; the on-air sync is `npr(2:)`).
#[rustfmt::skip]
const NPR: [u8; 207] = [
    0,0,0,0,1,1,0,0,0,1,1,0,1,1,0,0,1,0,1,0,0,0,0,0,0,0,1,1,0,0,
    0,0,0,0,0,0,0,0,0,0,1,0,1,1,0,1,1,0,1,0,1,1,1,1,1,0,1,0,0,0,
    1,0,0,1,0,0,1,1,1,1,1,0,0,0,1,0,1,0,0,0,1,1,1,1,0,1,1,0,0,1,
    0,0,0,1,1,0,1,0,1,0,1,0,1,0,1,1,1,1,1,0,1,0,1,0,1,1,0,1,0,1,
    0,1,1,1,0,0,1,0,1,1,0,1,1,1,1,0,0,0,0,1,1,0,1,1,0,0,0,1,1,1,
    0,1,1,1,0,1,1,1,0,0,1,0,0,0,1,1,0,1,1,0,0,1,0,0,0,1,1,1,1,1,
    1,0,0,1,1,0,0,0,0,1,1,0,0,0,1,0,1,1,0,1,1,1,1,0,1,0,1,
];

/// On-air sync bits: `npr(2:)` (the leading element is dropped), one LSB per
/// channel symbol. ref: wsjtx/lib/gen4.f90:38 (`itone=2*itone + npr(2:)`).
fn sync_bits() -> &'static [u8] {
    &NPR[1..]
}

/// JT4 interleave permutation `j0`: the 8-bit bit-reversal of `0..255`, keeping
/// only values ≤205, taken in ascending source order. Forward interleave places
/// code bit `i` at position `j0[i]`. ref: wsjtx/lib/interleave4.f90.
fn interleave_table() -> [usize; JT4_SYMBOLS] {
    let mut j0 = [0usize; JT4_SYMBOLS];
    let mut k = 0usize;
    for i in 0u32..256 {
        let n = i.reverse_bits() >> 24; // reverse low 8 bits
        if (n as usize) < JT4_SYMBOLS {
            j0[k] = n as usize;
            k += 1;
        }
    }
    j0
}

/// JT4 submodes A–G. They share the entire bit-domain pipeline and differ only
/// in tone spacing = keying rate × `nch`. ref: wsjtx/lib/jt4.f90:17 (`nch`),
/// jt4sim.f90 (`freq = f0 + itone*baud*nch(mode4)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jt4Submode {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
}

impl Jt4Submode {
    pub const ALL: [Jt4Submode; 7] = [
        Jt4Submode::A,
        Jt4Submode::B,
        Jt4Submode::C,
        Jt4Submode::D,
        Jt4Submode::E,
        Jt4Submode::F,
        Jt4Submode::G,
    ];

    /// Tone-spacing multiplier `nch`. ref: wsjtx/lib/jt4.f90:17.
    pub fn nch(self) -> u32 {
        match self {
            Jt4Submode::A => 1,
            Jt4Submode::B => 2,
            Jt4Submode::C => 4,
            Jt4Submode::D => 9,
            Jt4Submode::E => 18,
            Jt4Submode::F => 36,
            Jt4Submode::G => 72,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Jt4Submode::A => "jt4a",
            Jt4Submode::B => "jt4b",
            Jt4Submode::C => "jt4c",
            Jt4Submode::D => "jt4d",
            Jt4Submode::E => "jt4e",
            Jt4Submode::F => "jt4f",
            Jt4Submode::G => "jt4g",
        }
    }

    pub fn from_label(label: &str) -> Option<Jt4Submode> {
        Jt4Submode::ALL.into_iter().find(|v| v.label() == label)
    }

    /// Audio tone spacing (Hz): keying rate × `nch`.
    fn spacing_hz(self) -> f32 {
        JT4_BAUD * self.nch() as f32
    }
}

/// Fano-encode + JT4-interleave a 72-bit payload into the 206 interleaved code
/// bits (pre-sync). Shared by TX and the KAT.
fn encode_bits(fano: &FanoCode, bits: &[u8; MSG_BITS]) -> [u8; JT4_SYMBOLS] {
    let coded = fano.encode(bits); // (72+31)*2 = 206 code bits
    let j0 = interleave_table();
    let mut out = [0u8; JT4_SYMBOLS];
    for (i, &c) in coded.iter().enumerate() {
        out[j0[i]] = c;
    }
    out
}

fn caps(sub: Jt4Submode) -> ModeCaps {
    ModeCaps {
        native_rate: JT4_RATE,
        bandwidth_hz: JT4_TONES as f32 * sub.spacing_hz(),
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Windowed { window_s: JT4_WINDOW_S, period_s: JT4_WINDOW_S },
    }
}

pub struct Jt4Mod {
    submode: Jt4Submode,
    fano: FanoCode,
}

impl Jt4Mod {
    pub fn new(submode: Jt4Submode) -> Self {
        Self { submode, fano: FanoCode::default() }
    }
}

impl Modulator for Jt4Mod {
    fn caps(&self) -> ModeCaps {
        caps(self.submode)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t.clone(),
            _ => return Err(ModError::UnsupportedPayload("jt4 needs text")),
        };
        let bits = pack72(&text).ok_or_else(|| ModError::TooLong(text.clone()))?;
        let coded = encode_bits(&self.fano, &bits);
        let sync = sync_bits();
        // tone(i) = 2*data(i) + sync(i): data is the MSB, sync the LSB.
        let symbols: Vec<u32> =
            coded.iter().zip(sync).map(|(&d, &s)| 2 * d as u32 + s as u32).collect();
        let mfsk = MFsk::new(
            JT4_RATE as f32,
            JT4_SPS,
            JT4_BASE_HZ,
            self.submode.spacing_hz(),
            JT4_TONES,
        );
        Ok(mfsk.modulate(&symbols))
    }
}

pub struct Jt4Demod {
    submode: Jt4Submode,
    fano: FanoCode,
}

impl Jt4Demod {
    pub fn new(submode: Jt4Submode) -> Self {
        Self { submode, fano: FanoCode::default() }
    }
}

impl BlockDemodulator for Jt4Demod {
    fn caps(&self) -> ModeCaps {
        caps(self.submode)
    }

    fn decode_window(&mut self, window: &[Sample], _start_ns: u64) -> Vec<Frame> {
        if window.len() < JT4_SYMBOLS * JT4_SPS {
            return Vec::new();
        }
        let spacing = self.submode.spacing_hz();
        let sync = sync_bits();
        // Per-symbol soft data LLR: the sync bit is known, so compare the power
        // of the data=0 tone (2*0+sync) against the data=1 tone (2*1+sync).
        let mut inter = Vec::with_capacity(JT4_SYMBOLS);
        for (i, &s) in sync.iter().enumerate() {
            let start = i * JT4_SPS;
            let block = &window[start..start + JT4_SPS];
            let p = tone_powers(block, JT4_RATE as f32, JT4_BASE_HZ, spacing, JT4_TONES);
            let p0 = p[s as usize]; // data bit 0
            let p1 = p[2 + s as usize]; // data bit 1
            let llr: Llr = 6.0 * (p0 - p1) / (p0 + p1 + 1e-9);
            inter.push(llr);
        }
        // De-interleave to code-bit order, then Fano-decode.
        let j0 = interleave_table();
        let code_llr: Vec<Llr> = (0..JT4_SYMBOLS).map(|i| inter[j0[i]]).collect();
        let Some(bits) = self.fano.fano_decode(&code_llr, MSG_BITS, 0.5) else {
            return Vec::new();
        };
        let bits72: [u8; 72] = match bits.try_into() {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        match unpack72(&bits72) {
            Some(text) => vec![Frame {
                payload: FramePayload::Text(text),
                meta: FrameMeta { crc_ok: true, decoder: Some("jt4".into()), ..Default::default() },
            }],
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-exact KAT: the TX bit-domain path (Fano-encode → JT4 interleave →
    /// `npr` sync into 4-FSK symbols) reproduces the reference `gen4` channel
    /// symbols for the fixed 72-bit payload in `vectors/jt4_tones.json`.
    #[test]
    fn tx_symbols_match_reference_vector() {
        // ref: vectors/jt4_tones.json (build_jt4_tones.sh, wsjtx @ ccdfaf3c1).
        let msgbits =
            "111111000000101010010101111010000101010001100100111100001001110000011011";
        let want =
            "20013200332110012100020223320000200200032130112101313301202102302313132201232003313211203020132121030301313321210330121013100103301331200031013202131213103332212021301302320013313300330220110201231013310121";
        let mut bits = [0u8; MSG_BITS];
        for (b, ch) in bits.iter_mut().zip(msgbits.chars()) {
            *b = (ch == '1') as u8;
        }
        let coded = encode_bits(&FanoCode::default(), &bits);
        let sync = sync_bits();
        let tones: String = coded
            .iter()
            .zip(sync)
            .map(|(&d, &s)| char::from(b'0' + (2 * d + s)))
            .collect();
        assert_eq!(tones, want, "JT4 channel symbols diverge from gen4 reference");
    }

    #[test]
    fn interleave_is_a_permutation() {
        let j0 = interleave_table();
        let mut seen = [false; JT4_SYMBOLS];
        for &v in &j0 {
            assert!(v < JT4_SYMBOLS);
            assert!(!seen[v], "interleave value {v} repeats");
            seen[v] = true;
        }
        assert!(seen.iter().all(|&b| b), "interleave is not onto");
    }

    #[test]
    fn submode_tone_spacing_matches_reference_nch() {
        // ref: wsjtx/lib/jt4.f90:17  nch/1,2,4,9,18,36,72/
        let nch = [1, 2, 4, 9, 18, 36, 72];
        for (v, expect) in Jt4Submode::ALL.into_iter().zip(nch) {
            assert_eq!(v.nch(), expect);
            assert!((v.spacing_hz() - JT4_BAUD * expect as f32).abs() < 1e-3);
        }
    }

    #[test]
    fn caps_are_windowed_60s() {
        assert!(matches!(
            Jt4Mod::new(Jt4Submode::A).caps().shape,
            DemodShape::Windowed { window_s, .. } if (window_s - 60.0).abs() < 0.1
        ));
    }

    #[test]
    fn loopback_decodes_message_all_submodes() {
        for sub in Jt4Submode::ALL {
            for msg in ["K1ABC W9XYZ EN37", "CQ K1ABC FN42"] {
                let mut tx = Jt4Mod::new(sub);
                let samples = tx.modulate(&Frame::text(msg)).unwrap();
                assert!(
                    samples.iter().any(|&s| s.abs() > 0.1),
                    "{}: silent modulation for {msg}",
                    sub.label()
                );
                let n = (JT4_RATE as f32 * JT4_WINDOW_S) as usize;
                let mut window = samples.clone();
                window.resize(n, 0.0);
                let mut rx = Jt4Demod::new(sub);
                let decodes = rx.decode_window(&window, 0);
                assert!(
                    decodes
                        .iter()
                        .any(|f| matches!(&f.payload, FramePayload::Text(t) if t == msg)),
                    "no JT4 decode of {msg:?} in {}: {decodes:?}",
                    sub.label()
                );
            }
        }
    }
}
