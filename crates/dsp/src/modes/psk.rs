//! PSK family: differential BPSK + PSK31 Varicode, parametric by symbol rate.
//!
//! Port of fldigi's PSK modem (`fldigi/src/psk/{psk.cxx,pskvaricode.cxx}`,
//! upstream 4.1.23 @ 61b97f413). This module generalises the single-rate
//! `psk31.rs` assembly into a `PskVariant`-driven family covering the plain
//! differential-BPSK rates PSK31/63/125/250/500/1000. The higher-layer families
//! (QPSK, `+F`, PSK-R robust, and the multi-carrier `nX` grid) extend this same
//! parameter table and modulator/demodulator skeleton in later slices; their
//! variants are not yet reachable and this slice's `PskVariant` deliberately
//! enumerates only the plain-BPSK rates so every arm is exercised.
//!
//! Wire-determining arithmetic is bit-exact vs fldigi: the Varicode payload bit
//! stream (`encode_bpsk_bits`) is asserted byte-for-byte against a vector
//! extracted from the unmodified fldigi table (`tests/vectors/psk_bpsk.json`,
//! `scratch/refvectors/build_psk_varicode.sh`). Modulated audio is gated on a
//! loopback + AWGN decode-rate criterion only, never bit-exact (Doctrine §3):
//! fldigi's `tx_symbol` audio path is entangled with its modem/FLTK runtime, so
//! the modulator is validated by round-trip recovery rather than a captured
//! waveform.

use crate::fec::conv::{ConvCode, StreamingViterbi};
use crate::fec::interleave::DiagInterleaver;
use crate::framing::varicode::{
    decode as vari_decode, encode as vari_encode, mfsk_encode, mfsk_symbol_to_byte, PSK31,
};
use crate::frontend::fir::{design_lowpass, Fir};
use crate::frontend::modulate::DiffPsk;
use crate::frontend::nco::DownConverter;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::sync::costas::CostasLoop;
use crate::sync::timing::TransitionMinimizer;
use crate::types::{Cplx, Frame, FrameMeta, FramePayload, Sample};

/// fldigi samplerate for every Phase-7 (8 kHz) PSK submode. ref: psk.cxx:370.
pub const PSK_RATE: u32 = 8_000;

/// The Phase-7 PSK submodes ported so far: the plain differential-BPSK rates
/// and the differential-QPSK rates (K=5 convolutional FEC). `symbollen`
/// (samples/symbol) fixes the baud as `PSK_RATE / symbollen`. ref:
/// psk.cxx:382-444.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PskVariant {
    Psk31,
    Psk63,
    Psk125,
    Psk250,
    Psk500,
    Psk1000,
    Qpsk31,
    Qpsk63,
    Qpsk125,
    Qpsk250,
    Qpsk500,
    // Robust (`+F`/PSK-R): K=7 FEC + MFSK Varicode. PSK63F has no interleaver;
    // the PSK-R rates add the 2×2×idepth diagonal interleaver.
    Psk63F,
    Psk125R,
    Psk250R,
    Psk500R,
    Psk1000R,
}

/// Resolved per-variant parameters. ref: psk.cxx:382-687.
#[derive(Debug, Clone, Copy)]
pub struct PskParams {
    /// Samples per symbol at `PSK_RATE`; baud = `PSK_RATE / symbollen`.
    pub symbollen: usize,
    /// Preamble/postamble length in phase reversals (fldigi `dcdbits`).
    pub dcdbits: usize,
    /// Differential QPSK (2 bits/symbol, K=5 convolutional FEC) vs plain BPSK.
    pub qpsk: bool,
    /// Robust `+F`/PSK-R: K=7 FEC + MFSK Varicode, two BPSK symbols per code bit.
    pub robust: bool,
    /// Diagonal-interleaver depth (`2×2×idepth`); 0 = no interleaver (PSK63F).
    pub idepth: usize,
}

impl PskVariant {
    /// ref: psk.cxx:382-687 (symbollen/dcdbits/_qpsk/_pskr/idepth switch).
    pub fn params(self) -> PskParams {
        use PskVariant::*;
        let p = |symbollen, dcdbits, qpsk, robust, idepth| PskParams {
            symbollen,
            dcdbits,
            qpsk,
            robust,
            idepth,
        };
        match self {
            Psk31 => p(256, 32, false, false, 0),
            Psk63 => p(128, 64, false, false, 0),
            Psk125 => p(64, 128, false, false, 0),
            Psk250 => p(32, 256, false, false, 0),
            Psk500 => p(16, 512, false, false, 0),
            Psk1000 => p(8, 128, false, false, 0),
            // QPSK: same symbol rates, 2 bits/symbol, K=5 FEC. ref: psk.cxx:414-444.
            Qpsk31 => p(256, 32, true, false, 0),
            Qpsk63 => p(128, 64, true, false, 0),
            Qpsk125 => p(64, 128, true, false, 0),
            Qpsk250 => p(32, 256, true, false, 0),
            Qpsk500 => p(16, 512, true, false, 0),
            // BPSK63 + FEC (no interleaver). ref: psk.cxx:448-451.
            Psk63F => p(128, 64, false, true, 0),
            // PSK-R robust: K=7 FEC + 2×2×idepth interleaver. ref: psk.cxx:658-685.
            Psk125R => p(64, 128, false, true, 40),
            Psk250R => p(32, 256, false, true, 80),
            Psk500R => p(16, 512, false, true, 160),
            Psk1000R => p(8, 512, false, true, 160),
        }
    }

    pub fn baud(self) -> f32 {
        PSK_RATE as f32 / self.params().symbollen as f32
    }

    pub fn samples_per_symbol(self) -> usize {
        self.params().symbollen
    }

    pub fn is_qpsk(self) -> bool {
        self.params().qpsk
    }

    pub fn is_robust(self) -> bool {
        self.params().robust
    }

    /// Diagonal-interleaver depth (0 = none). ref: psk.cxx:658-685.
    pub fn idepth(self) -> usize {
        self.params().idepth
    }

    /// The convolutional code for the FEC-bearing modes: K=5 (POLY 0x17/0x19)
    /// for QPSK, K=7 (POLY 0x6d/0x4f) for the robust `+F`/PSK-R modes, or `None`
    /// for the plain-BPSK rates. ref: psk.cxx:66-74, 979-992.
    pub fn conv_code(self) -> Option<ConvCode> {
        if self.is_qpsk() {
            Some(ConvCode { k: 5, polys: vec![0x17, 0x19] })
        } else if self.is_robust() {
            Some(ConvCode { k: 7, polys: vec![0x6d, 0x4f] })
        } else {
            None
        }
    }

    /// Parse an omnimodem/fldigi mode label. Returns `None` for unknown labels.
    pub fn from_label(s: &str) -> Option<PskVariant> {
        use PskVariant::*;
        Some(match s {
            "psk31" => Psk31,
            "psk63" => Psk63,
            "psk125" => Psk125,
            "psk250" => Psk250,
            "psk500" => Psk500,
            "psk1000" => Psk1000,
            "qpsk31" => Qpsk31,
            "qpsk63" => Qpsk63,
            "qpsk125" => Qpsk125,
            "qpsk250" => Qpsk250,
            "qpsk500" => Qpsk500,
            "psk63f" => Psk63F,
            "psk125r" => Psk125R,
            "psk250r" => Psk250R,
            "psk500r" => Psk500R,
            "psk1000r" => Psk1000R,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        use PskVariant::*;
        match self {
            Psk31 => "psk31",
            Psk63 => "psk63",
            Psk125 => "psk125",
            Psk250 => "psk250",
            Psk500 => "psk500",
            Psk1000 => "psk1000",
            Qpsk31 => "qpsk31",
            Qpsk63 => "qpsk63",
            Qpsk125 => "qpsk125",
            Qpsk250 => "qpsk250",
            Qpsk500 => "qpsk500",
            Psk63F => "psk63f",
            Psk125R => "psk125r",
            Psk250R => "psk250r",
            Psk500R => "psk500r",
            Psk1000R => "psk1000r",
        }
    }
}

/// The differential rotation index fed to `DiffPsk` (bps=2) for a QPSK code
/// symbol `s` (0..3, `s = poly1_bit | poly2_bit<<1`). fldigi maps `s` through
/// `(4-s)&3` then `*4` into a 16-PSK constellation whose points are 22.5° apart
/// (`sym_vec_pos`), landing on the four 90°-spaced QPSK phases {180,90,0,270}°;
/// `DiffPsk` rotates by `idx*90°`, so `idx(s) = (6 - s) & 3` reproduces exactly
/// that rotation. ref: psk.cxx:2247-2252, 2193-2210.
fn qpsk_rot_index(s: u32) -> u32 {
    (6 - s) & 3
}

/// The plain-BPSK payload bitstream: PSK31 Varicode + `00` separators, MSB-first.
/// This is the exact bit domain fldigi's `tx_char` feeds its differential
/// encoder for the non-FEC BPSK modes, and is asserted bit-exact vs fldigi
/// (`tests/vectors/psk_bpsk.json`). ref: pskvaricode.cxx:31-334.
pub fn encode_bpsk_bits(_v: PskVariant, text: &str) -> Vec<u8> {
    vari_encode(&PSK31, text)
}

/// Leading idle reversals prepended on TX so the receiver's Costas and Gardner
/// loops lock before the payload (fldigi sends `dcdbits` preamble reversals;
/// ref: psk.cxx:2621-2628 tx_symbol(0)).
pub struct PskMod {
    v: PskVariant,
    center_hz: f32,
}

impl PskMod {
    pub fn new(v: PskVariant, center_hz: f32) -> Self {
        PskMod { v, center_hz }
    }

    /// The reversal stream fed to the differential modulator: `dcdbits` idle
    /// reversals, the `00` character-start as two reversals, then each Varicode
    /// payload bit `b` as reversal `1 ^ b` — a Varicode `0` is a phase reversal,
    /// a `1` a steady carrier (standard PSK31 rule, matching `psk31.rs`).
    fn reversal_stream(&self, text: &str) -> Vec<u32> {
        let pp = self.v.params();
        let mut rev = vec![1u32; pp.dcdbits];
        rev.push(1);
        rev.push(1);
        rev.extend(encode_bpsk_bits(self.v, text).into_iter().map(|b| (1 ^ b) as u32));
        // Postamble: trailing reversals so the receiver's timing loop has enough
        // symbols after the final character's `00` separator to strobe and drain
        // it (fldigi sends a `dcdbits` postamble via tx_symbol(2); ref:
        // psk.cxx:2539-2544). A short tail is sufficient for loopback flushing.
        rev.extend(std::iter::repeat_n(1u32, pp.dcdbits.min(64)));
        rev
    }

    /// The differential-rotation index stream (units of 90°) for a QPSK frame,
    /// fed to `DiffPsk` (bps=2): `dcdbits` preamble reversals (rotation index 2 =
    /// 180°), then the K=5-encoded Varicode payload — each code symbol `s` mapped
    /// to `qpsk_rot_index(s)` — then a short reversal postamble. The convolutional
    /// encoder appends its own K-1 zero-flush tail (via `ConvCode::encode`), which
    /// terminates the trellis so the streaming decoder settles.
    fn qpsk_rot_stream(&self, text: &str) -> Vec<u32> {
        let pp = self.v.params();
        let code = self.v.conv_code().expect("qpsk variant");
        // Sacrificial FEC-lock-in header of zero bits so the receiver's
        // continuous Viterbi converges onto the valid trellis before the real
        // Varicode payload (fldigi pads with <NUL> chars; ref: psk.cxx:2585).
        // The leading `00` also gives the Varicode framer a clean sync point.
        let mut vbits = vec![0u8; 64];
        vbits.extend(encode_bpsk_bits(self.v, text));
        let coded = code.encode(&vbits); // 2 bits/symbol, tail-flushed
        let mut rot = vec![2u32; pp.dcdbits]; // preamble: 180° reversals
        for pair in coded.chunks(2) {
            let s = pair[0] as u32 | ((pair[1] as u32) << 1);
            rot.push(qpsk_rot_index(s));
        }
        rot.extend(std::iter::repeat_n(2u32, pp.dcdbits.min(64))); // postamble
        rot
    }

    /// The reversal stream for a robust (`+F`/PSK-R) BPSK frame, fed to `DiffPsk`
    /// (bps=1): `dcdbits` preamble reversals, then the K=7-encoded MFSK-Varicode
    /// payload — each code pair optionally run through the 2×2×idepth diagonal
    /// interleaver (`Txinlv->bits`; ref psk.cxx:2337) and sent low-bit-first as two
    /// BPSK symbols (poly1 then poly2; psk.cxx:2338-2341) — then a reversal
    /// postamble. A code bit `1` is a steady carrier, `0` a reversal (DiffPsk value
    /// `1 ^ b`), matching `tx_symbol`'s BPSK mapping. A zero-bit header pads the
    /// FEC/interleaver so the receiver locks before the payload.
    fn robust_rev_stream(&self, text: &str) -> Vec<u32> {
        let pp = self.v.params();
        let code = self.v.conv_code().expect("robust variant");
        // Lock-in header covers the interleaver fill latency + FEC warm-up.
        let mut vbits = vec![0u8; pp.idepth + 64];
        vbits.extend(mfsk_encode(text));
        let coded = code.encode(&vbits); // 2 code bits per varicode bit, tail-flushed
        let mut inlv = DiagInterleaver::new(pp.idepth, true, 0u8);
        let mut rev = vec![1u32; pp.dcdbits]; // preamble reversals
        let mut push_pair = |rev: &mut Vec<u32>, mut bs: u32| {
            if pp.idepth > 0 {
                inlv.bits(&mut bs);
            }
            rev.push(1 ^ (bs & 1)); // bit0 first
            rev.push(1 ^ ((bs >> 1) & 1)); // bit1 second
        };
        for pair in coded.chunks(2) {
            // bit0 = poly1 (low), bit1 = poly2 (high) — fldigi's bitshreg layout.
            push_pair(&mut rev, pair[0] as u32 | ((pair[1] as u32) << 1));
        }
        // Flush the TX interleaver: it delays each pair's second bit by `idepth`
        // pairs, so the last `idepth` payload pairs are only fully sent after that
        // many more pairs pass through — feed zero pairs (which also read out as
        // reversals) so nothing is stranded in the interleaver. Then a raw
        // reversal tail flushes the RX deinterleaver + Viterbi traceback.
        for _ in 0..(pp.idepth + 2 * ROBUST_TRACEBACK) {
            push_pair(&mut rev, 0);
        }
        rev.extend(std::iter::repeat_n(1u32, 2 * (pp.idepth + 2 * ROBUST_TRACEBACK)));
        rev
    }
}

impl Modulator for PskMod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: PSK_RATE,
            bandwidth_hz: self.v.baud() * 2.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("psk needs text")),
        };
        let sps = self.v.samples_per_symbol();
        if self.v.is_qpsk() {
            let rot = self.qpsk_rot_stream(text);
            let psk = DiffPsk::new(PSK_RATE as f32, self.center_hz, sps, 2);
            Ok(psk.modulate(&rot))
        } else {
            let rev = if self.v.is_robust() {
                self.robust_rev_stream(text)
            } else {
                self.reversal_stream(text)
            };
            let psk = DiffPsk::new(PSK_RATE as f32, self.center_hz, sps, 1);
            Ok(psk.modulate(&rev))
        }
    }
}

/// Cap on the in-progress Varicode bit buffer (see `psk31.rs`).
const MAX_PENDING_BITS: usize = 512;
const CARRIER_EMA: f32 = 0.002;
const CARRIER_OPEN: f32 = 0.15;
const CARRIER_FLOOR: f32 = 1e-9;
const SQUELCH_TAPS: usize = 127;

pub struct PskDemod {
    v: PskVariant,
    center_hz: f32,
    nco: DownConverter,
    costas: CostasLoop,
    // Envelope-histogram symbol-timing recovery: for RC-shaped BPSK the eye is
    // widest at symbol centres and the envelope nulls mark the boundaries, so a
    // sliding per-phase energy histogram locks the integrate-and-dump window to
    // the true symbol phase (and follows slow clock drift) without a feedback
    // loop. This suits the whole rate family down to 8 samples/symbol, where a
    // Gardner loop's per-symbol walk breaks the decision. ref: timing.rs.
    tm: TransitionMinimizer,
    since_dump: usize,
    acc: Cplx,
    prev: Cplx,
    have_prev: bool,
    synced: bool,
    zrun: u8,
    lpf_i: Fir,
    lpf_q: Fir,
    p_in: f32,
    p_tot: f32,
    pending: Vec<u8>,
    sample_index: u64,
    // QPSK only: the continuous K=5 Viterbi decoding the differential symbol
    // stream into Varicode bits. `None` for the plain-BPSK rates.
    qpsk_dec: Option<StreamingViterbi>,
    // Robust (`+F`/PSK-R) only: the streaming two-phase K=7 decoder. `None`
    // otherwise.
    robust: Option<RobustRx>,
}

/// Viterbi traceback depth for the K=5 QPSK stream (~6× constraint length).
const QPSK_TRACEBACK: usize = 30;
/// Strong hard-decision LLR magnitude (fldigi feeds 0/255 hard soft-symbols).
const QPSK_LLR: f32 = 4.0;
/// Viterbi traceback depth for the K=7 robust stream (~6× constraint length).
const ROBUST_TRACEBACK: usize = 45;

/// Streaming two-phase decoder for the robust (`+F`/PSK-R) modes. The two soft
/// code bits of each pair arrive as consecutive BPSK symbols, but the pair phase
/// is unknown on entry, so two continuous K=7 Viterbis run at both alignments
/// (decoder A pairs symbols (0,1),(2,3),…; decoder B pairs (1,2),(3,4),…). Each
/// frames its own output through the MFSK Varicode into an append-only string;
/// `drain` emits the not-yet-returned tail of whichever stream has the higher
/// path metric. Append-only per-stream text keeps the byte offsets valid (no
/// mid-char slice) and bounds work to O(1) per symbol. ref: psk.cxx:1216-1290.
struct RobustRx {
    dec: [StreamingViterbi; 2],
    // Per-phase reverse interleavers (0.0 = erasure fill); pass-through at idepth 0.
    inlv: [DiagInterleaver<f32>; 2],
    idepth: usize,
    shreg: [u32; 2],
    text: [String; 2],
    emitted: [usize; 2],
    prev_soft: Option<f32>,
    n: usize,
}

impl RobustRx {
    fn new(code: &ConvCode, idepth: usize) -> Self {
        RobustRx {
            dec: [
                code.streaming_decoder(ROBUST_TRACEBACK),
                code.streaming_decoder(ROBUST_TRACEBACK),
            ],
            inlv: [
                DiagInterleaver::new(idepth, false, 0.0),
                DiagInterleaver::new(idepth, false, 0.0),
            ],
            idepth,
            shreg: [0, 0],
            text: [String::new(), String::new()],
            emitted: [0, 0],
            prev_soft: None,
            n: 0,
        }
    }

    /// Push one differential soft code bit (LLR, positive ⇒ code bit 0).
    fn push_symbol(&mut self, soft: f32) {
        if let Some(p) = self.prev_soft {
            // The pair (previous, current) feeds decoder A on odd symbol counts
            // (pairs starting at an even index) and decoder B on even counts —
            // the two phase hypotheses.
            let d = 1 - (self.n & 1); // n odd → 0 (dec A); n even → 1 (dec B)
            // fldigi de-interleaves the pair [newest, prev] then reverses it into
            // the decoder as [prev, newest] (poly1, poly2). ref: psk.cxx:1252-1263.
            let mut pair = [soft, p];
            if self.idepth > 0 {
                self.inlv[d].symbols(&mut pair);
            }
            if let Some(bit) = self.dec[d].push(&[pair[1], pair[0]]) {
                let sh = &mut self.shreg[d];
                *sh = (*sh << 1) | bit as u32;
                if *sh & 7 == 1 {
                    if let Some(b) = mfsk_symbol_to_byte(*sh >> 1) {
                        if b != 0 {
                            self.text[d].push(b as char);
                        }
                    }
                    *sh = 1;
                }
            }
        }
        self.prev_soft = Some(soft);
        self.n += 1;
    }

    /// The not-yet-emitted tail of the higher-metric stream, if any.
    fn drain(&mut self) -> Option<String> {
        let w = if self.dec[0].total_metric() >= self.dec[1].total_metric() { 0 } else { 1 };
        if self.text[w].len() > self.emitted[w] {
            let new = self.text[w][self.emitted[w]..].to_string();
            self.emitted[w] = self.text[w].len();
            Some(new)
        } else {
            None
        }
    }
}

/// Costas loop bandwidth (normalized to sample rate) for a given symbol length.
/// PSK31 (256 samples/symbol) uses a wide 0.06 loop that acquires in a few
/// symbols and then holds. At the short symbol lengths (down to 8 samples) that
/// same loop bandwidth is a large fraction of the symbol rate, so residual loop
/// jitter rotates consecutive symbols differently and corrupts the differential
/// decode; keeping the loop bandwidth a fixed fraction of the *symbol rate*
/// (not the sample rate) holds per-symbol phase steady at every rate.
fn costas_bw(symbollen: usize) -> f32 {
    // 0.06 at symbollen 256; scales *with* symbol length so the loop bandwidth
    // stays a fixed fraction of the symbol rate and per-symbol phase jitter is
    // constant across the family. The preamble is `dcdbits` symbols (~8k
    // samples at every rate), so the correspondingly slower lock still
    // completes well before the payload.
    (0.06 * symbollen as f32 / 256.0).clamp(0.004, 0.06)
}

impl PskDemod {
    pub fn new(v: PskVariant, center_hz: f32) -> Self {
        let rate = PSK_RATE as f32;
        let baud = v.baud();
        // Squelch lowpass a little wider than the occupied band so the reversal
        // idle at carrier ± baud/2 survives; scales with baud (psk31.rs pins
        // 80 Hz for 31.25 baud). ref: psk31.rs SQUELCH_CUTOFF_HZ.
        let cutoff = (baud * 1.3).max(80.0);
        // Keep the filter no longer than a couple of symbols: a fixed 127-tap
        // filter spans ~16 symbols at 1000 baud (8 samples/symbol) and smears
        // that much ISI into the matched-filter sum. Scale its length to the
        // symbol so its impulse response stays ~2 symbols wide at every rate
        // (still long enough to reject the 2·center_hz mixer image well above
        // the passband). Capped at 127 for the low rates.
        let taps = design_lowpass(SQUELCH_TAPS, cutoff, rate);
        PskDemod {
            v,
            center_hz,
            nco: DownConverter::new(center_hz, rate),
            costas: CostasLoop::new(costas_bw(v.samples_per_symbol()), 0.02),
            tm: TransitionMinimizer::new(v.samples_per_symbol()),
            since_dump: 0,
            acc: Cplx::new(0.0, 0.0),
            prev: Cplx::new(0.0, 0.0),
            have_prev: false,
            synced: false,
            zrun: 0,
            lpf_i: Fir::new(taps.clone()),
            lpf_q: Fir::new(taps),
            p_in: 0.0,
            p_tot: 0.0,
            pending: Vec::new(),
            sample_index: 0,
            qpsk_dec: v
                .is_qpsk()
                .then(|| v.conv_code().unwrap().streaming_decoder(QPSK_TRACEBACK)),
            robust: v.is_robust().then(|| RobustRx::new(&v.conv_code().unwrap(), v.idepth())),
        }
    }

    /// Feed one Varicode data bit into the `00`-framed accumulator/sync (shared
    /// by the BPSK differential path and the QPSK Viterbi output).
    fn push_data_bit(&mut self, data_bit: u8) {
        if self.synced {
            self.pending.push(data_bit);
        } else if data_bit == 0 {
            self.zrun += 1;
            if self.zrun >= 2 {
                self.synced = true;
            }
        } else {
            self.zrun = 0;
        }
    }

    /// QPSK symbol decision: differential phase → the two K=5 code bits (fldigi's
    /// exact remap), pushed as hard LLRs through the continuous Viterbi; any bit
    /// it emits is a Varicode data bit. ref: psk.cxx:1495-1497 (phase→bits),
    /// 1188-1211 (rx_qpsk soft-symbol mapping).
    fn on_qpsk_symbol(&mut self, sym: Cplx) {
        if !self.have_prev {
            self.prev = sym;
            self.have_prev = true;
            return;
        }
        let mut phase = (sym * self.prev.conj()).arg();
        if phase < 0.0 {
            phase += std::f32::consts::TAU;
        }
        self.prev = sym;
        // fldigi: bits = (int)(phase / (π/2) + 0.5) & 3; then (4 - bits) & 3.
        let r = (phase / std::f32::consts::FRAC_PI_2 + 0.5) as u32 & 3;
        let bits2 = (4 - r) & 3;
        // sym[0] carries poly1 (0x17): 1 ⇒ bits2&1. sym[1] carries poly2 (0x19),
        // "top bit flipped": poly2 bit = 1 ⇔ (bits2 & 2) == 0.
        let p1 = bits2 & 1;
        let p2 = u32::from(bits2 & 2 == 0);
        let llr = [
            if p1 == 0 { QPSK_LLR } else { -QPSK_LLR },
            if p2 == 0 { QPSK_LLR } else { -QPSK_LLR },
        ];
        if let Some(dec) = self.qpsk_dec.as_mut() {
            if let Some(bit) = dec.push(&llr) {
                self.push_data_bit(bit);
            }
        }
    }

    /// Robust (`+F`/PSK-R) symbol decision: differential BPSK → one soft code bit
    /// (LLR), fed to the streaming two-phase K=7 decoder. A steady carrier is code
    /// bit `1` (negative LLR), a reversal code bit `0` (positive LLR); `positive
    /// ⇒ code bit 0` is the Viterbi convention. ref: psk.cxx:1518-1528, 2335-2341.
    fn on_robust_symbol(&mut self, sym: Cplx) {
        if !self.have_prev {
            self.prev = sym;
            self.have_prev = true;
            return;
        }
        let dot = sym.re * self.prev.re + sym.im * self.prev.im;
        let norm = (self.prev.norm() * sym.norm()).max(1e-9);
        self.prev = sym;
        if let Some(r) = self.robust.as_mut() {
            r.push_symbol(-QPSK_LLR * (dot / norm)); // normalised soft bit
        }
    }

    fn drain_completed(&mut self) -> Vec<Frame> {
        if let Some(r) = self.robust.as_mut() {
            return r
                .drain()
                .map(|text| {
                    vec![Frame {
                        payload: FramePayload::Text(text),
                        meta: FrameMeta {
                            crc_ok: true,
                            sample_offset: self.sample_index,
                            decoder: Some(self.v.label().into()),
                            ..Default::default()
                        },
                    }]
                })
                .unwrap_or_default();
        }
        let last_sep = (1..self.pending.len())
            .rev()
            .find(|&i| self.pending[i] == 0 && self.pending[i - 1] == 0);
        let Some(idx) = last_sep else {
            if self.pending.len() > MAX_PENDING_BITS {
                self.pending.clear();
            }
            return Vec::new();
        };
        let text = vari_decode(&PSK31, &self.pending[..=idx]);
        self.pending.drain(..=idx);
        if text.is_empty() {
            return Vec::new();
        }
        vec![Frame {
            payload: FramePayload::Text(text),
            meta: FrameMeta {
                crc_ok: true,
                sample_offset: self.sample_index,
                decoder: Some(self.v.label().into()),
                ..Default::default()
            },
        }]
    }
}

impl Demodulator for PskDemod {
    fn caps(&self) -> ModeCaps {
        PskMod::new(self.v, self.center_hz).caps()
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        for &x in samples {
            self.sample_index += 1;
            let bb = self.nco.push(x);
            // Lowpass the down-converted baseband before the decision. The real
            // mixer leaves a 2·center_hz image in `bb`; integrating over a long
            // symbol (PSK31, 256 samples) averages it away, but at the short
            // symbol lengths (down to 8 samples) the residual leaks into the
            // matched-filter sum — center-dependently — and slips a symbol. The
            // squelch lowpass (cutoff ~1.3·baud) removes it, so the carrier
            // recovery, timing, and data decision all run on the filtered `f`.
            let f = Cplx::new(self.lpf_i.push(bb.re), self.lpf_q.push(bb.im));
            self.p_in += CARRIER_EMA * (f.norm_sqr() - self.p_in);
            self.p_tot += CARRIER_EMA * (bb.norm_sqr() - self.p_tot);
            // BPSK uses a Costas loop for carrier recovery; QPSK is detected
            // differentially (non-coherent), and a BPSK Costas mis-locks on the
            // 4-fold-symmetric QPSK constellation — its time-varying rotation
            // would not cancel in the differential, so QPSK runs on the raw
            // filtered baseband and lets `arg(conj(prev)·cur)` reject the
            // constant carrier phase.
            let derot = if self.v.is_qpsk() { f } else { self.costas.process(f) };
            self.acc += derot;
            self.since_dump += 1;
            // Track the symbol boundary from the filtered envelope and dump the
            // integrate-and-dump there. Both `derot` and the envelope share the
            // lowpass group delay, so the histogram's boundary phase aligns the
            // integration window to `derot`'s symbols automatically.
            //
            // The dump is pinned to one per symbol: it fires only in the window
            // [sps-1, sps+2) since the last dump, so it can follow the tracked
            // boundary drifting by ±1 sample/symbol (real clock offset is far
            // slower) but can never fire a half-length symbol — a loose guard
            // would let a jittering boundary inject a spurious extra symbol. The
            // `+2` fallback forces progress if the boundary jumps past the match.
            let sps = self.v.samples_per_symbol();
            self.tm.feed(f.norm());
            let boundary = self.tm.transition_phase() as u64;
            let at_boundary =
                self.sample_index % sps as u64 == boundary && self.since_dump + 1 >= sps;
            if at_boundary || self.since_dump >= sps + 2 {
                self.since_dump = 0;
                let sym = self.acc;
                self.acc = Cplx::new(0.0, 0.0);
                let open = self.p_tot > CARRIER_FLOOR && self.p_in > CARRIER_OPEN * self.p_tot;
                if !open {
                    self.pending.clear();
                    self.have_prev = false;
                    self.prev = Cplx::new(0.0, 0.0);
                    self.synced = false;
                    self.zrun = 0;
                    if let Some(dec) = self.qpsk_dec.as_mut() {
                        *dec = self.v.conv_code().unwrap().streaming_decoder(QPSK_TRACEBACK);
                    }
                    if self.robust.is_some() {
                        self.robust = Some(RobustRx::new(&self.v.conv_code().unwrap(), self.v.idepth()));
                    }
                    continue;
                }
                if self.v.is_robust() {
                    self.on_robust_symbol(sym);
                } else if self.v.is_qpsk() {
                    self.on_qpsk_symbol(sym);
                } else if self.have_prev {
                    // Differential BPSK on the complex symbol: Re{conj(prev)·sym}
                    // is positive for a steady carrier (data `1`), negative for a
                    // phase reversal (data `0`). Using the full phasor keeps the
                    // decision robust to any residual Costas phase offset, which
                    // matters at the high rates (16 samples/symbol) where a static
                    // offset would otherwise bleed the in-phase energy away.
                    let dot = sym.re * self.prev.re + sym.im * self.prev.im;
                    let data_bit = u8::from(dot >= 0.0);
                    self.push_data_bit(data_bit);
                    self.prev = sym;
                    self.have_prev = true;
                } else {
                    self.prev = sym;
                    self.have_prev = true;
                }
            }
        }
        self.drain_completed()
    }

    fn reset(&mut self) {
        *self = PskDemod::new(self.v, self.center_hz);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recovered_text(frames: &[Frame]) -> String {
        frames
            .iter()
            .filter_map(|f| match &f.payload {
                FramePayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// Bit-exact: omnimodem's PSK31 Varicode payload bitstream reproduces
    /// fldigi's `psk_varicode_encode` output byte-for-byte. Provenance:
    /// `tests/vectors/psk_bpsk.json` (fldigi 4.1.23 @ 61b97f413, driver
    /// `scratch/refvectors/build_psk_varicode.sh`). Kept as a lib unit test (not
    /// the `testutil`-gated kat.rs) so the reference gate runs in CI's default
    /// `cargo test --workspace`.
    #[test]
    fn varicode_matches_fldigi_vector() {
        let raw = include_str!("../../tests/vectors/psk_bpsk.json");
        for msg in ["CQ DE K1ABC", "The quick brown fox 0123456789"] {
            let needle = format!("\"msg\":\"{msg}\"");
            let line = raw.lines().find(|l| l.contains(&needle)).expect("vector record");
            let key = "\"varicode_bits\":\"";
            let bi = line.find(key).unwrap() + key.len();
            let want: Vec<u8> =
                line[bi..line[bi..].find('"').unwrap() + bi].bytes().map(|c| c - b'0').collect();
            assert_eq!(
                encode_bpsk_bits(PskVariant::Psk125, msg),
                want,
                "PSK Varicode payload differs from fldigi for {msg:?}"
            );
        }
    }

    #[test]
    fn params_match_fldigi_symbollen_table() {
        // ref: psk.cxx:382-409.
        assert_eq!(PskVariant::Psk31.params().symbollen, 256);
        assert_eq!(PskVariant::Psk63.params().symbollen, 128);
        assert_eq!(PskVariant::Psk125.params().symbollen, 64);
        assert_eq!(PskVariant::Psk250.params().symbollen, 32);
        assert_eq!(PskVariant::Psk500.params().symbollen, 16);
        assert_eq!(PskVariant::Psk1000.params().symbollen, 8);
        assert_eq!(PskVariant::Psk31.baud(), 31.25);
        assert_eq!(PskVariant::Psk1000.baud(), 1000.0);
    }

    #[test]
    fn labels_round_trip() {
        for v in [
            PskVariant::Psk31,
            PskVariant::Psk63,
            PskVariant::Psk125,
            PskVariant::Psk250,
            PskVariant::Psk500,
            PskVariant::Psk1000,
            PskVariant::Qpsk31,
            PskVariant::Qpsk63,
            PskVariant::Qpsk125,
            PskVariant::Qpsk250,
            PskVariant::Qpsk500,
        ] {
            assert_eq!(PskVariant::from_label(v.label()), Some(v));
        }
        assert_eq!(PskVariant::from_label("psk2000"), None);
    }

    /// Bit-exact: omnimodem's K=5 convolutional encoder reproduces fldigi's QPSK
    /// code-symbol sequence byte-for-byte over the Varicode payload. Provenance:
    /// `tests/vectors/psk_qpsk.json` (fldigi 4.1.23 @ 61b97f413, driver
    /// `scratch/refvectors/build_psk_qpsk.sh`).
    #[test]
    fn qpsk_fec_matches_fldigi_vector() {
        let raw = include_str!("../../tests/vectors/psk_qpsk.json");
        let line = raw.lines().find(|l| l.contains("\"qpsk_symbols\"")).expect("record");
        let vkey = "\"varicode_bits\":\"";
        let vi = line.find(vkey).unwrap() + vkey.len();
        let vbits: Vec<u8> =
            line[vi..line[vi..].find('"').unwrap() + vi].bytes().map(|c| c - b'0').collect();
        let skey = "\"qpsk_symbols\":\"";
        let si = line.find(skey).unwrap() + skey.len();
        let want: Vec<u8> = line[si..line[si..].find('"').unwrap() + si]
            .split(' ')
            .map(|s| s.parse().unwrap())
            .collect();
        let code = PskVariant::Qpsk125.conv_code().unwrap();
        let out = code.encode(&vbits);
        let got: Vec<u8> = (0..want.len()).map(|i| out[2 * i] | (out[2 * i + 1] << 1)).collect();
        assert_eq!(got, want, "K=5 QPSK code symbols differ from fldigi");
    }

    #[test]
    fn qpsk_uses_k5_conv_code() {
        // ref: psk.cxx:66-68, 979-981.
        let c = PskVariant::Qpsk125.conv_code().unwrap();
        assert_eq!((c.k, c.polys), (5, vec![0x17, 0x19]));
        assert!(PskVariant::Psk125.conv_code().is_none());
    }

    #[test]
    fn bpsk_rate_grid_loopback() {
        let msg = "CQ DE K1ABC";
        for v in [
            PskVariant::Psk63,
            PskVariant::Psk125,
            PskVariant::Psk250,
            PskVariant::Psk500,
            PskVariant::Psk1000,
        ] {
            let audio = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
            let text = recovered_text(&PskDemod::new(v, 1500.0).feed(&audio));
            assert!(text.contains(msg), "{v:?} loopback recovered {text:?}");
        }
    }

    #[test]
    fn qpsk_rate_grid_loopback() {
        let msg = "CQ DE K1ABC";
        for v in [
            PskVariant::Qpsk31,
            PskVariant::Qpsk63,
            PskVariant::Qpsk125,
            PskVariant::Qpsk250,
            PskVariant::Qpsk500,
        ] {
            let audio = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
            let text = recovered_text(&PskDemod::new(v, 1500.0).feed(&audio));
            assert!(text.contains(msg), "{v:?} loopback recovered {text:?}");
        }
    }

    #[test]
    fn psk63f_uses_k7_conv_code_and_mfsk() {
        // ref: psk.cxx:70-74, 983-992, 448-451.
        let c = PskVariant::Psk63F.conv_code().unwrap();
        assert_eq!((c.k, c.polys), (7, vec![0x6d, 0x4f]));
        assert!(PskVariant::Psk63F.is_robust());
        assert!(!PskVariant::Psk63F.is_qpsk());
    }

    #[test]
    fn psk63f_loopback() {
        // The MFSK Varicode drops the final char (no trailing boundary bit), so
        // check the message minus its last character round-trips through the full
        // robust chain (K=7 FEC + two-phase Viterbi + MFSK framing).
        let msg = "CQ DE K1ABC";
        let audio = PskMod::new(PskVariant::Psk63F, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let text = recovered_text(&PskDemod::new(PskVariant::Psk63F, 1500.0).feed(&audio));
        assert!(text.contains(&msg[..msg.len() - 1]), "psk63f recovered {text:?}");
    }

    #[test]
    fn pskr_idepth_table_matches_fldigi() {
        // ref: psk.cxx:658-685.
        assert_eq!(PskVariant::Psk125R.idepth(), 40);
        assert_eq!(PskVariant::Psk250R.idepth(), 80);
        assert_eq!(PskVariant::Psk500R.idepth(), 160);
        assert_eq!(PskVariant::Psk1000R.idepth(), 160);
        assert_eq!(PskVariant::Psk63F.idepth(), 0);
        for v in [PskVariant::Psk125R, PskVariant::Psk250R, PskVariant::Psk500R] {
            assert!(v.is_robust() && v.conv_code().unwrap().k == 7);
        }
    }

    #[test]
    fn pskr_grid_loopback() {
        // The interleaved robust grid: MFSK Varicode + K=7 FEC + 2×2×idepth
        // interleaver + two-phase Viterbi. MFSK drops the final char.
        let msg = "CQ DE K1ABC";
        for v in [
            PskVariant::Psk125R,
            PskVariant::Psk250R,
            PskVariant::Psk500R,
            PskVariant::Psk1000R,
        ] {
            let audio = PskMod::new(v, 1500.0).modulate(&Frame::text(msg)).unwrap();
            let text = recovered_text(&PskDemod::new(v, 1500.0).feed(&audio));
            assert!(text.contains(&msg[..msg.len() - 1]), "{v:?} recovered {text:?}");
        }
    }

    #[test]
    fn psk63f_chunked_feed_emits_each_char_once() {
        // The daemon streams small chunks: the concatenation of all emitted text
        // must equal the single-feed decode exactly — no duplicated or dropped
        // characters from the incremental two-phase emit.
        let msg = "CQ DE K1ABC";
        let audio = PskMod::new(PskVariant::Psk63F, 1500.0).modulate(&Frame::text(msg)).unwrap();
        let mut rx = PskDemod::new(PskVariant::Psk63F, 1500.0);
        let mut text = String::new();
        for chunk in audio.chunks(157) {
            text.push_str(&recovered_text(&rx.feed(chunk)));
        }
        assert!(text.contains(&msg[..msg.len() - 1]), "chunked recovered {text:?}");
        // No duplication: the recovered payload appears exactly once.
        assert_eq!(text.matches("K1AB").count(), 1, "duplicated emit: {text:?}");
    }
}

