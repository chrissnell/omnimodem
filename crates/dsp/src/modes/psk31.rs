//! PSK31 mode assembly: differential BPSK + Varicode at 31.25 baud.
//!
//! TX: text → PSK31 Varicode bitstream → differential BPSK symbols → raised-
//! cosine DBPSK at `center_hz`. RX: Costas carrier recovery + Gardner timing →
//! differential decode → Varicode decode.

use crate::fec::gray::diff_bpsk_encode;
use crate::framing::varicode::{decode as vari_decode, encode as vari_encode, PSK31};
use crate::frontend::modulate::DiffPsk;
use crate::frontend::nco::DownConverter;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::sync::costas::CostasLoop;
use crate::sync::timing::GardnerTed;
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

pub const PSK31_RATE: u32 = 8_000;
pub const PSK31_BAUD: f32 = 31.25;

/// Leading idle reversals prepended on TX so the receiver's Costas and Gardner
/// loops lock before the payload starts.
const PREAMBLE_REVERSALS: usize = 64;

fn samples_per_symbol() -> usize {
    (PSK31_RATE as f32 / PSK31_BAUD).round() as usize // 256
}

pub struct Psk31Mod {
    center_hz: f32,
    sps: usize,
}

impl Psk31Mod {
    pub fn new(center_hz: f32) -> Self {
        Psk31Mod { center_hz, sps: samples_per_symbol() }
    }
}

impl Modulator for Psk31Mod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: PSK31_RATE,
            bandwidth_hz: 62.5,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let text = match &frame.payload {
            FramePayload::Text(t) => t,
            _ => return Err(ModError::UnsupportedPayload("psk31 needs text")),
        };
        // PSK31 idles on continuous phase reversals so the receiver's timing
        // loop has transitions to lock onto. `diff_bpsk_encode` flips polarity
        // on a `1`, so the idle preamble is a run of `1` bits, closed by the
        // `00` inter-character separator that delimits the first real codeword.
        let mut bits = vec![1u8; PREAMBLE_REVERSALS];
        bits.push(0);
        bits.push(0);
        bits.extend(vari_encode(&PSK31, text));
        let syms = diff_bpsk_encode(&bits);
        let sym_u32: Vec<u32> = syms.iter().map(|&b| b as u32).collect();
        let psk = DiffPsk::new(PSK31_RATE as f32, self.center_hz, self.sps, 1);
        Ok(psk.modulate(&sym_u32))
    }
}

/// Cap on the in-progress Varicode bit buffer. A real codeword is short and is
/// followed by a `00` separator; if this many bits accumulate with no boundary
/// the stream is noise, so the buffer is dropped rather than growing unbounded.
const MAX_PENDING_BITS: usize = 512;

/// Carrier-detect (squelch) parameters. The detector measures *carrier
/// presence*, not lock quality: per symbol it compares the coherent sum of the
/// de-rotated samples, |Σz|², against the total power, N·Σ|z|². A narrowband
/// tone adds coherently (ratio → 1) even if the Costas loop hasn't fully locked;
/// white noise adds incoherently (ratio ≈ 1/N). Keying on presence rather than
/// lock means a real-but-imperfect signal still decodes instead of going silent.
const CARRIER_EMA: f32 = 0.1;
const CARRIER_OPEN: f32 = 0.04;
/// Absolute power floor so true digital silence reads as "no carrier".
const CARRIER_FLOOR: f32 = 1e-6;

pub struct Psk31Demod {
    center_hz: f32,
    nco: DownConverter,
    costas: CostasLoop,
    gardner: GardnerTed,
    // Integrate-and-dump accumulators over the current symbol (matched filter):
    // I carries the data, Q is used only for carrier detection.
    acc_i: f32,
    acc_q: f32,
    // Total de-rotated power and sample count over the current symbol, for the
    // carrier-presence squelch (coherent sum vs. total power).
    acc_pow: f32,
    nsym: u32,
    prev_i: f32,
    have_prev: bool,
    /// Smoothed coherent-power ratio (|Σz|² / N·Σ|z|²): ≈1 for a tone, ≈1/N for
    /// noise. The squelch opens above [`CARRIER_OPEN`].
    coh: f32,
    /// Previous reversal bit, for the incremental second differential layer.
    diff_prev: u8,
    /// Varicode data bits not yet resolved into a completed character; drained
    /// at each `00` separator so this stays bounded (no whole-stream re-decode).
    pending: Vec<u8>,
    sample_index: u64,
}

impl Psk31Demod {
    pub fn new(center_hz: f32) -> Self {
        let rate = PSK31_RATE as f32;
        Psk31Demod {
            center_hz,
            nco: DownConverter::new(center_hz, rate),
            // Narrow loop bandwidth for the slow 31.25 baud carrier.
            costas: CostasLoop::new(0.01, 0.02),
            gardner: GardnerTed::new(rate / PSK31_BAUD),
            acc_i: 0.0,
            acc_q: 0.0,
            acc_pow: 0.0,
            nsym: 0,
            prev_i: 0.0,
            have_prev: false,
            coh: 0.0,
            diff_prev: 0,
            pending: Vec::new(),
            sample_index: 0,
        }
    }

    /// Emit characters completed since the last call. Decodes only up to the
    /// last `00` separator and drains those bits, so each character is decoded
    /// and emitted exactly once and `pending` never holds more than the current
    /// in-progress codeword.
    fn drain_completed(&mut self) -> Vec<Frame> {
        // Find the last character boundary (a `00` pair).
        let last_sep = (1..self.pending.len())
            .rev()
            .find(|&i| self.pending[i] == 0 && self.pending[i - 1] == 0);
        let Some(idx) = last_sep else {
            if self.pending.len() > MAX_PENDING_BITS {
                self.pending.clear(); // runaway noise, no boundary in sight
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
                decoder: Some("psk31".into()),
                ..Default::default()
            },
        }]
    }
}

impl Demodulator for Psk31Demod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: PSK31_RATE,
            bandwidth_hz: 62.5,
            tx: false,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        for &x in samples {
            self.sample_index += 1;
            let bb = self.nco.push(x);
            let derot = self.costas.process(bb);
            // Integrate the de-rotated I/Q across the symbol for matched-filter
            // SNR gain; Gardner still times off the instantaneous sample.
            self.acc_i += derot.re;
            self.acc_q += derot.im;
            self.acc_pow += derot.re * derot.re + derot.im * derot.im;
            self.nsym += 1;
            if self.gardner.feed(derot.re).is_some() {
                let sym = self.acc_i;
                let q = self.acc_q;
                let coherent = sym * sym + q * q;
                let total = self.nsym as f32 * self.acc_pow;
                let pow = self.acc_pow;
                self.acc_i = 0.0;
                self.acc_q = 0.0;
                self.acc_pow = 0.0;
                self.nsym = 0;
                // Carrier-presence detect: a coherent tone makes |Σz|² approach
                // the total power N·Σ|z|² (ratio → 1); noise stays near 1/N. This
                // keys on a signal being present, not on a clean lock, so a real
                // but imperfectly-locked carrier still decodes. Below threshold,
                // drop any partial codeword and resync the differential decoder.
                let ratio = if total > 0.0 { coherent / total } else { 0.0 };
                self.coh += CARRIER_EMA * (ratio - self.coh);
                if pow < CARRIER_FLOOR || self.coh < CARRIER_OPEN {
                    self.pending.clear();
                    self.have_prev = false;
                    self.prev_i = 0.0;
                    continue;
                }
                // Differential BPSK symbol = phase reversal vs. the previous
                // symbol. `diff_bpsk_encode` flips polarity on a data `1`, so a
                // reversal (sign change) decodes to symbol `1`.
                if self.have_prev {
                    let reversal = u8::from(sym * self.prev_i < 0.0);
                    // Second differential layer, applied incrementally (the
                    // streaming equivalent of `diff_bpsk_decode`): data bit =
                    // reversal XOR previous reversal.
                    let data_bit = reversal ^ self.diff_prev;
                    self.diff_prev = reversal;
                    self.pending.push(data_bit);
                }
                self.prev_i = sym;
                self.have_prev = true;
            }
        }
        // Emit any characters whose `00` separator has now arrived.
        self.drain_completed()
    }

    fn reset(&mut self) {
        *self = Psk31Demod::new(self.center_hz);
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

    #[test]
    fn modulates_text_to_audio() {
        let mut m = Psk31Mod::new(1000.0);
        assert!(m.caps().tx);
        let s = m.modulate(&Frame::text("CQ")).unwrap();
        assert!(s.len() > m.sps * 8, "too few samples: {}", s.len());
    }

    #[test]
    fn rejects_packet_payload() {
        let mut m = Psk31Mod::new(1000.0);
        assert!(matches!(
            m.modulate(&Frame::packet(vec![1, 2])).unwrap_err(),
            ModError::UnsupportedPayload(_)
        ));
    }

    #[test]
    fn loopback_recovers_text() {
        let msg = "CQ DE K1ABC";
        let mut tx = Psk31Mod::new(1000.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rx = Psk31Demod::new(1000.0);
        let frames = rx.feed(&samples);
        let text = recovered_text(&frames);
        assert!(text.contains(msg), "recovered: {text:?}");
    }

    #[test]
    fn loopback_recovers_text_light_awgn() {
        let msg = "CQ DE K1ABC";
        let mut tx = Psk31Mod::new(1000.0);
        let mut samples = tx.modulate(&Frame::text(msg)).unwrap();
        let mut rng = crate::testutil::Rng::new(0x51A1);
        crate::testutil::add_awgn(&mut samples, 0.02, &mut rng);
        let mut rx = Psk31Demod::new(1000.0);
        let frames = rx.feed(&samples);
        let text = recovered_text(&frames);
        assert!(text.contains(msg), "recovered: {text:?}");
    }

    #[test]
    fn noise_does_not_decode() {
        // Pure white noise carries no PSK31 carrier; the squelch must keep it from
        // dribbling out characters off the noise floor.
        let mut rng = crate::testutil::Rng::new(0xC0FFEE);
        let mut noise = vec![0.0f32; PSK31_RATE as usize * 2]; // 2 s
        crate::testutil::add_awgn(&mut noise, 0.3, &mut rng);
        let mut rx = Psk31Demod::new(1000.0);
        let text = recovered_text(&rx.feed(&noise));
        assert!(text.len() <= 2, "noise should stay squelched, decoded {text:?}");
    }

    #[test]
    fn chunked_feed_emits_each_char_once_and_bounds_memory() {
        // The daemon RX worker streams ~20 ms chunks, not the whole signal. The
        // concatenation of all emitted frames must equal the message exactly —
        // no duplicate/partial frames — and `pending` must stay bounded.
        let msg = "CQ DE K1ABC";
        let mut tx = Psk31Mod::new(1000.0);
        let samples = tx.modulate(&Frame::text(msg)).unwrap();

        let mut rx = Psk31Demod::new(1000.0);
        let mut text = String::new();
        let mut max_pending = 0;
        for chunk in samples.chunks(160) {
            for f in rx.feed(chunk) {
                if let FramePayload::Text(t) = &f.payload {
                    text.push_str(t);
                }
            }
            max_pending = max_pending.max(rx.pending.len());
        }
        assert!(text.contains(msg), "recovered {text:?}");
        // No runaway accumulation: the pending buffer never holds more than a
        // couple of codewords' worth of bits.
        assert!(max_pending < MAX_PENDING_BITS, "pending grew to {max_pending}");
    }
}
