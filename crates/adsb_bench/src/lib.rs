//! adsb_bench decode core — the offline frame-yield path shared by the CLI
//! (`main.rs`) and the CI regression gate (`tests/regression_gate.rs`).
//!
//! [`decode_iq`] runs a raw uint8 interleaved I/Q buffer through the exact
//! daemon DSP path — `|I+jQ|` magnitude → resample to the 2 MHz working rate
//! (the same polyphase [`Resampler`]) → [`AdsbDemod`] — and returns a [`Report`]
//! tallying the decoder's usable yield. Keeping the decode here (not in the
//! binary) lets a test assert the same numbers the CLI prints.

use std::collections::BTreeMap;

use omnimodem_dsp::frontend::iq::u8_iq_to_cplx;
use omnimodem_dsp::frontend::resample::Resampler;
use omnimodem_dsp::mode::Demodulator;
use omnimodem_dsp::modes::adsb::{AdsbDemod, ModeS, ADSB_RATE};
use omnimodem_dsp::types::{FramePayload, Sample};

/// Default capture rate — the wideband rate the daemon commands the dongle to
/// (`ADSB_CAPTURE_RATE` in the RTL-SDR front end).
pub const DEFAULT_IN_RATE: u32 = 2_400_000;

/// Tally accumulated over every frame the demod emitted. `AdsbDemod` gates on
/// parity (`require_crc`), so every emitted frame is CRC-valid — this counts the
/// decoder's usable yield, the number each R-phase is trying to move.
#[derive(Default)]
pub struct Report {
    /// CRC-valid frames emitted.
    pub frames_valid: u64,
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
}

/// Decode a raw uint8 interleaved I/Q buffer at `in_rate` and tally the yield.
///
/// Mirrors the daemon path: magnitude envelope at the capture rate, resampled to
/// [`ADSB_RATE`] through the same polyphase resampler, fed to the streaming
/// demod. The resampler is stateful, so a single instance spans every window;
/// `AdsbDemod` buffers frames straddling a window boundary. Windowing only
/// bounds peak memory on long captures.
pub fn decode_iq(bytes: &[u8], in_rate: u32) -> Report {
    let iq = u8_iq_to_cplx(bytes);
    let mut rs = Resampler::new(in_rate, ADSB_RATE, 16);
    let mut demod = AdsbDemod::new();
    let mut report = Report { samples_in: iq.len(), ..Default::default() };

    let window = (in_rate as usize).max(1); // ~1 s of complex samples per window
    for chunk in iq.chunks(window) {
        let mag: Vec<Sample> = chunk.iter().map(|c| c.norm()).collect();
        let resampled = rs.process(&mag);
        report.samples_work += resampled.len() as u64;
        for frame in demod.feed(&resampled) {
            tally(&mut report, &frame.payload, frame.meta.crc_ok);
        }
    }
    for frame in demod.flush() {
        tally(&mut report, &frame.payload, frame.meta.crc_ok);
    }
    report
}

fn tally(report: &mut Report, payload: &FramePayload, crc_ok: bool) {
    // Both guards are defensive: `AdsbDemod` only ever emits CRC-valid `Packet`
    // frames today, but keep counting honest if that ever changes.
    let FramePayload::Packet(bytes) = payload else {
        return;
    };
    if !crc_ok {
        return;
    }
    report.frames_valid += 1;
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
