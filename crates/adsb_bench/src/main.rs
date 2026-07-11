//! adsb_bench — offline frame-yield ruler for the omnimodem ADS-B decoder.
//!
//! Reads a raw uint8 interleaved I/Q recording (as `rtl_tcp` streams it),
//! runs it through the exact daemon DSP path — `|I+jQ|` magnitude → resample to
//! the 2 MHz working rate → [`AdsbDemod`] — and tallies what came out:
//! CRC-valid frames, unique aircraft, and DF / DF17-type-code histograms.
//!
//! This is the yardstick the ADS-B improvement phases (R1–R5) are measured
//! against: run the same reference recording before and after a change and the
//! frame count and unique-aircraft count show whether the change moved the
//! number. Pass a readsb (or dump1090) baseline to print the gap directly.
//!
//! Usage:
//!   adsb_bench <file.iq> [--in-rate 2400000] [--json]
//!             [--baseline-frames N] [--baseline-aircraft M]
//!
//! The recording is captured off the air, never re-transmitted: 1090 MHz is
//! protected aeronautical spectrum. This tool only reads.

use std::collections::BTreeMap;

use omnimodem_dsp::frontend::iq::u8_iq_to_cplx;
use omnimodem_dsp::frontend::resample::Resampler;
use omnimodem_dsp::mode::Demodulator;
use omnimodem_dsp::modes::adsb::{AdsbDemod, ModeS, ADSB_RATE};
use omnimodem_dsp::types::{FramePayload, Sample};

/// Default capture rate — the wideband rate the daemon commands the dongle to
/// (`ADSB_CAPTURE_RATE` in the RTL-SDR front end).
const DEFAULT_IN_RATE: u32 = 2_400_000;

struct Args {
    path: String,
    in_rate: u32,
    json: bool,
    baseline_frames: Option<u64>,
    baseline_aircraft: Option<u64>,
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&raw) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!(
                "usage: adsb_bench <file.iq> [--in-rate {DEFAULT_IN_RATE}] [--json] \
                 [--baseline-frames N] [--baseline-aircraft M]"
            );
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let path = raw
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .ok_or("missing <file.iq> argument")?;
    Ok(Args {
        path,
        in_rate: flag(raw, "--in-rate")
            .map(|s| s.parse().map_err(|_| "bad --in-rate".to_string()))
            .transpose()?
            .unwrap_or(DEFAULT_IN_RATE),
        json: raw.iter().any(|a| a == "--json"),
        baseline_frames: flag(raw, "--baseline-frames")
            .map(|s| s.parse().map_err(|_| "bad --baseline-frames".to_string()))
            .transpose()?,
        baseline_aircraft: flag(raw, "--baseline-aircraft")
            .map(|s| s.parse().map_err(|_| "bad --baseline-aircraft".to_string()))
            .transpose()?,
    })
}

/// Read `--name value` from the arg list.
fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Tally accumulated over every frame the demod emitted.
#[derive(Default)]
struct Report {
    frames_emitted: u64,
    frames_valid: u64,
    /// Airborne-position frames (DF17/18, TC 9-18/20-22) that passed CRC.
    airborne_pos: u64,
    /// Distinct 24-bit ICAO addresses seen in CRC-valid DF11/17/18 frames.
    aircraft: BTreeMap<u32, u64>,
    /// DF → count, CRC-valid frames only.
    df_hist: BTreeMap<u8, u64>,
    /// DF17/18 type code → count, CRC-valid frames only.
    tc_hist: BTreeMap<u8, u64>,
}

fn run(args: &Args) -> Result<(), String> {
    let bytes = std::fs::read(&args.path).map_err(|e| format!("read {}: {e}", args.path))?;
    let iq = u8_iq_to_cplx(&bytes);
    let samples_in = iq.len();

    // Exact daemon path: magnitude envelope at the capture rate, resampled to
    // the 2 MHz working rate through the same polyphase resampler, then fed to
    // the streaming demod. The resampler is stateful, so a single instance
    // spans every window; `AdsbDemod` itself buffers frames straddling a
    // window boundary. Windowing only bounds peak memory on long captures.
    let mut rs = Resampler::new(args.in_rate, ADSB_RATE, 16);
    let mut demod = AdsbDemod::new();
    let mut report = Report::default();
    let mut samples_work = 0u64;

    let window = args.in_rate as usize; // ~1 s of complex samples per window
    for chunk in iq.chunks(window.max(1)) {
        let mag: Vec<Sample> = chunk.iter().map(|c| c.norm()).collect();
        let resampled = rs.process(&mag);
        samples_work += resampled.len() as u64;
        for frame in demod.feed(&resampled) {
            tally(&mut report, &frame.payload, frame.meta.crc_ok);
        }
    }
    for frame in demod.flush() {
        tally(&mut report, &frame.payload, frame.meta.crc_ok);
    }

    let dur_in = samples_in as f64 / args.in_rate as f64;
    if args.json {
        print_json(args, &report, samples_in, dur_in, samples_work);
    } else {
        print_human(args, &report, samples_in, dur_in, samples_work);
    }
    Ok(())
}

fn tally(report: &mut Report, payload: &FramePayload, crc_ok: bool) {
    report.frames_emitted += 1;
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

fn print_human(args: &Args, r: &Report, samples_in: usize, dur_in: f64, samples_work: u64) {
    println!("adsb_bench {}", args.path);
    println!(
        "  input:        {} Hz uint8 IQ, {} samples ({:.1} s)",
        args.in_rate, samples_in, dur_in
    );
    println!(
        "  working rate: {} Hz, {} samples after resample",
        ADSB_RATE, samples_work
    );
    println!("  frames emitted:      {}", r.frames_emitted);
    println!(
        "  frames CRC-valid:    {}  ({} false-positive preambles)",
        r.frames_valid,
        r.frames_emitted - r.frames_valid
    );
    println!("  airborne positions:  {}", r.airborne_pos);
    println!("  unique aircraft:     {}", r.aircraft.len());
    if !r.aircraft.is_empty() {
        let list: Vec<String> = r.aircraft.keys().map(|a| format!("{a:06X}")).collect();
        println!("    {}", list.join(", "));
    }
    if !r.df_hist.is_empty() {
        println!("  DF histogram (CRC-valid):");
        for (df, n) in &r.df_hist {
            println!("    DF{df:<2} {n:>5}   {}", df_label(*df));
        }
    }
    if !r.tc_hist.is_empty() {
        println!("  DF17/18 type-code histogram:");
        for (tc, n) in &r.tc_hist {
            println!("    TC{tc:<2} {n:>5}   {}", tc_label(*tc));
        }
    }
    print_delta(args, r);
}

fn print_delta(args: &Args, r: &Report) {
    let Some(base_frames) = args.baseline_frames else {
        return;
    };
    println!("  vs baseline:");
    let gap = r.frames_valid as i64 - base_frames as i64;
    println!(
        "    frames:   ours {} / baseline {}  (delta {gap:+}, {:.0}% of baseline)",
        r.frames_valid,
        base_frames,
        pct(r.frames_valid, base_frames)
    );
    if let Some(base_ac) = args.baseline_aircraft {
        let gap = r.aircraft.len() as i64 - base_ac as i64;
        println!(
            "    aircraft: ours {} / baseline {}  (delta {gap:+})",
            r.aircraft.len(),
            base_ac
        );
    }
}

fn pct(ours: u64, base: u64) -> f64 {
    if base == 0 {
        return 0.0;
    }
    100.0 * ours as f64 / base as f64
}

fn print_json(args: &Args, r: &Report, samples_in: usize, dur_in: f64, samples_work: u64) {
    let aircraft: Vec<String> = r.aircraft.keys().map(|a| format!("\"{a:06X}\"")).collect();
    let df: Vec<String> = r
        .df_hist
        .iter()
        .map(|(k, v)| format!("\"{k}\":{v}"))
        .collect();
    let tc: Vec<String> = r
        .tc_hist
        .iter()
        .map(|(k, v)| format!("\"{k}\":{v}"))
        .collect();
    print!("{{");
    print!("\"path\":{:?},", args.path);
    print!("\"in_rate\":{},", args.in_rate);
    print!("\"samples_in\":{samples_in},");
    print!("\"duration_s\":{dur_in:.3},");
    print!("\"work_rate\":{ADSB_RATE},");
    print!("\"samples_work\":{samples_work},");
    print!("\"frames_emitted\":{},", r.frames_emitted);
    print!("\"frames_crc_valid\":{},", r.frames_valid);
    print!("\"airborne_positions\":{},", r.airborne_pos);
    print!("\"unique_aircraft\":{},", r.aircraft.len());
    print!("\"aircraft\":[{}],", aircraft.join(","));
    print!("\"df_hist\":{{{}}},", df.join(","));
    print!("\"tc_hist\":{{{}}}", tc.join(","));
    if let Some(bf) = args.baseline_frames {
        print!(",\"baseline_frames\":{bf}");
    }
    if let Some(ba) = args.baseline_aircraft {
        print!(",\"baseline_aircraft\":{ba}");
    }
    println!("}}");
}

/// Short label for a Mode S downlink format.
fn df_label(df: u8) -> &'static str {
    match df {
        0 => "short air-air surveillance",
        4 => "surveillance, altitude",
        5 => "surveillance, identity",
        11 => "all-call reply",
        16 => "long air-air surveillance",
        17 => "extended squitter (ADS-B)",
        18 => "extended squitter (TIS-B/non-transponder)",
        20 => "Comm-B, altitude",
        21 => "Comm-B, identity",
        24..=31 => "Comm-D (ELM)",
        _ => "",
    }
}

/// Short label for a DF17/18 ME type code.
fn tc_label(tc: u8) -> &'static str {
    match tc {
        1..=4 => "identification",
        5..=8 => "surface position",
        9..=18 => "airborne position (baro alt)",
        19 => "airborne velocity",
        20..=22 => "airborne position (GNSS alt)",
        23..=27 => "reserved",
        28 => "aircraft status",
        29 => "target state & status",
        31 => "operational status",
        _ => "",
    }
}
