//! wavtool — feed a WAV file through a real omnimodem demodulator (offline), or
//! generate a known-good WAV from text. This exercises the same DSP the daemon
//! runs, so it isolates "is the demod chain decoding this signal?" from the
//! live audio / gRPC / UI path: if `wavtool decode` reads a reference recording
//! correctly but the daemon does not, the fault is in the audio device or
//! resampling, not the demod.
//!
//! Usage:
//!   wavtool decode <file.wav> --mode rtty   [--center 2210] [--baud 45.45] [--shift 170] [--scan]
//!   wavtool decode <file.wav> --mode psk31  [--center 1000]
//!   wavtool decode <file.wav> --mode cw     [--center 700] [--wpm 20]
//!   wavtool decode <file.wav> --mode olivia [--tones 32] [--bw 1000]
//!   wavtool decode <file.wav> --mode afsk1200
//!   wavtool gen --mode rtty --text "CQ CQ DE NW5W" --out out.wav [--center 2210] ...
//!
//! `--scan` (decode, FSK/PSK modes) sweeps the center frequency and prints the
//! decode at each step, so you can find the right center for an unknown sample.

use omnimodem_dsp::frontend::resample::Resampler;
use omnimodem_dsp::mode::{Demodulator, Modulator};
use omnimodem_dsp::modes::{
    afsk1200::Afsk1200Demod,
    cw::{CwDemod, CwMod},
    olivia::{OliviaDemod, OliviaMod},
    psk31::{Psk31Demod, Psk31Mod},
    rtty::{RttyDemod, RttyMod},
};
use omnimodem_dsp::types::{Frame, FramePayload, Sample};

struct Params {
    mode: String,
    center: f32,
    baud: f32,
    shift: f32,
    wpm: u16,
    tones: u16,
    bw: u16,
    text: String,
    out: String,
    scan: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: wavtool <decode|gen> [file] --mode <m> [options]   (see header)");
        std::process::exit(2);
    }
    let cmd = args[0].clone();
    let rest = &args[1..];

    // The first non-flag token after the subcommand is the positional file.
    let file = rest.iter().find(|a| !a.starts_with("--")).cloned();
    let mode = flag(rest, "--mode").unwrap_or_else(|| "rtty".into());
    // Per-mode defaults chosen for real-world recordings (US ham RTTY ≈ 2210 Hz).
    let default_center = match mode.as_str() {
        "psk31" => 1000.0,
        "cw" => 700.0,
        _ => 2210.0,
    };
    let p = Params {
        center: flag(rest, "--center").and_then(|s| s.parse().ok()).unwrap_or(default_center),
        baud: flag(rest, "--baud").and_then(|s| s.parse().ok()).unwrap_or(45.45),
        shift: flag(rest, "--shift").and_then(|s| s.parse().ok()).unwrap_or(170.0),
        wpm: flag(rest, "--wpm").and_then(|s| s.parse().ok()).unwrap_or(20),
        tones: flag(rest, "--tones").and_then(|s| s.parse().ok()).unwrap_or(32),
        bw: flag(rest, "--bw").and_then(|s| s.parse().ok()).unwrap_or(1000),
        text: flag(rest, "--text").unwrap_or_default(),
        out: flag(rest, "--out").unwrap_or_else(|| "out.wav".into()),
        scan: rest.iter().any(|a| a == "--scan"),
        mode,
    };

    let r = match cmd.as_str() {
        "decode" => match file {
            Some(f) => decode(&f, &p),
            None => Err("decode needs a <file.wav> argument".into()),
        },
        "gen" => generate(&p),
        other => Err(format!("unknown command {other:?} (expected decode|gen)")),
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Read `--name value` from the arg list.
fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn build_demod(p: &Params, center: f32) -> Result<Box<dyn Demodulator>, String> {
    Ok(match p.mode.as_str() {
        "rtty" => Box::new(RttyDemod::with_center(p.baud, p.shift, center)),
        "psk31" => Box::new(Psk31Demod::new(center)),
        "cw" => Box::new(CwDemod::new(p.wpm, center)),
        "olivia" => Box::new(OliviaDemod::new(p.tones, p.bw)),
        "afsk1200" => Box::new(Afsk1200Demod::ensemble(9)),
        m => return Err(format!("unknown mode {m:?}")),
    })
}

fn decode(path: &str, p: &Params) -> Result<(), String> {
    let (mono, in_rate) = read_wav(path)?;
    let native = build_demod(p, p.center)?.caps().native_rate;
    let samples = resample(mono, in_rate, native);
    println!(
        "decode {path}: {in_rate} Hz -> {native} Hz, {} samples ({:.1}s), mode={}",
        samples.len(),
        samples.len() as f32 / native as f32,
        p.mode,
    );

    if p.scan {
        // Sweep the center to find where an unknown recording decodes.
        for c in (500..=2600).step_by(100) {
            let mut d = build_demod(p, c as f32)?;
            let text = run(&mut *d, &samples);
            println!("  center={c:>4} Hz -> {:?}", trim(&text, 60));
        }
        return Ok(());
    }

    let mut d = build_demod(p, p.center)?;
    let text = run(&mut *d, &samples);
    println!("center={} Hz decoded:\n{text}", p.center);
    Ok(())
}

/// Feed the samples through the demod in daemon-sized chunks and collect text.
fn run(d: &mut dyn Demodulator, samples: &[Sample]) -> String {
    let mut out = String::new();
    for chunk in samples.chunks(4096) {
        for f in d.feed(chunk) {
            push_text(&mut out, &f);
        }
    }
    for f in d.flush() {
        push_text(&mut out, &f);
    }
    out
}

fn push_text(out: &mut String, f: &Frame) {
    match &f.payload {
        FramePayload::Text(t) => out.push_str(t),
        FramePayload::Packet(b) => out.push_str(&format!("[packet {} bytes]\n", b.len())),
        _ => {}
    }
}

fn generate(p: &Params) -> Result<(), String> {
    if p.text.is_empty() {
        return Err("gen needs --text \"...\"".into());
    }
    let mut m: Box<dyn Modulator> = match p.mode.as_str() {
        "rtty" => Box::new(RttyMod::with_center(p.baud, p.shift, p.center)),
        "psk31" => Box::new(Psk31Mod::new(p.center)),
        "cw" => Box::new(CwMod::new(p.wpm, p.center)),
        "olivia" => Box::new(OliviaMod::new(p.tones, p.bw)),
        m => return Err(format!("gen does not support mode {m:?}")),
    };
    let native = m.caps().native_rate;
    let samples = m.modulate(&Frame::text(&p.text)).map_err(|e| format!("{e:?}"))?;
    write_wav(&p.out, &samples, native)?;
    println!(
        "wrote {} ({} samples, {:.1}s @ {} Hz, center {} Hz, mode {})",
        p.out,
        samples.len(),
        samples.len() as f32 / native as f32,
        native,
        p.center,
        p.mode,
    );
    Ok(())
}

/// Read a WAV into mono f32 in [-1, 1] plus its sample rate.
fn read_wav(path: &str) -> Result<(Vec<f32>, u32), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let spec = reader.spec();
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            reader.samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    let ch = spec.channels.max(1) as usize;
    let mono = if ch == 1 {
        raw
    } else {
        raw.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect()
    };
    Ok((mono, spec.sample_rate))
}

fn resample(samples: Vec<f32>, in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate {
        samples
    } else {
        Resampler::new(in_rate, out_rate, 16).process(&samples)
    }
}

fn write_wav(path: &str, samples: &[f32], rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("create {path}: {e}"))?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        w.write_sample(v).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

fn trim(s: &str, n: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(n).collect()
}
