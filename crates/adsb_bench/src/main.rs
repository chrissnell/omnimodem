//! adsb_bench — offline frame-yield ruler for the omnimodem ADS-B decoder.
//!
//! Reads a raw uint8 interleaved I/Q recording (as `rtl_tcp` streams it),
//! runs it through the same magnitude → resample → demod path the daemon uses —
//! `|I+jQ|` magnitude → resample to the 2 MHz working rate → `AdsbDemod` — and
//! tallies what came out: CRC-valid frames, unique aircraft, and DF /
//! DF17-type-code histograms. The decode itself lives in the crate library
//! ([`adsb_bench::decode_iq`]) so the CI regression gate asserts the same path.
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
//!   adsb_bench <file.iq> [--in-rate 2400000] [--json]
//!             [--baseline-frames N] [--baseline-aircraft M]
//!
//! The recording is captured off the air, never re-transmitted: 1090 MHz is
//! protected aeronautical spectrum. This tool only reads.

use adsb_bench::{decode_iq, Report, DEFAULT_IN_RATE};
use omnimodem_dsp::modes::adsb::ADSB_RATE;

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

/// Flags that consume the following token as their value.
const VALUED_FLAGS: &[&str] = &["--in-rate", "--baseline-frames", "--baseline-aircraft"];

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let path = positional(raw).ok_or("missing <file.iq> argument")?.clone();
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
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
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
    let report = decode_iq(&bytes, args.in_rate);
    let dur_in = report.samples_in as f64 / args.in_rate as f64;
    if args.json {
        print_json(args, &report, dur_in);
    } else {
        print_human(args, &report, dur_in);
    }
    Ok(())
}

fn print_human(args: &Args, r: &Report, dur_in: f64) {
    println!("adsb_bench {}", args.path);
    println!(
        "  input:        {} Hz uint8 IQ, {} samples ({:.1} s)",
        args.in_rate, r.samples_in, dur_in
    );
    println!("  working rate: {} Hz, {} samples after resample", ADSB_RATE, r.samples_work);
    println!("  frames (CRC-valid):  {}", r.frames_valid);
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

fn print_json(args: &Args, r: &Report, dur_in: f64) {
    let aircraft: Vec<String> = r.aircraft.keys().map(|a| format!("\"{a:06X}\"")).collect();
    let df: Vec<String> = r.df_hist.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
    let tc: Vec<String> = r.tc_hist.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
    print!("{{");
    print!("\"path\":{},", json_str(&args.path));
    print!("\"in_rate\":{},", args.in_rate);
    print!("\"samples_in\":{},", r.samples_in);
    print!("\"duration_s\":{dur_in:.3},");
    print!("\"work_rate\":{ADSB_RATE},");
    print!("\"samples_work\":{},", r.samples_work);
    print!("\"frames_crc_valid\":{},", r.frames_valid);
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
        assert!(!p.json);
        assert_eq!(p.baseline_frames, None);
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
