//! adsb_bench — offline frame-yield ruler for the omnimodem ADS-B decoder.
//!
//! resamples it to the 2 MHz working rate, and runs the envelope through
//! `AdsbDemod`, tallying what came out: CRC-valid frames, unique aircraft, and
//! DF / DF17-type-code histograms. The decode itself lives in the crate library
//! ([`adsb_bench::decode_iq`]) so the CI regression gate asserts the same path.
//!
//! ## Front ends (`--front`)
//! Two ways to get from the 2.4 Msps capture to the 2 MHz magnitude envelope the
//! demod slices:
//! - `complex` (default, R1): band-limited complex decimation first —
//!   [`ComplexResampler`] resamples the I/Q 2.4M→2.0M, then `|I+jQ|` magnitude.
//!   This is apples-to-apples with how readsb consumes its native rate: the
//!   anti-alias lowpass sees the true channel, not the doubled-bandwidth
//!   envelope, so the pulse edges are not aliased the way the envelope path
//!   aliases them.
//! - `mag` (R0 baseline): the original path — `|I+jQ|` magnitude at the capture
//!   rate, then a real [`Resampler`] decimates the envelope 2.4M→2.0M. Envelope
//!   detection doubles the bandwidth, so decimating it aliases sharp pulse edges.
//!   Kept so the ruler can reproduce the pre-R1 baseline on any recording.
//!
//! The `mag` path is the one the live daemon runs today (it takes the magnitude
//! at the 2.4 Msps capture rate and the RX worker decimates the envelope to
//! 2 MHz). `complex` is the *intended* R1 front end and currently runs ahead of
//! production — it is not yet wired into the daemon's `RawMag` transport, whose
//! i16 audio path carries an envelope rather than I/Q. Read a default (`complex`)
//! run as the R1 target, not as the shipping decoder's yield.
//!
//! Center-frequency caveat: [`ComplexResampler`]'s anti-alias lowpass is centered
//! at DC, so `complex` assumes the 1090 MHz signal sits near DC. That holds for
//! the `rtl_sdr`-captured reference recording (tuned directly to 1090 MHz). It is
//! *not* automatically true of a daemon-produced capture: the daemon's tuner
//! plan parks the signal a quarter-band (~600 kHz) above hardware center to dodge
//! the R820T DC spike, and `RawMag` bypasses the NCO, so a daemon I/Q recording
//! is offset from DC. `--center-offset <hz>` closes that gap: the `complex` front
//! end NCO-down-shifts the capture by that offset (reusing `DownConverter`) so the
//! DC-centered anti-alias lowpass sees the true channel. Point it at a daemon
//! capture with `--center-offset 600000` to measure `complex`-in-daemon against
//! the shipping `mag` path before deciding whether to promote it.
//!
//! (The live daemon additionally scales the envelope by 1/√2 and quantizes it to
//! i16 for its audio-delivery path; the PPM demod is scale-independent, so the
//! bench skips both and the decode is equivalent, not bit-identical.)
//!
//! This is the yardstick the ADS-B improvement phases (R1–R5) are measured
//! against: run the same reference recording before and after a change and the
//! frame count and unique-aircraft count show whether the change moved the
//! number. Pass a readsb (or dump1090) baseline to print the gap directly.
//!
//! Usage:
//!   adsb_bench <file.iq> [--in-rate 2400000] [--front complex|mag] [--phases N]
//!             [--min-conf C] [--dump] [--json] [--baseline-frames N] [--baseline-aircraft M]
//!
//! `--min-conf 0` disables the R4 soft-decision gate, so a before/after pair
//! (`--min-conf 0` vs the default) shows exactly which frames the gate rejects.
//! `--dump` lists every accepted frame (df/tc/icao/conf/bytes, plus the DF18
//! control field) to stderr, so a frame flagged as a false positive can be
//! audited on the evidence — frame count, control field, soft confidence.
//!
//! The recording is captured off the air, never re-transmitted: 1090 MHz is
//! protected aeronautical spectrum. This tool only reads.

use adsb_bench::{
    decode_iq, decode_iq_with, DecodeOpts, Demod, Front, Report, ADSB_NATIVE_RATE, DEFAULT_IN_RATE,
    DEFAULT_MIN_CONF, DEFAULT_PHASES, DEFAULT_WORK_RATE,
};
use omnimodem_dsp::modes::adsb::ModeS;
use omnimodem_dsp::types::{Frame, FramePayload};

struct Args {
    path: String,
    in_rate: u32,
    /// Working rate to resample to and slice at (R5 Lever 1). Default is the
    /// shipping 2 MHz rate; `--work-rate 4000000` preserves the native capture
    /// bandwidth instead of band-limiting it away in a 2.0 MHz downsample.
    work_rate: u32,
    front: Front,
    /// Hz above hardware center where 1090 MHz sits in the capture. `0` (default)
    /// for a DC-tuned reference recording; a daemon capture parks the signal a
    /// quarter-band up (~600 kHz at 2.4 Msps) to dodge the R820T DC spike, so pass
    /// that offset to let the `complex` front end NCO-shift it to DC first.
    center_offset_hz: f32,
    /// R6 demod core. Default `legacy` is the shipping R1–R5 ensemble; `native`
    /// runs the 2.4 Msps correlating core on the un-resampled capture.
    demod: Demod,
    /// Sub-sample slicer phases (R3 ensemble). Default matches the shipping
    /// decoder; `--phases 1` reproduces the pre-R3 single-phase baseline.
    phases: usize,
    /// R4 soft-decision reject threshold. Default matches the shipping decoder;
    /// `--min-conf 0` disables the gate to reveal the ghosts it rejects.
    min_conf: f32,
    /// R5 Lever 2a: single-bit CRC repair (off by default, measurement-gated).
    repair: bool,
    /// R5 Lever 2b: ICAO-roster-gated address-overlaid recovery (off by default).
    roster: bool,
    /// Print every accepted frame (df/tc/icao/conf/bytes) to stderr.
    dump: bool,
    json: bool,
    baseline_frames: Option<u64>,
    baseline_aircraft: Option<u64>,
}

impl Args {
    /// The knobs this run passes to the shared decode path.
    fn decode_opts(&self) -> DecodeOpts {
        DecodeOpts {
            in_rate: self.in_rate,
            work_rate: self.work_rate,
            front: self.front,
            center_offset_hz: self.center_offset_hz,
            demod: self.demod,
            phases: self.phases,
            min_conf: self.min_conf,
            repair: self.repair,
            roster: self.roster,
        }
    }
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&raw) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!(
                "usage: adsb_bench <file.iq> [--in-rate {DEFAULT_IN_RATE}] \
                 [--demod legacy|native] \
                 [--work-rate {DEFAULT_WORK_RATE}] [--front complex|mag] \
                 [--center-offset 0] \
                 [--phases {DEFAULT_PHASES}] [--min-conf {DEFAULT_MIN_CONF}] \
                 [--repair] [--roster] [--dump] [--json] \
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

/// Flags that consume the following token as their value.
const VALUED_FLAGS: &[&str] = &[
    "--in-rate",
    "--demod",
    "--work-rate",
    "--front",
    "--center-offset",
    "--phases",
    "--min-conf",
    "--baseline-frames",
    "--baseline-aircraft",
];

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let path = positional(raw).ok_or("missing <file.iq> argument")?.clone();
    Ok(Args {
        path,
        in_rate: flag(raw, "--in-rate")
            .map(|s| s.parse().map_err(|_| "bad --in-rate".to_string()))
            .transpose()?
            .unwrap_or(DEFAULT_IN_RATE),
        work_rate: flag(raw, "--work-rate")
            .map(|s| parse_work_rate(&s))
            .transpose()?
            .unwrap_or(DEFAULT_WORK_RATE),
        front: match flag(raw, "--front").as_deref() {
            None | Some("complex") => Front::Complex,
            Some("mag") => Front::Mag,
            Some(other) => return Err(format!("bad --front {other:?} (want complex|mag)")),
        },
        center_offset_hz: flag(raw, "--center-offset")
            .map(|s| s.parse::<f32>().map_err(|_| "bad --center-offset (want Hz)".to_string()))
            .transpose()?
            .unwrap_or(0.0),
        demod: match flag(raw, "--demod").as_deref() {
            None | Some("legacy") => Demod::Legacy,
            Some("native") => Demod::Native,
            Some(other) => return Err(format!("bad --demod {other:?} (want legacy|native)")),
        },
        phases: flag(raw, "--phases")
            .map(|s| match s.parse() {
                Ok(0) | Err(_) => Err("bad --phases (want an integer >= 1)".to_string()),
                Ok(n) => Ok(n),
            })
            .transpose()?
            .unwrap_or(DEFAULT_PHASES),
        min_conf: flag(raw, "--min-conf")
            .map(|s| match s.parse::<f32>() {
                Ok(c) if (0.0..=1.0).contains(&c) => Ok(c),
                _ => Err("bad --min-conf (want a float in [0, 1])".to_string()),
            })
            .transpose()?
            .unwrap_or(DEFAULT_MIN_CONF),
        repair: raw.iter().any(|a| a == "--repair"),
        roster: raw.iter().any(|a| a == "--roster"),
        dump: raw.iter().any(|a| a == "--dump"),
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
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Parse `--work-rate`, rejecting rates the slicer cannot run at — it needs an
/// even whole number of MHz so a half-µs PPM slot is a whole number of samples.
fn parse_work_rate(s: &str) -> Result<u32, String> {
    let rate: u32 = s.parse().map_err(|_| "bad --work-rate".to_string())?;
    if rate.is_multiple_of(1_000_000) && (rate / 1_000_000).is_multiple_of(2) {
        Ok(rate)
    } else {
        Err(format!("bad --work-rate {rate} (want an even whole number of MHz, e.g. 2000000 or {ADSB_NATIVE_RATE})"))
    }
}

/// First positional token — the input path — skipping flags and the values
/// consumed by valued flags, so `--baseline-frames 100 rec.iq` finds `rec.iq`.
fn positional(args: &[String]) -> Option<&String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with("--") {
            if VALUED_FLAGS.contains(&a.as_str()) {
                i += 1; // also skip its value
            }
            i += 1;
            continue;
        }
        return Some(a);
    }
    None
}

fn run(args: &Args) -> Result<(), String> {
    let bytes = std::fs::read(&args.path).map_err(|e| format!("read {}: {e}", args.path))?;
    // `--front` selects the front end that turns the capture into the 2 MHz
    // envelope, `--phases` the slicer ensemble width, and `--min-conf` the R4
    // soft-decision gate; the decode itself is the shared library path (see
    // [`adsb_bench::decode_iq`]), so the CLI and the CI gate agree. `--dump`
    // routes every accepted frame through the audit hook on the way past.
    let opts = args.decode_opts();
    let report = if args.dump {
        decode_iq_with(&bytes, &opts, |f| {
            if let Some(line) = dump_line(f) {
                eprintln!("{line}");
            }
        })
    } else {
        decode_iq(&bytes, &opts)
    };
    let dur_in = report.samples_in as f64 / args.in_rate as f64;
    if args.json {
        print_json(args, &report, dur_in);
    } else {
        print_human(args, &report, dur_in);
    }
    Ok(())
}

/// Format one accepted frame for `--dump`, or `None` for a non-packet payload.
/// The per-frame audit trail behind the aggregate counts: `df`/`tc`/`icao`, the
/// soft `conf`, the sample offset, and the raw hex. For DF18 it also prints the
/// control field `cf` — a real ADS-B/TIS-B frame uses cf 0-6; cf 7 is reserved,
/// so a lone cf=7 frame is a CRC-lucky slice, not a transmitter. Pair with
/// `--min-conf 0` to also see the frames the gate rejects.
fn dump_line(frame: &Frame) -> Option<String> {
    let FramePayload::Packet(b) = &frame.payload else {
        return None;
    };
    let m = ModeS::new(b);
    let df = m.df();
    let tc = m.type_code().map(|t| t as i32).unwrap_or(-1);
    // DF18 control field = the low 3 bits of byte 0 (same bits `ca()` returns).
    let cf = if df == 18 { format!(" cf={}", m.ca()) } else { String::new() };
    let hex: String = b.iter().map(|x| format!("{x:02X}")).collect();
    Some(format!(
        "frame df={df}{cf} tc={tc} icao={:06X} conf={:.3} off={} {hex}",
        m.icao(),
        frame.meta.confidence.unwrap_or(0.0),
        frame.meta.sample_offset
    ))
}

fn print_human(args: &Args, r: &Report, dur_in: f64) {
    println!("adsb_bench {}", args.path);
    println!(
        "  input:        {} Hz uint8 IQ, {} samples ({:.1} s)",
        args.in_rate, r.samples_in, dur_in
    );
    match args.demod {
        Demod::Legacy => {
            println!("  demod core:   legacy (R1–R5 resample + phase ensemble)");
            println!("  front end:    {}", args.front.label());
            if args.front == Front::Complex && args.center_offset_hz != 0.0 {
                println!("  center offset: {:.0} Hz (NCO down-shift to DC)", args.center_offset_hz);
            }
            println!("  slicer phases: {}", args.phases);
            println!(
                "  min confidence: {:.2}{}",
                args.min_conf,
                if args.min_conf == 0.0 { " (gate disabled)" } else { "" }
            );
            println!("  recovery:     repair {}, roster {}", onoff(args.repair), onoff(args.roster));
            println!(
                "  working rate: {} Hz, {} samples after resample",
                r.work_rate, r.samples_work
            );
        }
        Demod::Native => {
            println!("  demod core:   native (R6 2.4 Msps correlating slicer, CRC-clean)");
            println!(
                "  working rate: {} Hz (native, no resample), {} magnitude samples",
                r.work_rate, r.samples_work
            );
        }
    }
    println!("  frames (CRC-valid):  {}", r.frames_valid);
    if let (Some(mean), Some(min)) = (r.conf_mean(), r.conf_min) {
        println!("  frame confidence:    mean {mean:.3}, min {min:.3} (gate {:.2})", args.min_conf);
    }
    println!("  airborne positions:  {}", r.airborne_pos);
    println!("  unique aircraft:     {}", r.unique_aircraft());
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
        let gap = r.unique_aircraft() as i64 - base_ac as i64;
        println!(
            "    aircraft: ours {} / baseline {}  (delta {gap:+})",
            r.unique_aircraft(),
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

fn onoff(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

fn print_json(args: &Args, r: &Report, dur_in: f64) {
    let aircraft: Vec<String> = r.aircraft.keys().map(|a| format!("\"{a:06X}\"")).collect();
    let df: Vec<String> = r.df_hist.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
    let tc: Vec<String> = r.tc_hist.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
    print!("{{");
    print!("\"path\":{},", json_str(&args.path));
    print!("\"in_rate\":{},", args.in_rate);
    print!(
        "\"demod\":\"{}\",",
        match args.demod {
            Demod::Legacy => "legacy",
            Demod::Native => "native",
        }
    );
    print!(
        "\"front\":\"{}\",",
        match args.front {
            Front::Complex => "complex",
            Front::Mag => "mag",
        }
    );
    print!("\"center_offset_hz\":{:.0},", args.center_offset_hz);
    print!("\"phases\":{},", args.phases);
    print!("\"min_conf\":{:.3},", args.min_conf);
    print!("\"repair\":{},", args.repair);
    print!("\"roster\":{},", args.roster);
    print!("\"samples_in\":{},", r.samples_in);
    print!("\"duration_s\":{dur_in:.3},");
    print!("\"work_rate\":{},", r.work_rate);
    print!("\"samples_work\":{},", r.samples_work);
    print!("\"frames_crc_valid\":{},", r.frames_valid);
    print!("\"conf_mean\":{:.3},", r.conf_mean().unwrap_or(0.0));
    print!("\"conf_min\":{:.3},", r.conf_min.unwrap_or(0.0));
    print!("\"airborne_positions\":{},", r.airborne_pos);
    print!("\"unique_aircraft\":{},", r.unique_aircraft());
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

/// Encode a string as a JSON string literal (quotes + minimal escaping).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn path_first_with_defaults() {
        let p = parse_args(&args(&["rec.iq"])).unwrap();
        assert_eq!(p.path, "rec.iq");
        assert_eq!(p.in_rate, DEFAULT_IN_RATE);
        assert_eq!(p.demod, Demod::Legacy); // R6 native core is opt-in, default off
        assert_eq!(p.front, Front::Complex); // R1 complex front end is the default
        assert_eq!(p.phases, DEFAULT_PHASES); // R3 ensemble is the default
        assert_eq!(p.min_conf, DEFAULT_MIN_CONF); // R4 gate on by default
        assert!(!p.dump);
        assert!(!p.json);
        assert_eq!(p.baseline_frames, None);
    }

    #[test]
    fn min_conf_flag_parses_and_bounds() {
        assert_eq!(parse_args(&args(&["rec.iq", "--min-conf", "0"])).unwrap().min_conf, 0.0);
        assert_eq!(parse_args(&args(&["rec.iq", "--min-conf", "0.5"])).unwrap().min_conf, 0.5);
        assert!(parse_args(&args(&["rec.iq", "--min-conf", "1.5"])).is_err());
        assert!(parse_args(&args(&["rec.iq", "--min-conf", "-0.1"])).is_err());
        assert!(parse_args(&args(&["rec.iq", "--min-conf", "high"])).is_err());
    }

    #[test]
    fn dump_flag_parses() {
        assert!(!parse_args(&args(&["rec.iq"])).unwrap().dump);
        assert!(parse_args(&args(&["rec.iq", "--dump"])).unwrap().dump);
    }

    #[test]
    fn dump_line_formats_df18_control_field() {
        use omnimodem_dsp::types::FrameMeta;
        // The reference ghost: DF18, reserved CF=7, TC=18, ICAO B6A22C.
        let bytes = vec![
            0x97, 0xB6, 0xA2, 0x2C, 0x91, 0xD7, 0x47, 0x55, 0xA6, 0x75, 0xA6, 0x64, 0xD7, 0xEA,
        ];
        let f = Frame {
            payload: FramePayload::Packet(bytes),
            meta: FrameMeta { confidence: Some(0.258), sample_offset: 19349634, ..Default::default() },
        };
        let line = dump_line(&f).unwrap();
        assert!(line.contains("df=18 cf=7 tc=18 icao=B6A22C"), "line: {line}");
        assert!(line.contains("conf=0.258"), "line: {line}");
        assert!(line.ends_with("97B6A22C91D74755A675A664D7EA"), "line: {line}");
        // A short (non-DF18) frame carries no cf= field.
        let short = Frame {
            payload: FramePayload::Packet(vec![0x5D, 0xA6, 0xC8, 0x8E, 0x15, 0xC0, 0xA7]),
            meta: FrameMeta::default(),
        };
        assert!(!dump_line(&short).unwrap().contains("cf="));
    }

    #[test]
    fn phases_flag_parses_and_rejects_zero() {
        assert_eq!(parse_args(&args(&["rec.iq", "--phases", "1"])).unwrap().phases, 1);
        assert_eq!(parse_args(&args(&["rec.iq", "--phases", "6"])).unwrap().phases, 6);
        assert!(parse_args(&args(&["rec.iq", "--phases", "0"])).is_err());
        assert!(parse_args(&args(&["rec.iq", "--phases", "two"])).is_err());
    }

    #[test]
    fn demod_flag_selects_core() {
        assert_eq!(parse_args(&args(&["rec.iq"])).unwrap().demod, Demod::Legacy);
        assert_eq!(
            parse_args(&args(&["rec.iq", "--demod", "legacy"])).unwrap().demod,
            Demod::Legacy
        );
        assert_eq!(
            parse_args(&args(&["rec.iq", "--demod", "native"])).unwrap().demod,
            Demod::Native
        );
        assert!(parse_args(&args(&["rec.iq", "--demod", "bogus"])).is_err());
        // `--demod` consumes its value, so a valued flag before the path still
        // finds the path.
        let p = parse_args(&args(&["--demod", "native", "rec.iq"])).unwrap();
        assert_eq!(p.path, "rec.iq");
        assert_eq!(p.demod, Demod::Native);
    }

    #[test]
    fn center_offset_flag_parses_and_defaults_zero() {
        assert_eq!(parse_args(&args(&["rec.iq"])).unwrap().center_offset_hz, 0.0);
        assert_eq!(
            parse_args(&args(&["rec.iq", "--center-offset", "600000"])).unwrap().center_offset_hz,
            600_000.0
        );
        assert!(parse_args(&args(&["rec.iq", "--center-offset", "middle"])).is_err());
        // A valued flag before the path is not mistaken for the path.
        let p = parse_args(&args(&["--center-offset", "600000", "rec.iq"])).unwrap();
        assert_eq!(p.path, "rec.iq");
        assert_eq!(p.center_offset_hz, 600_000.0);
    }

    #[test]
    fn front_flag_selects_path() {
        assert_eq!(
            parse_args(&args(&["rec.iq"])).unwrap().front,
            Front::Complex
        );
        assert_eq!(
            parse_args(&args(&["rec.iq", "--front", "complex"]))
                .unwrap()
                .front,
            Front::Complex
        );
        assert_eq!(
            parse_args(&args(&["rec.iq", "--front", "mag"]))
                .unwrap()
                .front,
            Front::Mag
        );
        assert!(parse_args(&args(&["rec.iq", "--front", "bogus"])).is_err());
        // `--front` greedily consumes the next token as its value, so an
        // adjacent flag is read as a (bad) value rather than silently ignored.
        assert!(parse_args(&args(&["rec.iq", "--front", "--json"])).is_err());
        // A valued --front must not be mistaken for the input path.
        let p = parse_args(&args(&["--front", "mag", "rec.iq"])).unwrap();
        assert_eq!(p.path, "rec.iq");
        assert_eq!(p.front, Front::Mag);
    }

    #[test]
    fn valued_flag_before_path_is_not_taken_as_path() {
        let p = parse_args(&args(&["--baseline-frames", "100", "rec.iq"])).unwrap();
        assert_eq!(p.path, "rec.iq");
        assert_eq!(p.baseline_frames, Some(100));
    }

    #[test]
    fn boolean_flag_before_path() {
        let p = parse_args(&args(&["--json", "rec.iq"])).unwrap();
        assert_eq!(p.path, "rec.iq");
        assert!(p.json);
    }

    #[test]
    fn all_flags_parse() {
        let p = parse_args(&args(&[
            "rec.iq",
            "--in-rate",
            "3000000",
            "--baseline-frames",
            "250",
            "--baseline-aircraft",
            "3",
            "--json",
        ]))
        .unwrap();
        assert_eq!(p.in_rate, 3_000_000);
        assert_eq!(p.baseline_frames, Some(250));
        assert_eq!(p.baseline_aircraft, Some(3));
        assert!(p.json);
    }

    #[test]
    fn work_rate_flag_parses_and_bounds() {
        assert_eq!(parse_args(&args(&["rec.iq"])).unwrap().work_rate, DEFAULT_WORK_RATE);
        assert_eq!(
            parse_args(&args(&["rec.iq", "--work-rate", "4000000"])).unwrap().work_rate,
            4_000_000
        );
        // Non-integer-MHz and odd-MHz rates the slicer cannot run at are rejected.
        assert!(parse_args(&args(&["rec.iq", "--work-rate", "2400000"])).is_err());
        assert!(parse_args(&args(&["rec.iq", "--work-rate", "3000000"])).is_err());
        assert!(parse_args(&args(&["rec.iq", "--work-rate", "fast"])).is_err());
    }

    #[test]
    fn repair_and_roster_flags_default_off() {
        let p = parse_args(&args(&["rec.iq"])).unwrap();
        assert!(!p.repair);
        assert!(!p.roster);
        let p = parse_args(&args(&["rec.iq", "--repair", "--roster"])).unwrap();
        assert!(p.repair);
        assert!(p.roster);
    }

    #[test]
    fn missing_path_errors() {
        assert!(parse_args(&args(&["--json"])).is_err());
        assert!(parse_args(&args(&[])).is_err());
    }

    #[test]
    fn bad_numeric_flag_errors() {
        assert!(parse_args(&args(&["rec.iq", "--in-rate", "fast"])).is_err());
    }

    #[test]
    fn json_str_escapes_control_and_quotes() {
        assert_eq!(json_str("a/b.iq"), "\"a/b.iq\"");
        assert_eq!(json_str("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_str("a\tb"), "\"a\\tb\"");
        assert_eq!(json_str("\u{7f}"), "\"\u{7f}\""); // DEL is >= 0x20, passes through
        assert_eq!(json_str("\u{1}"), "\"\\u0001\"");
    }
}
