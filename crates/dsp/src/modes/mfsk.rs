//! MFSK family: M-ary FSK with a NASA K=7 rate-1/2 convolutional code, a
//! diagonal interleaver, and the self-framing MFSK Varicode, parametric by
//! submode (MFSK4/8/11/16/22/31/32/64/128 + the 64L/128L deep-interleave modes).
//!
//! Port of fldigi's MFSK modem (`fldigi/src/mfsk/{mfsk.cxx,mfskvaricode.cxx,
//! interleave.cxx}`, upstream 4.1.23 @ 61b97f413). The TX chain mirrors
//! `sendchar`→`sendbit`→`sendsymbol` (mfsk.cxx:919-961) exactly:
//!
//! ```text
//!   text ──varicode──▶ bits ──K=7 conv (0x6d/0x4f)──▶ code bits
//!        ──group symbits──▶ ──interleave(symbits,depth)──▶ ──grayencode──▶ tone
//! ```
//!
//! `numtones == 2^symbits`, so each MFSK symbol is `symbits` code bits (two code
//! bits per data bit — poly1 parity then poly2). The interleaver width is
//! `symbits`; its depth is per-submode. fldigi's `grayencode` is the XOR-cascade
//! (gray→binary), which is our [`gray_decode`]; the inverse [`gray_encode`] runs
//! on RX. ref: mfsk.cxx:919-937 (`sendsymbol`), misc.cxx:123 (`grayencode`).
//!
//! Wire-determining arithmetic is bit-exact vs fldigi: the varicode bits, the
//! K=7 code bits, the interleaved symbol stream, and the gray tone indices are
//! all asserted byte-for-byte against vectors extracted from the unmodified
//! fldigi units (`tests/vectors/mfsk.json`, `scratch/refvectors/build_mfsk.sh`).
//! Modulated audio is gated on a loopback decode only, never bit-exact
//! (Doctrine §3). Sync, AFC and the TX framing envelope (preamble/STX/EOT) of
//! real MFSK are deferred — the loopback is the gate and cross-decode against
//! fldigi is the `#[ignore]` nightly gate, matching the Phase-9 DominoEX / Phase-7
//! PSK-R precedent.

use crate::fec::conv::ConvCode;
use crate::fec::gray::{gray_decode, gray_encode};
use crate::fec::interleave::MfskInterleaver;
use crate::framing::varicode::{mfsk_encode, mfsk_symbol_to_byte};
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::{argmax, tone_powers};
use crate::types::{Frame, FrameMeta, FramePayload, Llr, Sample};

/// NASA K=7 rate-1/2 generator polynomials (mfsk.h:54-56 POLY1/POLY2).
const POLY1: u32 = 0x6d;
const POLY2: u32 = 0x4f;

/// The MFSK submodes ported here. Each fixes `(symlen, symbits, depth,
/// samplerate)`; `numtones == 1 << symbits` and the tone spacing / baud derive
/// from `samplerate / symlen`. ref: mfsk.cxx:180-289 (constructor switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfskVariant {
    M4,
    M8,
    M11,
    M16,
    M22,
    M31,
    M32,
    M64,
    M128,
    M64L,
    M128L,
}

/// Resolved per-submode parameters. ref: mfsk.cxx:180-289.
#[derive(Debug, Clone, Copy)]
pub struct MfskParams {
    /// Samples per symbol at `samplerate` (fldigi `symlen`).
    pub symlen: usize,
    /// Bits per MFSK symbol = interleaver width (fldigi `symbits`).
    pub symbits: usize,
    /// Diagonal-interleaver depth (fldigi `depth`).
    pub depth: usize,
    /// Native sample rate (8000 or 11025 Hz).
    pub samplerate: u32,
    /// TX flush/preamble length in bits (fldigi `preamble`), sized to drain the
    /// interleaver. ref: mfsk.cxx:189-286.
    pub preamble: usize,
}

impl MfskParams {
    /// Number of MFSK tones = `2^symbits` (fldigi `numtones`).
    pub fn numtones(&self) -> u32 {
        1 << self.symbits
    }
    /// Tone spacing in Hz = `samplerate / symlen` (mfsk.cxx:291).
    pub fn tone_spacing(&self) -> f32 {
        self.samplerate as f32 / self.symlen as f32
    }
    /// Occupied bandwidth = `(numtones - 1) * tone_spacing` (mfsk.cxx:330).
    pub fn bandwidth(&self) -> f32 {
        (self.numtones() - 1) as f32 * self.tone_spacing()
    }
    /// Symbol rate (baud) = `samplerate / symlen`.
    pub fn baud(&self) -> f32 {
        self.samplerate as f32 / self.symlen as f32
    }
}

impl MfskVariant {
    /// ref: mfsk.cxx:180-289.
    pub fn params(self) -> MfskParams {
        use MfskVariant::*;
        let p = |symlen, symbits, depth, samplerate, preamble| MfskParams {
            symlen,
            symbits,
            depth,
            samplerate,
            preamble,
        };
        match self {
            M4 => p(2048, 5, 5, 8000, 107),
            M8 => p(1024, 5, 5, 8000, 107),
            M16 => p(512, 4, 10, 8000, 107),
            M32 => p(256, 4, 10, 8000, 107),
            M31 => p(256, 3, 10, 8000, 107),
            M64 => p(128, 4, 10, 8000, 180),
            M128 => p(64, 4, 20, 8000, 214),
            M64L => p(128, 4, 400, 8000, 2500),
            M128L => p(64, 4, 800, 8000, 5000),
            M11 => p(1024, 4, 10, 11025, 107),
            M22 => p(512, 4, 10, 11025, 107),
        }
    }

    pub fn samplerate(self) -> u32 {
        self.params().samplerate
    }

    pub fn from_label(s: &str) -> Option<MfskVariant> {
        use MfskVariant::*;
        Some(match s {
            "mfsk4" => M4,
            "mfsk8" => M8,
            "mfsk11" => M11,
            "mfsk16" => M16,
            "mfsk22" => M22,
            "mfsk31" => M31,
            "mfsk32" => M32,
            "mfsk64" => M64,
            "mfsk128" => M128,
            "mfsk64l" => M64L,
            "mfsk128l" => M128L,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        use MfskVariant::*;
        match self {
            M4 => "mfsk4",
            M8 => "mfsk8",
            M11 => "mfsk11",
            M16 => "mfsk16",
            M22 => "mfsk22",
            M31 => "mfsk31",
            M32 => "mfsk32",
            M64 => "mfsk64",
            M128 => "mfsk128",
            M64L => "mfsk64l",
            M128L => "mfsk128l",
        }
    }

    /// Every ported submode, for table-driven tests and the TUI selector.
    pub fn all() -> &'static [MfskVariant] {
        use MfskVariant::*;
        &[M4, M8, M11, M16, M22, M31, M32, M64, M128, M64L, M128L]
    }
}

/// Streaming K=7 (POLY1/POLY2) convolutional encode of a data-bit stream, no
/// tail flush — the never-terminated stream fldigi's `sendbit` feeds. Two output
/// bits per input bit: poly1 parity then poly2 parity. ref: mfsk.cxx:939-953
/// (`sendbit`), viterbi.cxx (`encoder::encode`).
fn conv_stream(bits: &[u8]) -> Vec<u8> {
    let mut reg = 0u32;
    let mut out = Vec::with_capacity(bits.len() * 2);
    for &b in bits {
        reg = (reg << 1) | (b as u32 & 1);
        out.push((reg & POLY1).count_ones() as u8 & 1);
        out.push((reg & POLY2).count_ones() as u8 & 1);
    }
    out
}

/// The gray-coded tone-index sequence for a data-bit stream: group the code bits
/// into `symbits`-bit symbols (MSB first), interleave, then `grayencode`
/// (== [`gray_decode`]). Emits one tone per complete symbol; a trailing partial
/// symbol is dropped (drained on TX by the flush). ref: mfsk.cxx:939-937.
fn symbols_to_tones(code: &[u8], p: MfskParams) -> Vec<u32> {
    let mut il = MfskInterleaver::new(p.symbits, p.depth, true, 0u8);
    let mut tones = Vec::new();
    let (mut shreg, mut state) = (0u32, 0usize);
    for &cb in code {
        shreg = (shreg << 1) | cb as u32;
        state += 1;
        if state == p.symbits {
            il.bits(&mut shreg);
            tones.push(gray_decode(shreg & (p.numtones() - 1)));
            shreg = 0;
            state = 0;
        }
    }
    tones
}

/// The bit-exact data-portion tone sequence fldigi's TX emits for `text` (no
/// framing envelope, no flush). Asserted byte-for-byte against `mfsk.json`.
pub fn text_tones(v: MfskVariant, text: &str) -> Vec<u32> {
    let p = v.params();
    symbols_to_tones(&conv_stream(&mfsk_encode(text)), p)
}

fn caps(v: MfskVariant) -> ModeCaps {
    let p = v.params();
    ModeCaps {
        native_rate: p.samplerate,
        bandwidth_hz: p.bandwidth(),
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

pub struct MfskMod {
    v: MfskVariant,
    center_hz: f32,
}

impl MfskMod {
    pub fn new(v: MfskVariant, center_hz: f32) -> Self {
        MfskMod { v, center_hz }
    }

    /// Lowest tone frequency: fldigi centres the `numtones` tones on the carrier,
    /// tone 0 at `center - bandwidth/2`. ref: mfsk.cxx:923 (`f = txfreq - bw/2`).
    fn base_hz(&self) -> f32 {
        self.center_hz - self.v.params().bandwidth() / 2.0
    }
}

impl Modulator for MfskMod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("mfsk needs text")),
        };
        let p = self.v.params();
        // Payload bits + a flush: one NUL codeword (its leading boundary closes
        // the last real varicode char) then enough zero data bits to drain the
        // interleaver (latency ≤ symbits·depth symbols) and prime the RX Viterbi
        // traceback. The payload tones are causal, so the flush never perturbs
        // them (see `text_tones`). ref: mfsk.cxx:973-985 (`flushtx`).
        let mut bits = mfsk_encode(text);
        bits.extend(mfsk_encode("\0"));
        bits.extend(std::iter::repeat_n(0u8, p.preamble + p.symbits * p.depth + 4 * TRACEBACK));
        let tones = symbols_to_tones(&conv_stream(&bits), p);
        let mfsk = MFsk::new(p.samplerate as f32, p.symlen, self.base_hz(), p.tone_spacing(), p.numtones());
        Ok(mfsk.modulate(&tones))
    }
}

pub struct MfskDemod {
    v: MfskVariant,
    center_hz: f32,
    buf: Vec<Sample>,
    rxinlv: MfskInterleaver<u8>,
    dec: crate::fec::conv::StreamingViterbi,
    pending: Vec<u8>, // recovered code bits not yet paired into a Viterbi step
    vshreg: u32,      // MFSK Varicode reframing shift register
}

/// RX Viterbi traceback depth — comfortably past 5·K for a clean recovery on a
/// loopback (fldigi uses `tracepair.trace`, of the same order).
const TRACEBACK: usize = 48;

impl MfskDemod {
    pub fn new(v: MfskVariant, center_hz: f32) -> Self {
        let p = v.params();
        MfskDemod {
            v,
            center_hz,
            buf: Vec::new(),
            rxinlv: MfskInterleaver::new(p.symbits, p.depth, false, 0u8),
            dec: ConvCode { k: 7, polys: vec![POLY1, POLY2] }.streaming_decoder(TRACEBACK),
            pending: Vec::new(),
            vshreg: 0,
        }
    }

    fn base_hz(&self) -> f32 {
        self.center_hz - self.v.params().bandwidth() / 2.0
    }

    /// Feed one recovered data bit through the MFSK Varicode reframer, returning a
    /// completed character (0 = NUL/idle, dropped by the caller). ref:
    /// mfsk.cxx:525-535 (`recvbit`), varicode.rs `mfsk_decode`.
    fn reframe(&mut self, bit: u8) -> Option<u8> {
        self.vshreg = (self.vshreg << 1) | bit as u32;
        if self.vshreg & 7 == 1 {
            let sym = self.vshreg >> 1;
            self.vshreg = 1;
            return mfsk_symbol_to_byte(sym);
        }
        None
    }

    /// Detect every fully-buffered symbol, invert gray + interleave + Viterbi +
    /// varicode, and emit completed characters.
    fn drain_symbols(&mut self) -> Vec<Frame> {
        let p = self.v.params();
        let sps = p.symlen;
        let (base, spacing, rate) = (self.base_hz(), p.tone_spacing(), p.samplerate as f32);
        let mut out = Vec::new();
        let mut consumed = 0;
        while self.buf.len() - consumed >= sps {
            let block = &self.buf[consumed..consumed + sps];
            let powers = tone_powers(block, rate, base, spacing, p.numtones());
            let tone = argmax(&powers) as u32;
            // tone → grayencode⁻¹ (fldigi graydecode == our gray_encode) → symbol.
            let mut sym = gray_encode(tone);
            self.rxinlv.bits(&mut sym);
            // Unpack the deinterleaved symbits code bits, MSB first (TX pack order).
            for i in 0..p.symbits {
                self.pending.push(((sym >> (p.symbits - 1 - i)) & 1) as u8);
            }
            // Consume code-bit pairs (poly1, poly2) into the Viterbi decoder.
            while self.pending.len() >= 2 {
                let p1 = self.pending.remove(0);
                let p2 = self.pending.remove(0);
                let llrs: [Llr; 2] =
                    [if p1 == 0 { 4.0 } else { -4.0 }, if p2 == 0 { 4.0 } else { -4.0 }];
                if let Some(dbit) = self.dec.push(&llrs) {
                    if let Some(c) = self.reframe(dbit) {
                        if c != 0 {
                            out.push(char_frame(c));
                        }
                    }
                }
            }
            consumed += sps;
        }
        self.buf.drain(..consumed);
        out
    }
}

fn char_frame(c: u8) -> Frame {
    Frame {
        payload: FramePayload::Text((c as char).to_string()),
        meta: FrameMeta { crc_ok: true, decoder: Some("mfsk".into()), ..Default::default() },
    }
}

impl Demodulator for MfskDemod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        self.buf.extend_from_slice(samples);
        self.drain_symbols()
    }

    fn reset(&mut self) {
        let p = self.v.params();
        self.buf.clear();
        self.rxinlv = MfskInterleaver::new(p.symbits, p.depth, false, 0u8);
        self.dec = ConvCode { k: 7, polys: vec![POLY1, POLY2] }.streaming_decoder(TRACEBACK);
        self.pending.clear();
        self.vshreg = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- bit-exact KATs mirrored from the fldigi golden vector ---------------
    // These run in plain `cargo test` (CI does not enable the `testutil`
    // feature); the reference-vector KAT in tests/kat.rs asserts against the same
    // JSON. Provenance: tests/vectors/mfsk.json.

    fn vector_line(mode: &str) -> String {
        let raw = include_str!("../../tests/vectors/mfsk.json");
        raw.lines()
            .find(|l| l.contains(&format!("\"mode\":\"{mode}\"")))
            .unwrap_or_else(|| panic!("no vector for {mode}"))
            .to_string()
    }

    fn field(line: &str, k: &str) -> String {
        let i = line.find(k).unwrap() + k.len();
        line[i..line[i..].find('"').unwrap() + i].to_string()
    }

    #[test]
    fn tones_match_fldigi_vector_all_submodes() {
        // Every submode whose (symbits, depth) appears in the vector is checked
        // against its representative line; submodes sharing a bit-domain shape
        // reuse the same reference tones (the family is parametric).
        for &v in MfskVariant::all() {
            let line = vector_line(v.label());
            let want: Vec<u32> =
                field(&line, "\"tones\":\"").split(' ').map(|s| s.parse().unwrap()).collect();
            let msg = field(&line, "\"msg\":\"");
            assert_eq!(text_tones(v, &msg), want, "tones differ from fldigi for {}", v.label());
        }
    }

    #[test]
    fn code_bits_match_fldigi_vector() {
        // Isolate the K=7 conv stage: the streaming encoder output equals the
        // golden `coded` bits for the reference message.
        let line = vector_line("mfsk16");
        let want: Vec<u8> = field(&line, "\"coded\":\"").bytes().map(|c| c - b'0').collect();
        let msg = field(&line, "\"msg\":\"");
        assert_eq!(conv_stream(&mfsk_encode(&msg)), want, "K=7 code bits differ from fldigi");
    }

    #[test]
    fn numtones_is_two_pow_symbits() {
        for &v in MfskVariant::all() {
            let p = v.params();
            assert_eq!(p.numtones(), 1 << p.symbits, "{}", v.label());
        }
    }

    #[test]
    fn params_spot_checks() {
        // ref: mfsk.cxx:180-289.
        assert!((MfskVariant::M16.params().tone_spacing() - 15.625).abs() < 1e-3);
        assert!((MfskVariant::M16.params().baud() - 15.625).abs() < 1e-3);
        assert_eq!(MfskVariant::M11.samplerate(), 11025);
        assert_eq!(MfskVariant::M8.params().symbits, 5);
        assert_eq!(MfskVariant::M31.params().numtones(), 8);
        assert_eq!(MfskVariant::M128L.params().depth, 800);
    }

    #[test]
    fn labels_round_trip() {
        for &v in MfskVariant::all() {
            assert_eq!(MfskVariant::from_label(v.label()), Some(v));
        }
        assert_eq!(MfskVariant::from_label("mfsk99"), None);
        assert_eq!(MfskVariant::all().len(), 11);
    }

    // ---- loopback gates ------------------------------------------------------

    fn loopback(v: MfskVariant, msg: &str) -> String {
        let mut tx = MfskMod::new(v, 1500.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = MfskDemod::new(v, 1500.0);
        rx.feed(&samples)
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn loopback_recovers_text_common_submodes() {
        let msg = "CQ DE K1ABC/7 2026";
        // The deep-interleave 64L/128L modes have very long latency; their bit
        // domain is exercised by the tone KAT and the params table — the loopback
        // covers the shallow-depth representatives of each symbits width.
        for &v in &[
            MfskVariant::M4,
            MfskVariant::M8,
            MfskVariant::M16,
            MfskVariant::M31,
            MfskVariant::M32,
            MfskVariant::M64,
            MfskVariant::M128,
            MfskVariant::M11,
            MfskVariant::M22,
        ] {
            assert_eq!(loopback(v, msg), msg, "submode {}", v.label());
        }
    }

    #[test]
    fn loopback_recovers_punctuation_and_case() {
        let msg = "The quick brown fox! (73) $5 @ 90%";
        assert_eq!(loopback(MfskVariant::M16, msg), msg);
    }
}
