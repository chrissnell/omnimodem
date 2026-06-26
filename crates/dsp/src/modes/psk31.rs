//! PSK31 mode assembly: differential BPSK + Varicode at 31.25 baud.
//!
//! TX: text → PSK31 Varicode bitstream → differential BPSK symbols → raised-
//! cosine DBPSK at `center_hz`. RX follows fldigi's BPSK31 receiver: a two-stage
//! `pskcore` matched filter, envelope-asymmetry symbol-timing recovery, and
//! differential phase detection → Varicode decode.

use crate::framing::varicode::{decode as vari_decode, encode as vari_encode, PSK31};
use crate::frontend::fir::{design_lowpass, Fir};
use crate::frontend::modulate::DiffPsk;
use crate::frontend::nco::DownConverter;
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::types::{Cplx, Frame, FrameMeta, FramePayload, Sample};
use std::f32::consts::{PI, TAU};

pub const PSK31_RATE: u32 = 8_000;
pub const PSK31_BAUD: f32 = 31.25;

/// Leading idle reversals prepended on TX so the receiver's timing and matched
/// filter settle before the payload starts.
const PREAMBLE_REVERSALS: usize = 64;
/// Trailing idle reversals so the receiver's matched-filter group delay can
/// flush the final character (real PSK31 sends a postamble before unkeying).
const POSTAMBLE_REVERSALS: usize = 16;

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
        // Standard PSK31: data rides on phase *reversals* — a Varicode `0` is a
        // reversal, a `1` is a steady carrier — and the line idles on continuous
        // reversals so the receiver's timing loop has transitions to lock onto.
        // `DiffPsk::modulate` differentially encodes its input (running phase
        // sum), so feed it the reversal stream directly — one differential layer,
        // matching what other PSK31 stacks (fldigi, …) send. (1 = reversal: idle
        // reversals, the `00` separator as two reversals, then payload bit b →
        // reversal `1 ^ b`.)
        let mut rev = vec![1u32; PREAMBLE_REVERSALS];
        rev.push(1);
        rev.push(1);
        rev.extend(vari_encode(&PSK31, text).into_iter().map(|b| (1 ^ b) as u32));
        rev.extend(std::iter::repeat_n(1u32, POSTAMBLE_REVERSALS));
        let psk = DiffPsk::new(PSK31_RATE as f32, self.center_hz, self.sps, 1);
        Ok(psk.modulate(&rev))
    }
}

/// Cap on the in-progress Varicode bit buffer. A real codeword is short and is
/// followed by a `00` separator; if this many bits accumulate with no boundary
/// the stream is noise, so the buffer is dropped rather than growing unbounded.
const MAX_PENDING_BITS: usize = 512;

/// Carrier squelch: smoothed in-band / total power ratio (same idea as the RTTY
/// demod). A narrowband PSK31 signal — including the continuous-reversal idle,
/// whose energy sits at carrier ±15.6 Hz — survives a lowpass a little wider
/// than the ~31 Hz occupied band, so most of its power passes (ratio high);
/// white noise spread across the whole band is mostly rejected (ratio low).
/// This keys on signal *presence*, not Costas lock, and (unlike a DC matched
/// filter) does not mistake the reversal idle for silence.
const SQUELCH_CUTOFF_HZ: f32 = 80.0;
const SQUELCH_TAPS: usize = 127;
const CARRIER_EMA: f32 = 0.002;
const CARRIER_OPEN: f32 = 0.15;
/// Absolute power floor so true digital silence reads as "no carrier".
const CARRIER_FLOOR: f32 = 1e-9;

/// Decimation from the 8 kHz audio rate to 16 samples/symbol (500 Hz), matching
/// fldigi's `symbollen/16` for PSK31. Timing recovery runs at this rate.
const DECIMATE: usize = 16;
/// Sub-symbol timing-recovery resolution (`bitsteps` in fldigi).
const BITSTEPS: usize = 16;

/// fldigi's `pskcore_filter`: a symmetric 65-tap windowed-sinc matched filter for
/// 31.25-baud PSK. Used twice — as the anti-alias filter before the ÷16
/// decimation (where it is wideband relative to 8 kHz) and again at 500 Hz
/// (where the same taps are narrowband, ~one symbol wide: the matched filter).
/// Ported verbatim from fldigi `src/psk/pskcoeff.cxx` (double literals kept as
/// written; they round to the nearest f32).
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const PSKCORE_FILTER: [f32; 65] = [
    4.3453566e-005, -0.00049122414, -0.00078771292, -0.0013507826, -0.0021287814,
    -0.003133466, -0.004366817, -0.0058112187, -0.0074249976, -0.0091398882,
    -0.010860157, -0.012464086, -0.013807772, -0.014731191, -0.015067057,
    -0.014650894, -0.013333425, -0.01099166, -0.0075431246, -0.0029527849,
    0.0027546292, 0.0094932775, 0.017113308, 0.025403511, 0.034099681,
    0.042895839, 0.051458575, 0.059444853, 0.066521003, 0.072381617,
    0.076767694, 0.079481619, 0.080420311, 0.079481619, 0.076767694,
    0.072381617, 0.066521003, 0.059444853, 0.051458575, 0.042895839,
    0.034099681, 0.025403511, 0.017113308, 0.0094932775, 0.0027546292,
    -0.0029527849, -0.0075431246, -0.01099166, -0.013333425, -0.014650894,
    -0.015067057, -0.014731191, -0.013807772, -0.012464086, -0.010860157,
    -0.0091398882, -0.0074249976, -0.0058112187, -0.004366817, -0.003133466,
    -0.0021287814, -0.0013507826, -0.00078771292, -0.00049122414, 4.3453566e-005,
];

pub struct Psk31Demod {
    center_hz: f32,
    nco: DownConverter,
    // fldigi's two-stage matched filter on the complex baseband: fir1 ahead of
    // the ÷16 decimation, fir2 at 500 Hz (the narrowband symbol matched filter).
    // Each is a real FIR applied to I and Q.
    fir1_i: Fir,
    fir1_q: Fir,
    fir2_i: Fir,
    fir2_q: Fir,
    decim: usize,
    // Envelope-asymmetry symbol-timing recovery (fldigi `syncbuf`/`bitclk`): one
    // smoothed symbol-magnitude waveform over BITSTEPS sub-samples; the lower vs.
    // upper half asymmetry nudges the sampling instant.
    syncbuf: [f32; BITSTEPS],
    bitclk: f32,
    // Previous symbol for differential phase detection.
    prevsym: Cplx,
    have_prev: bool,
    // Once the squelch opens we join the bit stream mid-symbol; `synced` gates
    // decoding until a `00` character boundary appears, so the leading partial
    // codeword (and acquisition transient) is discarded instead of emitted.
    synced: bool,
    zrun: u8,
    // Carrier squelch: a lowpass on the complex baseband plus smoothed in-band
    // and total power; their ratio tells a PSK31 signal from the noise floor.
    lpf_i: Fir,
    lpf_q: Fir,
    p_in: f32,
    p_tot: f32,
    /// Varicode data bits not yet resolved into a completed character; drained
    /// at each `00` separator so this stays bounded (no whole-stream re-decode).
    pending: Vec<u8>,
    sample_index: u64,
}

impl Psk31Demod {
    pub fn new(center_hz: f32) -> Self {
        let rate = PSK31_RATE as f32;
        let taps = design_lowpass(SQUELCH_TAPS, SQUELCH_CUTOFF_HZ, rate);
        let mf = || Fir::new(PSKCORE_FILTER.to_vec());
        Psk31Demod {
            center_hz,
            nco: DownConverter::new(center_hz, rate),
            fir1_i: mf(),
            fir1_q: mf(),
            fir2_i: mf(),
            fir2_q: mf(),
            decim: 0,
            syncbuf: [0.0; BITSTEPS],
            bitclk: 0.0,
            prevsym: Cplx::new(0.0, 0.0),
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
            // Carrier squelch: lowpass the raw baseband and track the in-band vs.
            // total power. The lowpass is wide enough to pass the PSK31 band
            // (including the ±15.6 Hz reversal idle), so the gate stays open on a
            // real signal but closes on the broadband noise floor.
            let f = Cplx::new(self.lpf_i.push(bb.re), self.lpf_q.push(bb.im));
            self.p_in += CARRIER_EMA * (f.norm_sqr() - self.p_in);
            self.p_tot += CARRIER_EMA * (bb.norm_sqr() - self.p_tot);

            // fir1: matched filter ahead of the ÷16 decimation. Run it on every
            // sample to keep its delay line current; use the output only at the
            // decimation instants.
            let z1 = Cplx::new(self.fir1_i.push(bb.re), self.fir1_q.push(bb.im));
            self.decim += 1;
            if self.decim < DECIMATE {
                continue;
            }
            self.decim = 0;
            // fir2 at 500 Hz: the narrowband symbol matched filter.
            let z2 = Cplx::new(self.fir2_i.push(z1.re), self.fir2_q.push(z1.im));

            // Envelope-asymmetry timing recovery (fldigi). Draw the symbol's
            // magnitude waveform into `syncbuf` indexed by the sub-symbol clock,
            // then steer `bitclk` by the lower/upper-half asymmetry.
            let idx = (self.bitclk as usize).min(BITSTEPS - 1);
            self.syncbuf[idx] = 0.8 * self.syncbuf[idx] + 0.2 * z2.norm();
            let (mut sum, mut ampsum) = (0.0f32, 0.0f32);
            for i in 0..BITSTEPS / 2 {
                sum += self.syncbuf[i] - self.syncbuf[i + BITSTEPS / 2];
                ampsum += self.syncbuf[i] + self.syncbuf[i + BITSTEPS / 2];
            }
            let err = if ampsum == 0.0 { 0.0 } else { sum / ampsum };
            self.bitclk -= err / 5.0;
            self.bitclk += 1.0;
            if self.bitclk < 0.0 {
                self.bitclk += BITSTEPS as f32;
            }
            if self.bitclk < BITSTEPS as f32 {
                continue;
            }
            // A full symbol has been drawn: `z2` is the symbol decision point.
            self.bitclk -= BITSTEPS as f32;

            let open = self.p_tot > CARRIER_FLOOR && self.p_in > CARRIER_OPEN * self.p_tot;
            if !open {
                self.pending.clear();
                self.have_prev = false;
                self.synced = false;
                self.zrun = 0;
                self.prevsym = Cplx::new(0.0, 0.0);
                continue;
            }
            // Differential phase between consecutive symbols. A phase near π is a
            // reversal (Varicode data `0`); near 0 a steady carrier (`1`).
            if self.have_prev {
                let mut phase = (self.prevsym.conj() * z2).arg();
                if phase < 0.0 {
                    phase += TAU;
                }
                let reversal = (((phase / PI + 0.5) as i32) & 1) << 1; // {0, 2}
                let data_bit = u8::from(reversal == 0);
                if self.synced {
                    self.pending.push(data_bit);
                } else if data_bit == 0 {
                    // Wait for a `00` boundary before decoding real characters.
                    self.zrun += 1;
                    if self.zrun >= 2 {
                        self.synced = true;
                    }
                } else {
                    self.zrun = 0;
                }
            }
            self.prevsym = z2;
            self.have_prev = true;
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
