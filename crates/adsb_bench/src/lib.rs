//! adsb_bench decode core — the offline frame-yield path shared by the CLI
//! (`main.rs`) and the CI regression gate (`tests/regression_gate.rs`).
//!
//! [`decode_iq`] runs a raw uint8 interleaved I/Q buffer through the same DSP
//! path the daemon uses — resample the 2.4 Msps capture to the 2 MHz working
//! rate, take the `|I+jQ|` magnitude envelope, feed [`AdsbDemod`] — and returns
//! a [`Report`] tallying the decoder's usable yield. Keeping the decode here
//! (not in the binary) lets a test assert the same numbers the CLI prints.
//!
//! `--front` (see [`Front`]) selects where the magnitude is taken relative to
//! the 2.4M→2.0M decimation; the CLI defaults to the R1 `complex` front end,
//! while the CI gate feeds a clip already at the working rate so the resample is
//! a no-op.

use std::collections::BTreeMap;

use omnimodem_dsp::frontend::iq::u8_iq_to_cplx;
use omnimodem_dsp::frontend::resample::{ComplexResampler, Resampler};
use omnimodem_dsp::mode::Demodulator;
use omnimodem_dsp::modes::adsb::{AdsbDemod, ModeS, ADSB_RATE};
use omnimodem_dsp::types::{Cplx, Frame, FramePayload, Sample};

/// Default slicer-phase count — re-exported so the CLI default and the CI gate
/// can name the shipping decoder's ensemble width (see [`decode_iq`]).
pub use omnimodem_dsp::modes::adsb::ADSB_SLICER_PHASES as DEFAULT_PHASES;

/// Default R4 soft-decision reject threshold — re-exported so the CLI default and
/// the CI gate match the shipping decoder's gate (see [`decode_iq`]).
pub use omnimodem_dsp::modes::adsb::ADSB_MIN_CONFIDENCE as DEFAULT_MIN_CONF;

/// Default capture rate — the wideband rate the daemon commands the dongle to
/// (`ADSB_CAPTURE_RATE` in the RTL-SDR front end).
pub const DEFAULT_IN_RATE: u32 = 2_400_000;

/// How the 2.4 Msps capture becomes the 2 MHz magnitude envelope. See the
/// `main.rs` module doc for the DSP rationale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Front {
    /// R1: complex-decimate the I/Q 2.4M→2.0M, then take magnitude.
    Complex,
    /// R0: take magnitude at the capture rate, then decimate the envelope.
    Mag,
}

impl Front {
    /// Human-readable label for the front end, printed by the CLI.
    pub fn label(self) -> &'static str {
        match self {
            Front::Complex => "complex (resample I/Q → magnitude)",
            Front::Mag => "mag (magnitude → resample envelope)",
        }
    }
}

/// The stateful resampler for the selected front end. Only the chosen variant is
/// constructed, and it persists across chunk windows so decimation phase and
/// filter history carry over.
enum FrontEnd {
    Complex(ComplexResampler),
    Mag(Resampler),
}

impl FrontEnd {
    fn new(front: Front, in_rate: u32) -> Self {
        match front {
            Front::Complex => FrontEnd::Complex(ComplexResampler::new(in_rate, ADSB_RATE, 16)),
            Front::Mag => FrontEnd::Mag(Resampler::new(in_rate, ADSB_RATE, 16)),
        }
    }

    /// Turn one window of capture-rate I/Q into the 2 MHz magnitude envelope.
    fn envelope(&mut self, chunk: &[Cplx]) -> Vec<Sample> {
        match self {
            // R1: band-limited complex decimation first, magnitude at 2 MHz.
            FrontEnd::Complex(rs) => rs.process(chunk).iter().map(|c| c.norm()).collect(),
            // R0: magnitude at the capture rate, then decimate the envelope.
            FrontEnd::Mag(rs) => {
                let mag: Vec<Sample> = chunk.iter().map(|c| c.norm()).collect();
                rs.process(&mag)
            }
        }
    }
}

/// Tally accumulated over every frame the demod emitted. `AdsbDemod` gates on
/// parity (`require_crc`), so every emitted frame is CRC-valid — this counts the
/// decoder's usable yield, the number each R-phase is trying to move.
#[derive(Default)]
pub struct Report {
    /// CRC-valid frames emitted.
    pub frames_valid: u64,
    /// Sum of accepted-frame soft confidences (for the mean); `conf_min` is the
    /// weakest frame that cleared the R4 gate. Together they show the headroom the
    /// accepted set keeps above the reject threshold. `None` min if no frames.
    pub conf_sum: f64,
    pub conf_min: Option<f32>,
    /// Airborne-position frames (DF17/18, TC 9-18/20-22) that passed CRC.
    pub airborne_pos: u64,
    /// Distinct 24-bit ICAO addresses seen in CRC-valid DF11/17/18 frames → count.
    pub aircraft: BTreeMap<u32, u64>,
    /// DF → count, CRC-valid frames only.
    pub df_hist: BTreeMap<u8, u64>,
    /// DF17/18 type code → count, CRC-valid frames only.
    pub tc_hist: BTreeMap<u8, u64>,
    /// Complex I/Q samples read from the input.
    pub samples_in: usize,
    /// Working-rate samples produced by the resampler.
    pub samples_work: u64,
}

impl Report {
    /// Number of distinct aircraft (ICAO addresses) seen.
    pub fn unique_aircraft(&self) -> usize {
        self.aircraft.len()
    }

    /// Mean soft confidence over the accepted frames, or `None` if none.
    pub fn conf_mean(&self) -> Option<f64> {
        (self.frames_valid > 0).then(|| self.conf_sum / self.frames_valid as f64)
    }
}

/// Decode a raw uint8 interleaved I/Q buffer at `in_rate` and tally the yield.
///
/// Mirrors the daemon path: the `front` front end turns the capture into the
/// 2 MHz magnitude envelope (see [`Front`]), which is fed to the streaming
/// demod running `phases` sub-sample slicer phases (R3 ensemble;
/// [`DEFAULT_PHASES`] matches the shipping decoder, `1` reproduces the pre-R3
/// baseline). The resampler is stateful, so a single instance spans every
/// window; `AdsbDemod` buffers frames straddling a window boundary. Windowing
/// only bounds peak memory on long captures.
pub fn decode_iq(bytes: &[u8], in_rate: u32, front: Front, phases: usize, min_conf: f32) -> Report {
    decode_iq_with(bytes, in_rate, front, phases, min_conf, |_| {})
}

/// Like [`decode_iq`], but invokes `on_frame` for every accepted frame before it
/// is tallied — the hook behind the CLI's `--dump` per-frame audit trail. The
/// decode path is otherwise identical, so the callback cannot change the counts.
pub fn decode_iq_with(
    bytes: &[u8],
    in_rate: u32,
    front: Front,
    phases: usize,
    min_conf: f32,
    mut on_frame: impl FnMut(&Frame),
) -> Report {
    let iq = u8_iq_to_cplx(bytes);
    let mut front_end = FrontEnd::new(front, in_rate);
    let mut demod = AdsbDemod::with_phases_min_conf(phases, min_conf);
    let mut report = Report { samples_in: iq.len(), ..Default::default() };

    let window = (in_rate as usize).max(1); // ~1 s of complex samples per window
    for chunk in iq.chunks(window) {
        let envelope = front_end.envelope(chunk);
        report.samples_work += envelope.len() as u64;
        for frame in demod.feed(&envelope) {
            on_frame(&frame);
            tally(&mut report, &frame.payload, frame.meta.crc_ok, frame.meta.confidence);
        }
    }
    for frame in demod.flush() {
        on_frame(&frame);
        tally(&mut report, &frame.payload, frame.meta.crc_ok, frame.meta.confidence);
    }
    report
}

fn tally(report: &mut Report, payload: &FramePayload, crc_ok: bool, confidence: Option<f32>) {
    // Both guards are defensive: `AdsbDemod` only ever emits CRC-valid `Packet`
    // frames today, but keep counting honest if that ever changes.
    let FramePayload::Packet(bytes) = payload else {
        return;
    };
    if !crc_ok {
        return;
    }
    report.frames_valid += 1;
    if let Some(c) = confidence {
        report.conf_sum += c as f64;
        report.conf_min = Some(report.conf_min.map_or(c, |m| m.min(c)));
    }
    let msg = ModeS::new(bytes);
    let df = msg.df();
    *report.df_hist.entry(df).or_default() += 1;

    // ICAO lives in bits 8..32 for all-call replies (DF11) and extended
    // squitters (DF17/18); other DFs carry it XOR-folded into the parity, so
    // only count the address where it is read directly.
    if matches!(df, 11 | 17 | 18) {
        *report.aircraft.entry(msg.icao()).or_default() += 1;
    }
    if matches!(df, 17 | 18) {
        if let Some(tc) = msg.type_code() {
            *report.tc_hist.entry(tc).or_default() += 1;
        }
        if msg.airborne_position().is_some() {
            report.airborne_pos += 1;
        }
    }
}
