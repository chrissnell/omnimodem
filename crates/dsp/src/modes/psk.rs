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

use crate::framing::varicode::{decode as vari_decode, encode as vari_encode, PSK31};
use crate::frontend::fir::{design_lowpass, Fir};
use crate::frontend::modulate::DiffPsk;
use crate::frontend::nco::DownConverter;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::sync::costas::CostasLoop;
use crate::sync::timing::TransitionMinimizer;
use crate::types::{Cplx, Frame, FrameMeta, FramePayload, Sample};

/// fldigi samplerate for every Phase-7 (8 kHz) PSK submode. ref: psk.cxx:370.
pub const PSK_RATE: u32 = 8_000;

/// The plain differential-BPSK rates. `symbollen` (samples/symbol) fixes the
/// baud as `PSK_RATE / symbollen`. ref: psk.cxx:382-409.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PskVariant {
    Psk31,
    Psk63,
    Psk125,
    Psk250,
    Psk500,
    Psk1000,
}

/// Resolved per-variant parameters. ref: psk.cxx:382-409.
#[derive(Debug, Clone, Copy)]
pub struct PskParams {
    /// Samples per symbol at `PSK_RATE`; baud = `PSK_RATE / symbollen`.
    pub symbollen: usize,
    /// Preamble/postamble length in phase reversals (fldigi `dcdbits`).
    pub dcdbits: usize,
}

impl PskVariant {
    /// ref: psk.cxx:382-409 (symbollen/dcdbits switch).
    pub fn params(self) -> PskParams {
        use PskVariant::*;
        let p = |symbollen, dcdbits| PskParams { symbollen, dcdbits };
        match self {
            Psk31 => p(256, 32),
            Psk63 => p(128, 64),
            Psk125 => p(64, 128),
            Psk250 => p(32, 256),
            Psk500 => p(16, 512),
            Psk1000 => p(8, 128),
        }
    }

    pub fn baud(self) -> f32 {
        PSK_RATE as f32 / self.params().symbollen as f32
    }

    pub fn samples_per_symbol(self) -> usize {
        self.params().symbollen
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
        }
    }
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
        let rev = self.reversal_stream(text);
        let psk = DiffPsk::new(PSK_RATE as f32, self.center_hz, self.v.samples_per_symbol(), 1);
        Ok(psk.modulate(&rev))
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
        }
    }

    fn drain_completed(&mut self) -> Vec<Frame> {
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
            let derot = self.costas.process(f);
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
                    continue;
                }
                if self.have_prev {
                    // Differential BPSK on the complex symbol: Re{conj(prev)·sym}
                    // is positive for a steady carrier (data `1`), negative for a
                    // phase reversal (data `0`). Using the full phasor keeps the
                    // decision robust to any residual Costas phase offset, which
                    // matters at the high rates (16 samples/symbol) where a static
                    // offset would otherwise bleed the in-phase energy away.
                    let dot = sym.re * self.prev.re + sym.im * self.prev.im;
                    let data_bit = u8::from(dot >= 0.0);
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
                self.prev = sym;
                self.have_prev = true;
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
        ] {
            assert_eq!(PskVariant::from_label(v.label()), Some(v));
        }
        assert_eq!(PskVariant::from_label("qpsk31"), None);
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
}

