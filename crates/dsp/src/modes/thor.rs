//! THOR family: DominoEX's 18-tone IFK+ core with rate-1/2 convolutional FEC, a
//! size-4 diagonal interleaver, soft-decision Viterbi decode and the THOR
//! varicode, parametric by submode.
//!
//! Port of fldigi's THOR modem (`fldigi/src/thor/{thor.cxx,thorvaricode.cxx}`,
//! upstream 4.1.23 @ 61b97f413). The TX chain per character (`thor::sendchar` /
//! `sendsymbol`, reverse=false): the char → THOR varicode bits (primary = IZ8BLY
//! MFSK varicode, `framing::varicode::MFSK`; secondary = `framing::thor_varicode`)
//! → a *streaming* rate-1/2 convolutional encoder (state carried across the whole
//! message, no per-char flush; K=7 `0x6d`/`0x4f`, or K=15 `044735`/`063057` for
//! the high-speed modes) → 4-bit nibbles (MSB-first, poly1 before poly2) → a
//! size-4 diagonal interleaver → an IFK+ tone: `tone = (prevtone + 2 + nibble) %
//! 18`, prevtone starting 0. ref: thor.cxx:1124-1159, 217-350.
//!
//! Wire-determining arithmetic is bit-exact vs fldigi: the varicode bits, the
//! convolutional code pairs, the post-interleave nibbles and the IFK+ tone-index
//! sequence are asserted byte-for-byte against vectors extracted from the
//! unmodified fldigi leaf files (`tests/vectors/thor_varicode.json`,
//! `scratch/refvectors/build_thor.sh`). Modulated audio is gated on a loopback
//! decode only, never bit-exact (Doctrine §3).
//!
//! Like the DominoEX port, the streaming demod assumes symbol alignment to the
//! fed buffer; sync/AFC, RSID, preamble detection and the soft-decode CWI/doppler
//! refinements of real THOR are deferred (the loopback is the gate, fldigi
//! cross-decode the `#[ignore]` nightly gate). The picture sub-protocol
//! (`thor-pic.cxx`) belongs to Phase 15.
//!
//! Because preamble detection (fldigi's `preambledetect`/`softflushrx`) is
//! deferred, the RX has no way to suppress the brief transient the Viterbi +
//! interleaver emit before the varicode framer locks: on a clean channel the
//! decoded stream is a short (≤ a few chars), message-independent startup smear
//! followed by the intact message. fldigi hides this with preamble detection; the
//! loopback tests assert the message arrives intact at the tail with a bounded
//! leading transient. Wiring preamble detection here would eliminate the smear.

use crate::fec::conv::{ConvCode, ConvEncoder, StreamingViterbi};
use crate::fec::interleave::MfskInterleaver;
use crate::framing::thor_varicode;
use crate::frontend::modulate::MFsk;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::modes::fsk_util::{argmax, tone_powers};
use crate::types::{Frame, FrameMeta, FramePayload, Llr, Sample};

/// Number of IFK+ tones (`THORNUMTONES`). ref: thor.h:58.
pub const NUMTONES: u32 = 18;

/// Interleaver square size (`isize`). ref: thor.cxx:213.
const INTERLEAVE_SIZE: usize = 4;

/// Soft-bit magnitude fed to the Viterbi for a hard-detected code bit.
const SOFT_MAG: Llr = 8.0;

/// The THOR submodes ported here. ref: thor.cxx:217-297.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThorVariant {
    Micro,
    T4,
    T5,
    T8,
    T11,
    T16,
    T22,
    T25x4,
    T50x1,
    T50x2,
    T100,
}

/// Resolved per-submode parameters. ref: thor.cxx:217-297.
#[derive(Debug, Clone, Copy)]
pub struct ThorParams {
    /// Samples per symbol at `samplerate` (fldigi `symlen`).
    pub symlen: usize,
    /// IFK+ tone spacing multiplier (fldigi `doublespaced`).
    pub doublespaced: usize,
    /// Native sample rate (8000 or 11025 Hz).
    pub samplerate: u32,
    /// Convolutional constraint length (7 or 15).
    pub k: usize,
    /// Interleaver depth (fldigi `idepth`).
    pub idepth: usize,
}

impl ThorVariant {
    /// ref: thor.cxx:217-297.
    pub fn params(self) -> ThorParams {
        use ThorVariant::*;
        let p = |symlen, doublespaced, samplerate, k, idepth| ThorParams {
            symlen,
            doublespaced,
            samplerate,
            k,
            idepth,
        };
        match self {
            Micro => p(4000, 1, 8000, 7, 4),
            T4 => p(2048, 2, 8000, 7, 10),
            T5 => p(2048, 2, 11025, 7, 10),
            T8 => p(1024, 2, 8000, 7, 10),
            T11 => p(1024, 1, 11025, 7, 10),
            T16 => p(512, 1, 8000, 7, 10),
            T22 => p(512, 1, 11025, 7, 10),
            T25x4 => p(320, 4, 8000, 15, 50),
            T50x1 => p(160, 1, 8000, 15, 50),
            T50x2 => p(160, 2, 8000, 15, 50),
            T100 => p(80, 1, 8000, 15, 50),
        }
    }

    /// The convolutional code for this submode. K=7 for the low-speed modes,
    /// K=15 for 25x4/50x1/50x2/100. ref: thor.cxx:342-350.
    pub fn conv_code(self) -> ConvCode {
        if self.params().k == 15 {
            ConvCode::thor_k15()
        } else {
            ConvCode::thor_k7()
        }
    }

    /// Viterbi traceback depth (fldigi `settraceback`): 45 for K=7, 15*12 for
    /// K=15. ref: thor.cxx:345,349.
    pub fn traceback(self) -> usize {
        if self.params().k == 15 {
            15 * 12
        } else {
            45
        }
    }

    pub fn samplerate(self) -> u32 {
        self.params().samplerate
    }

    pub fn samples_per_symbol(self) -> usize {
        self.params().symlen
    }

    /// Tone spacing in Hz: `samplerate * doublespaced / symlen`. ref: thor.cxx:299.
    pub fn tone_spacing(self) -> f32 {
        let p = self.params();
        p.samplerate as f32 * p.doublespaced as f32 / p.symlen as f32
    }

    /// Occupied bandwidth: `NUMTONES * tone_spacing`. ref: thor.cxx:301.
    pub fn bandwidth(self) -> f32 {
        NUMTONES as f32 * self.tone_spacing()
    }

    pub fn baud(self) -> f32 {
        let p = self.params();
        p.samplerate as f32 / p.symlen as f32
    }

    pub fn from_label(s: &str) -> Option<ThorVariant> {
        use ThorVariant::*;
        Some(match s {
            "thormicro" => Micro,
            "thor4" => T4,
            "thor5" => T5,
            "thor8" => T8,
            "thor11" => T11,
            "thor16" => T16,
            "thor22" => T22,
            "thor25x4" => T25x4,
            "thor50x1" => T50x1,
            "thor50x2" => T50x2,
            "thor100" => T100,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        use ThorVariant::*;
        match self {
            Micro => "thormicro",
            T4 => "thor4",
            T5 => "thor5",
            T8 => "thor8",
            T11 => "thor11",
            T16 => "thor16",
            T22 => "thor22",
            T25x4 => "thor25x4",
            T50x1 => "thor50x1",
            T50x2 => "thor50x2",
            T100 => "thor100",
        }
    }

    /// Every ported submode, for table-driven tests and the TUI selector.
    pub fn all() -> &'static [ThorVariant] {
        use ThorVariant::*;
        &[Micro, T4, T5, T8, T11, T16, T22, T25x4, T50x1, T50x2, T100]
    }
}

/// The IFK+ tone-index sequence for a text message: THOR varicode → streaming
/// conv encode → 4-bit interleaved nibbles → IFK+ tones. This is the exact
/// sequence fldigi's `sendchar`/`sendsymbol` emits for the *data* characters
/// (no preamble/framing), asserted bit-exact vs the golden vector.
/// ref: thor.cxx:1138-1159.
pub fn encode_symbols(v: ThorVariant, text: &str) -> Vec<u32> {
    encode_chars(v, text.bytes(), false)
}

/// Shared TX pipeline over a character stream. `secondary` selects the varicode
/// set. The convolutional encoder and interleaver carry state across the whole
/// stream (fldigi never flushes per character).
fn encode_chars(v: ThorVariant, chars: impl Iterator<Item = u8>, secondary: bool) -> Vec<u32> {
    let p = v.params();
    let mut enc = ConvEncoder::new(v.conv_code());
    let mut inlv = MfskInterleaver::<u8>::new(INTERLEAVE_SIZE, p.idepth, true, 0u8);
    let mut prev_tone = 0u32;
    let mut bitstate = 0;
    let mut bitshreg = 0u32;
    let mut coded = Vec::with_capacity(2);
    let mut out = Vec::new();
    for ch in chars {
        for bit in thor_varicode::encode(ch, secondary) {
            coded.clear();
            enc.encode(bit, &mut coded); // poly1 then poly2
            for &cb in &coded {
                bitshreg = (bitshreg << 1) | cb as u32;
                bitstate += 1;
                if bitstate == INTERLEAVE_SIZE {
                    inlv.bits(&mut bitshreg);
                    let tone = (prev_tone + 2 + bitshreg) % NUMTONES;
                    prev_tone = tone;
                    out.push(tone);
                    bitstate = 0;
                    bitshreg = 0;
                }
            }
        }
    }
    out
}

/// Idle (NUL) characters prepended before / appended after the data to prime and
/// drain the RX interleaver + Viterbi latency. fldigi frames with a short preamble
/// (`Clearbits` fills the interleaver *silently* + 16 symbols + idle) and a
/// `flushlength` idle tail; because our RX has no preamble *detection* yet (see the
/// module doc), the priming idle here is audible and must be long enough for the
/// far end's interleaver + Viterbi to fill on their own.
///
/// The dominant cost is the diagonal interleaver's fill, which is `~idepth`
/// symbols; the K=15 Viterbi traceback is a smaller second-order term. This is the
/// measured minimum that still decodes plus a safety margin — kept as small as
/// decoding allows so TX airtime stays close to fldigi's rather than the ~2×
/// over-pad it was. The high-speed modes (idepth=50, a ~2 s interleave) still carry
/// a real lead-in; eliminating it entirely needs fldigi's preamble detection.
fn frame_pad(v: ThorVariant) -> usize {
    // Empirically (loopback grid, `tests/kat.rs`) the floor that still decodes is
    // ≈ `0.9*idepth + 4` idle chars; `idepth + traceback/24 + 6` clears it on every
    // submode with ~20–50% margin without ballooning the low-idepth modes.
    let p = v.params();
    p.idepth + v.traceback() / 24 + 6
}

fn caps(v: ThorVariant) -> ModeCaps {
    ModeCaps {
        native_rate: v.samplerate(),
        bandwidth_hz: v.bandwidth(),
        tx: true,
        duplex: Duplex::Half,
        shape: DemodShape::Streaming,
    }
}

pub struct ThorMod {
    v: ThorVariant,
    center_hz: f32,
}

impl ThorMod {
    pub fn new(v: ThorVariant, center_hz: f32) -> Self {
        ThorMod { v, center_hz }
    }

    /// Lowest tone frequency: tone `k` sits at `center + (k - (NUMTONES-1)/2) *
    /// spacing`. ref: thor.cxx:1112 (`f = (tone + 0.5) * tonespacing + carrier -
    /// bandwidth/2`).
    fn base_hz(&self) -> f32 {
        self.center_hz - 0.5 * (NUMTONES as f32 - 1.0) * self.v.tone_spacing()
    }
}

impl Modulator for ThorMod {
    fn caps(&self) -> ModeCaps {
        caps(self.v)
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("thor needs text")),
        };
        // Wrap the data with an idle preamble/flush so the FEC+interleave RX can
        // prime and drain — the encoder/interleaver run as one continuous stream.
        let pad = frame_pad(self.v);
        let chars = std::iter::repeat_n(0u8, pad)
            .chain(text.bytes())
            .chain(std::iter::repeat_n(0u8, pad));
        let tones = encode_chars(self.v, chars, false);
        let p = self.v.params();
        let mfsk = MFsk::new(
            p.samplerate as f32,
            p.symlen,
            self.base_hz(),
            self.v.tone_spacing(),
            NUMTONES,
        );
        Ok(mfsk.modulate(&tones))
    }
}

pub struct ThorDemod {
    v: ThorVariant,
    center_hz: f32,
    buf: Vec<Sample>,
    prev_tone: u32,
    deinlv: MfskInterleaver<Llr>,
    viterbi: StreamingViterbi,
    datashreg: u32,
}

impl ThorDemod {
    pub fn new(v: ThorVariant, center_hz: f32) -> Self {
        let p = v.params();
        ThorDemod {
            v,
            center_hz,
            buf: Vec::new(),
            prev_tone: 0,
            deinlv: MfskInterleaver::<Llr>::new(INTERLEAVE_SIZE, p.idepth, false, 0.0),
            viterbi: v.conv_code().streaming_decoder(v.traceback()),
            datashreg: 1,
        }
    }

    fn base_hz(&self) -> f32 {
        self.center_hz - 0.5 * (NUMTONES as f32 - 1.0) * self.v.tone_spacing()
    }

    /// Decode every fully-buffered symbol: detect the tone, invert the IFK+
    /// (`nibble = (tone - prev - 2) mod 18`), unpack to 4 soft bits, reverse-
    /// interleave, feed the pairs to the streaming Viterbi and reframe the
    /// varicode. ref: thor.cxx:508-547 (decodesymbol/decodePairs).
    fn drain_symbols(&mut self) -> Vec<Frame> {
        let p = self.v.params();
        let sps = p.symlen;
        let spacing = self.v.tone_spacing();
        let base = self.base_hz();
        let rate = p.samplerate as f32;
        let mut out = Vec::new();
        let mut consumed = 0;
        while self.buf.len() - consumed >= sps {
            let block = &self.buf[consumed..consumed + sps];
            let powers = tone_powers(block, rate, base, spacing, NUMTONES);
            let tone = argmax(&powers) as u32;
            let nibble = (tone + NUMTONES * 2 - self.prev_tone - 2) % NUMTONES;
            self.prev_tone = tone;
            self.decode_nibble(nibble, &mut out);
            consumed += sps;
        }
        self.buf.drain(..consumed);
        out
    }

    /// Unpack the interleaved nibble to 4 soft bits (`symbols[0]` = MSB), reverse-
    /// interleave, and feed the two code pairs to the Viterbi + varicode framer.
    fn decode_nibble(&mut self, nibble: u32, out: &mut Vec<Frame>) {
        // symbols[3] = LSB … symbols[0] = MSB; hard bit → ±SOFT_MAG (LLR positive
        // ⇒ code bit 0, matching fldigi's 0/255 soft bytes). ref: thor.cxx:539-542.
        let mut soft = [0.0f32; INTERLEAVE_SIZE];
        let mut n = nibble;
        for idx in (0..INTERLEAVE_SIZE).rev() {
            soft[idx] = if n & 1 == 1 { -SOFT_MAG } else { SOFT_MAG };
            n >>= 1;
        }
        self.deinlv.symbols(&mut soft);
        for pair in soft.chunks(2) {
            if let Some(bit) = self.viterbi.push(pair) {
                self.datashreg = (self.datashreg << 1) | bit as u32;
                if self.datashreg & 7 == 1 {
                    let sym = self.datashreg >> 1;
                    if let Some(val) = thor_varicode::decode(sym) {
                        push_char(out, val);
                    }
                    self.datashreg = 1;
                }
            }
        }
    }
}

/// A decoded THOR varicode value as a frame. Primary values (`0..=255`) are the
/// character; secondary values (`0x100..=0x1FF`) carry fldigi's second live-text
/// stream, which it routes to a separate status line. We do not emit secondary
/// text on its own channel this phase (and nothing here transmits it), so a
/// secondary value would surface only its low byte as text; the `0xB80` decode
/// floor keeps primary and secondary codewords from colliding. NUL is dropped as
/// idle. ref: thor.cxx:438-462 (recvchar).
fn push_char(out: &mut Vec<Frame>, val: u16) {
    let ch = (val & 0xFF) as u8;
    if ch == 0 {
        return; // idle / NUL
    }
    out.push(Frame {
        payload: FramePayload::Text((ch as char).to_string()),
        meta: FrameMeta { crc_ok: true, decoder: Some("thor".into()), ..Default::default() },
    });
}

impl Demodulator for ThorDemod {
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
        self.prev_tone = 0;
        self.deinlv = MfskInterleaver::<Llr>::new(INTERLEAVE_SIZE, p.idepth, false, 0.0);
        self.viterbi = self.v.conv_code().streaming_decoder(self.v.traceback());
        self.datashreg = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- bit-exact KATs mirrored from the fldigi golden vector ----------------
    // These run in plain `cargo test` (CI does not enable the `testutil`
    // feature). Provenance: tests/vectors/thor_varicode.json.

    fn vector_line(mode: &str) -> String {
        let raw = include_str!("../../tests/vectors/thor_varicode.json");
        raw.lines()
            .find(|l| l.contains(&format!("\"mode\":\"{mode}\"")))
            .unwrap()
            .to_string()
    }

    fn field(line: &str, k: &str) -> String {
        let i = line.find(k).unwrap() + k.len();
        line[i..line[i..].find('"').unwrap() + i].to_string()
    }

    fn nums(s: &str) -> Vec<u32> {
        s.split(' ').map(|x| x.parse().unwrap()).collect()
    }

    /// Bit-exact: the IFK+ tone-index sequence for the reference message matches
    /// fldigi for both the K=7 (THOR16) and K=15 (THOR100) paths.
    #[test]
    fn tones_match_reference() {
        for (mode, v) in [("thor16", ThorVariant::T16), ("thor100", ThorVariant::T100)] {
            let line = vector_line(mode);
            let msg = field(&line, "\"msg\":\"");
            let want = nums(&field(&line, "\"tones\":\""));
            assert_eq!(encode_symbols(v, &msg), want, "{mode} tones differ from fldigi");
        }
    }

    /// Bit-exact stage KAT: the convolutional code pairs and the post-interleave
    /// nibbles reproduce fldigi's `Enc->encode` / `Txinlv->bits` output.
    #[test]
    fn code_pairs_and_interleave_match_reference() {
        for (mode, v) in [("thor16", ThorVariant::T16), ("thor100", ThorVariant::T100)] {
            let line = vector_line(mode);
            let msg = field(&line, "\"msg\":\"");
            let want_pairs = nums(&field(&line, "\"codepairs\":\""));
            let want_inlv = nums(&field(&line, "\"inlv\":\""));

            // Re-derive the code pairs (poly1 | poly2<<1) and interleaved nibbles.
            let mut enc = ConvEncoder::new(v.conv_code());
            let mut inlv = MfskInterleaver::<u8>::new(INTERLEAVE_SIZE, v.params().idepth, true, 0u8);
            let mut pairs = Vec::new();
            let mut nibbles = Vec::new();
            let mut bitstate = 0;
            let mut bitshreg = 0u32;
            let mut coded = Vec::new();
            for &ch in msg.as_bytes() {
                for bit in thor_varicode::encode(ch, false) {
                    coded.clear();
                    enc.encode(bit, &mut coded);
                    pairs.push(coded[0] as u32 | ((coded[1] as u32) << 1));
                    for &cb in &coded {
                        bitshreg = (bitshreg << 1) | cb as u32;
                        bitstate += 1;
                        if bitstate == INTERLEAVE_SIZE {
                            inlv.bits(&mut bitshreg);
                            nibbles.push(bitshreg);
                            bitstate = 0;
                            bitshreg = 0;
                        }
                    }
                }
            }
            assert_eq!(pairs, want_pairs, "{mode} code pairs differ from fldigi");
            assert_eq!(nibbles, want_inlv, "{mode} interleaved nibbles differ from fldigi");
        }
    }

    #[test]
    fn params_derive_spacing_and_baud() {
        // ref: thor.cxx:217-301 spot checks.
        assert!((ThorVariant::T16.tone_spacing() - 15.625).abs() < 1e-3);
        assert!((ThorVariant::T16.baud() - 15.625).abs() < 1e-3);
        assert!((ThorVariant::Micro.baud() - 2.0).abs() < 1e-3);
        assert_eq!(ThorVariant::T5.samplerate(), 11025);
        assert_eq!(ThorVariant::T8.samples_per_symbol(), 1024);
        assert_eq!(ThorVariant::T100.params().k, 15);
        assert_eq!(ThorVariant::T16.params().k, 7);
    }

    #[test]
    fn labels_round_trip() {
        for &v in ThorVariant::all() {
            assert_eq!(ThorVariant::from_label(v.label()), Some(v));
        }
        assert_eq!(ThorVariant::from_label("thor99"), None);
        assert_eq!(ThorVariant::all().len(), 11);
    }

    // ---- loopback gates -------------------------------------------------------

    fn loopback(v: ThorVariant, msg: &str) -> String {
        let mut tx = ThorMod::new(v, 1500.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = ThorDemod::new(v, 1500.0);
        let frames = rx.feed(&samples);
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// Assert a loopback recovered `msg`. The streaming demod has no preamble
    /// detection yet (deferred, like DominoEX's sync — see the module doc), so the
    /// Viterbi + interleaver startup emits a short, message-independent transient
    /// before the varicode framer locks; the message then arrives intact at the
    /// tail. We assert the strong invariants rather than a loose `contains`: the
    /// decoded stream **ends with** the message (so any dropped/corrupted/truncated
    /// character fails) **and** the leading transient stays bounded (so a
    /// regression that grows or garbles it also fails). Empirically the transient
    /// is ≤5 bytes across every submode; the bound gives a little margin.
    const MAX_STARTUP_TRANSIENT: usize = 8;
    fn assert_recovers(v: ThorVariant, msg: &str, got: &str) {
        assert!(got.ends_with(msg), "submode {} lost the message tail: {got:?}", v.label());
        let prefix = got.len() - msg.len();
        assert!(
            prefix <= MAX_STARTUP_TRANSIENT,
            "submode {} startup transient too long ({prefix} bytes): {got:?}",
            v.label()
        );
    }

    #[test]
    fn loopback_recovers_text_k7_submodes() {
        // The K=7 (low-speed) family, through varicode → FEC → interleave → IFK+
        // and back. The message arrives intact at the tail after a bounded
        // startup transient (idle preamble/flush frame it).
        let msg = "CQ DE K1ABC/7 2026";
        for &v in &[
            ThorVariant::Micro,
            ThorVariant::T4,
            ThorVariant::T8,
            ThorVariant::T16,
            ThorVariant::T22,
        ] {
            assert_recovers(v, msg, &loopback(v, msg));
        }
    }

    #[test]
    fn loopback_recovers_text_k15_submode() {
        // Exercise the K=15 long-constraint-length path end-to-end.
        let msg = "THOR 100 TEST";
        assert_recovers(ThorVariant::T100, msg, &loopback(ThorVariant::T100, msg));
    }

    #[test]
    fn loopback_recovers_punctuation_and_case() {
        let msg = "The quick brown fox! (73) $5 @ 90%";
        assert_recovers(ThorVariant::T16, msg, &loopback(ThorVariant::T16, msg));
    }
}
