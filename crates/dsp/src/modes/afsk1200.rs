//! AFSK 1200 (AX.25) mode assembly: Bell-202 AFSK ⇄ HDLC/AX.25.
//!
//! TX: Frame payload bytes are an *intact* AX.25 frame → HDLC-frame (flag,
//! bit-stuff, FCS) → NRZI → Bell-202 1200/2200 Hz AFSK at 48 kHz.
//! RX: per-tone correlators (bandpass → rectify → lowpass) feed a mark/space
//! decision, a DPLL recovers the bit clock, NRZI decode + HDLC deframe
//! (FCS-validated) emit packets. The ensemble runs N slicer thresholds in
//! parallel and dedups their frames — the Graywolf "hydra".

use crate::ensemble::DedupWindow;
use crate::fec::nrzi::{nrzi_decode, nrzi_encode};
use crate::fec::slicer::MultiSlicer;
use crate::frontend::fir::{design_bandpass, design_lowpass, Fir};
use crate::frontend::modulate::Afsk;
use crate::framing::hdlc::{hdlc_deframe, hdlc_frame};
use crate::mode::{DemodShape, Demodulator, Duplex, ModError, ModeCaps, Modulator};
use crate::sync::dpll::DpllClockRecovery;
use crate::types::{Frame, FrameMeta, FramePayload, Sample};

/// Bell-202 native rate. 48 kHz is the AFSK working rate.
pub const AFSK1200_RATE: u32 = 48_000;

const MARK_HZ: f32 = 1200.0;
const SPACE_HZ: f32 = 2200.0;
const BAUD: f32 = 1200.0;
/// Tone bandpass half-width; ±500 Hz isolates a Bell-202 tone while passing the
/// FSK sidebands.
const TONE_BW: f32 = 500.0;
const BP_TAPS: usize = 96;
const LP_TAPS: usize = 64;
/// Envelope lowpass cutoff (~baud) smooths the rectified tone to its magnitude.
const ENV_CUTOFF: f32 = 1200.0;
/// Number of leading HDLC flags transmitted to train the receiver.
const PREAMBLE_FLAGS: usize = 24;
/// Trailing flags so the closing flag is fully captured even if the RX clock
/// recovery swallows a symbol or two while acquiring.
const POSTAMBLE_FLAGS: usize = 4;

// ---------------------------------------------------------------------------
// Modulator
// ---------------------------------------------------------------------------

pub struct Afsk1200Mod {
    afsk: Afsk,
}

impl Afsk1200Mod {
    pub fn new() -> Self {
        Afsk1200Mod { afsk: Afsk::bell202(AFSK1200_RATE as f32) }
    }
}

impl Default for Afsk1200Mod {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator for Afsk1200Mod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: AFSK1200_RATE,
            bandwidth_hz: 2400.0,
            tx: true,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn modulate(&mut self, frame: &Frame) -> Result<Vec<Sample>, ModError> {
        let payload = match &frame.payload {
            FramePayload::Packet(b) => b,
            other => return Err(ModError::UnsupportedPayload(payload_kind(other))),
        };
        // HDLC frame → bit vector (LSB-first) → NRZI line levels → AFSK. A short
        // preamble of flags lets the RX front end and DPLL settle before data.
        let mut bits = preamble_bits(PREAMBLE_FLAGS);
        bits.extend_from_slice(&hdlc_frame(payload));
        bits.extend_from_slice(&preamble_bits(POSTAMBLE_FLAGS));
        let levels = nrzi_encode(&bits);
        let level_bools: Vec<bool> = levels.iter().map(|&b| b != 0).collect();
        Ok(self.afsk.modulate(&level_bools))
    }
}

/// LSB-first bits for `n` repeated `0x7E` flags.
fn preamble_bits(n: usize) -> Vec<u8> {
    let mut bits = Vec::with_capacity(n * 8);
    for _ in 0..n {
        for i in 0..8 {
            bits.push((0x7Eu8 >> i) & 1);
        }
    }
    bits
}

fn payload_kind(p: &FramePayload) -> &'static str {
    match p {
        FramePayload::Packet(_) => "packet",
        FramePayload::Text(_) => "text",
        FramePayload::Message77(_) => "message77",
        FramePayload::Vocoder(_) => "vocoder",
    }
}

// ---------------------------------------------------------------------------
// Front end: per-tone magnitude correlator
// ---------------------------------------------------------------------------

/// One tone-energy correlator: bandpass → rectify → lowpass-smooth, yielding a
/// running magnitude estimate of the tone.
struct ToneEnergy {
    bp: Fir,
    lp: Fir,
}

impl ToneEnergy {
    fn new(center: f32, rate: f32) -> Self {
        let bp = Fir::new(design_bandpass(BP_TAPS, center - TONE_BW, center + TONE_BW, rate));
        let lp = Fir::new(design_lowpass(LP_TAPS, ENV_CUTOFF, rate));
        ToneEnergy { bp, lp }
    }

    fn push(&mut self, x: Sample) -> f32 {
        let band = self.bp.push(x);
        self.lp.push(band.abs())
    }

    fn reset(&mut self) {
        self.bp.reset();
        self.lp.reset();
    }
}

/// Shared mark/space front end driven once per sample.
struct FrontEnd {
    mark: ToneEnergy,
    space: ToneEnergy,
}

impl FrontEnd {
    fn new(rate: f32) -> Self {
        FrontEnd { mark: ToneEnergy::new(MARK_HZ, rate), space: ToneEnergy::new(SPACE_HZ, rate) }
    }

    fn push(&mut self, x: Sample) -> (f32, f32) {
        (self.mark.push(x), self.space.push(x))
    }

    fn reset(&mut self) {
        self.mark.reset();
        self.space.reset();
    }
}

// ---------------------------------------------------------------------------
// Bit assembler: DPLL + NRZI + HDLC
// ---------------------------------------------------------------------------

/// Largest live level buffer before the oldest levels are dropped.
const MAX_LEVELS: usize = 4096;

/// Accumulates sampled NRZI line levels through the DPLL and drains them through
/// NRZI + HDLC, emitting any FCS-valid payload bytes.
struct BitAssembler {
    dpll: DpllClockRecovery,
    /// NRZI line levels sampled at each bit instant.
    levels: Vec<u8>,
}

impl BitAssembler {
    fn new(rate: f32) -> Self {
        BitAssembler { dpll: DpllClockRecovery::new(rate / BAUD), levels: Vec::with_capacity(4096) }
    }

    /// Feed one mark/space decision (`true` = mark). Returns any payloads that
    /// completed on this sample.
    fn push(&mut self, mark_wins: bool) -> Vec<Vec<u8>> {
        let Some(level) = self.dpll.feed(mark_wins) else {
            return Vec::new();
        };
        self.levels.push(u8::from(level));
        // A frame can only close on a flag, i.e. once another byte of levels has
        // arrived. Deframe the whole live buffer and clear it on any valid FCS.
        if !self.levels.len().is_multiple_of(8) {
            return Vec::new();
        }
        let bits = nrzi_decode(&self.levels);
        let frames = hdlc_deframe(&bits);
        if !frames.is_empty() {
            self.levels.clear();
            return frames;
        }
        // Bound memory on a continuous stream with no valid frame.
        if self.levels.len() > MAX_LEVELS {
            let drop = self.levels.len() - MAX_LEVELS / 2;
            self.levels.drain(0..drop);
        }
        Vec::new()
    }

    fn reset(&mut self, rate: f32) {
        self.dpll = DpllClockRecovery::new(rate / BAUD);
        self.levels.clear();
    }
}

fn packet_frame(payload: Vec<u8>, offset: u64, decoder: &str) -> Frame {
    Frame {
        payload: FramePayload::Packet(payload),
        meta: FrameMeta {
            crc_ok: true,
            sample_offset: offset,
            decoder: Some(decoder.to_string()),
            ..Default::default()
        },
    }
}

// ---------------------------------------------------------------------------
// Single-slicer streaming demodulator
// ---------------------------------------------------------------------------

pub struct Afsk1200Demod {
    front: FrontEnd,
    bits: BitAssembler,
    sample_index: u64,
}

impl Afsk1200Demod {
    /// Single-slicer demod (one unity decision threshold). The ensemble variant
    /// is [`Afsk1200Demod::ensemble`].
    pub fn single() -> Self {
        let rate = AFSK1200_RATE as f32;
        Afsk1200Demod { front: FrontEnd::new(rate), bits: BitAssembler::new(rate), sample_index: 0 }
    }

    /// Build the multi-slicer ensemble with `n` slicers (use an odd `n`; 9 is
    /// the Graywolf default). Returns a [`Demodulator`].
    pub fn ensemble(n: usize) -> Afsk1200Ensemble {
        Afsk1200Ensemble::new(n)
    }
}

impl Demodulator for Afsk1200Demod {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: AFSK1200_RATE,
            bandwidth_hz: 2400.0,
            tx: false,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        let mut out = Vec::new();
        for &x in samples {
            self.sample_index += 1;
            let (m, s) = self.front.push(x);
            for p in self.bits.push(m > s) {
                out.push(packet_frame(p, self.sample_index, "afsk1200/single"));
            }
        }
        out
    }

    fn reset(&mut self) {
        let rate = AFSK1200_RATE as f32;
        self.front.reset();
        self.bits.reset(rate);
        self.sample_index = 0;
    }
}

// ---------------------------------------------------------------------------
// Multi-slicer ensemble ("hydra")
// ---------------------------------------------------------------------------

/// N independent slicer lanes sharing one front end, deduped by content+offset.
pub struct Afsk1200Ensemble {
    front: FrontEnd,
    slicer: MultiSlicer,
    lanes: Vec<BitAssembler>,
    dedup: DedupWindow,
    sample_index: u64,
}

impl Afsk1200Ensemble {
    fn new(n: usize) -> Self {
        let rate = AFSK1200_RATE as f32;
        Afsk1200Ensemble {
            front: FrontEnd::new(rate),
            slicer: MultiSlicer::new(n),
            lanes: (0..n).map(|_| BitAssembler::new(rate)).collect(),
            // ~3 symbol-times dedup window.
            dedup: DedupWindow::new((3.0 * rate / BAUD) as u64),
            sample_index: 0,
        }
    }
}

impl Demodulator for Afsk1200Ensemble {
    fn caps(&self) -> ModeCaps {
        ModeCaps {
            native_rate: AFSK1200_RATE,
            bandwidth_hz: 2400.0,
            tx: false,
            duplex: Duplex::Half,
            shape: DemodShape::Streaming,
        }
    }

    fn feed(&mut self, samples: &[Sample]) -> Vec<Frame> {
        let mut out = Vec::new();
        for &x in samples {
            self.sample_index += 1;
            let (m, s) = self.front.push(x);
            let decisions = self.slicer.slice(m, s);
            for (lane, &mark_wins) in self.lanes.iter_mut().zip(decisions.iter()) {
                for p in lane.push(mark_wins) {
                    let frame = packet_frame(p, self.sample_index, "afsk1200/hydra");
                    if self.dedup.admit(&frame) {
                        out.push(frame);
                    }
                }
            }
        }
        self.dedup.prune_to_latest();
        out
    }

    fn reset(&mut self) {
        let rate = AFSK1200_RATE as f32;
        self.front.reset();
        for lane in &mut self.lanes {
            lane.reset(rate);
        }
        self.dedup.clear();
        self.sample_index = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::ax25::{Address, Ax25Frame};
    use crate::testutil::{add_awgn, Rng};

    fn sample_frame() -> Ax25Frame {
        Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("K1ABC", 7),
            digipeaters: vec![],
            info: b"!4903.50N/07201.75W-Test".to_vec(),
        }
    }

    #[test]
    fn modulator_caps_streaming_tx_and_frame_is_long() {
        let mut m = Afsk1200Mod::new();
        let caps = m.caps();
        assert_eq!(caps.native_rate, 48_000);
        assert!(caps.tx);
        assert!(matches!(caps.shape, DemodShape::Streaming));
        let frame = Frame::packet(b"K1ABC>APRS:hi".to_vec());
        let samples = m.modulate(&frame).unwrap();
        assert!(samples.len() > 1000, "got {} samples", samples.len());
    }

    #[test]
    fn modulator_rejects_text_payload() {
        let mut m = Afsk1200Mod::new();
        let err = m.modulate(&Frame::text("nope")).unwrap_err();
        assert!(matches!(err, ModError::UnsupportedPayload("text")));
    }

    #[test]
    fn loopback_recovers_ax25_frame() {
        let f = sample_frame();
        let frame = Frame::packet(f.encode());

        let mut tx = Afsk1200Mod::new();
        let samples = tx.modulate(&frame).unwrap();

        let mut rx = Afsk1200Demod::single();
        let frames = rx.feed(&samples);
        assert!(!frames.is_empty(), "demod produced no frames");
        let got = match &frames[0].payload {
            FramePayload::Packet(b) => b.clone(),
            other => panic!("expected packet, got {other:?}"),
        };
        assert_eq!(got, f.encode());
        assert!(frames[0].meta.crc_ok);
    }

    #[test]
    fn ensemble_decodes() {
        let f = Ax25Frame {
            dest: Address::new("APRS", 0),
            source: Address::new("N0CALL", 1),
            digipeaters: vec![],
            info: b"ensemble test".to_vec(),
        };
        let frame = Frame::packet(f.encode());
        let mut tx = Afsk1200Mod::new();
        let mut samples = tx.modulate(&frame).unwrap();
        let mut rng = Rng::new(20260620);
        add_awgn(&mut samples, 0.12, &mut rng);

        let mut rx = Afsk1200Demod::ensemble(9);
        let frames = rx.feed(&samples);
        assert!(
            frames
                .iter()
                .any(|fr| matches!(&fr.payload, FramePayload::Packet(b) if b == &f.encode())),
            "ensemble failed to recover the exact frame"
        );
    }
}
