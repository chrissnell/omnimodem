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
    /// Explicit `--center`; `None` means auto-detect from the spectrum on decode.
    center: Option<f32>,
    baud: f32,
    shift: f32,
    wpm: u16,
    tones: u16,
    bw: u16,
    text: String,
    out: String,
    scan: bool,
    reverse: bool,
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
    let p = Params {
        center: flag(rest, "--center").and_then(|s| s.parse().ok()),
        baud: flag(rest, "--baud").and_then(|s| s.parse().ok()).unwrap_or(45.45),
        shift: flag(rest, "--shift").and_then(|s| s.parse().ok()).unwrap_or(170.0),
        wpm: flag(rest, "--wpm").and_then(|s| s.parse().ok()).unwrap_or(20),
        tones: flag(rest, "--tones").and_then(|s| s.parse().ok()).unwrap_or(32),
        bw: flag(rest, "--bw").and_then(|s| s.parse().ok()).unwrap_or(1000),
        text: flag(rest, "--text").unwrap_or_default(),
        out: flag(rest, "--out").unwrap_or_else(|| "out.wav".into()),
        scan: rest.iter().any(|a| a == "--scan"),
        reverse: rest.iter().any(|a| a == "--reverse"),
        mode,
    };

    let r = match cmd.as_str() {
        "decode" => match file {
            Some(f) => decode(&f, &p),
            None => Err("decode needs a <file.wav> argument".into()),
        },
        "gen" => generate(&p),
        "analyze" => match file {
            Some(f) => analyze(&f),
            None => Err("analyze needs a <file.wav> argument".into()),
        },
        other => Err(format!("unknown command {other:?} (expected decode|gen|analyze)")),
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
        "rtty" => Box::new(RttyDemod::with_center(p.baud, p.shift, center).reversed(p.reverse)),
        "psk31" => Box::new(Psk31Demod::new(center)),
        "cw" => Box::new(CwDemod::new(p.wpm, center)),
        "olivia" => Box::new(OliviaDemod::new(p.tones, p.bw)),
        "afsk1200" => Box::new(Afsk1200Demod::ensemble(9)),
        m => return Err(format!("unknown mode {m:?}")),
    })
}

/// Per-mode fallback center when none is given and auto-detect declines.
fn default_center(mode: &str) -> f32 {
    match mode {
        "psk31" => 1000.0,
        "cw" => 700.0,
        _ => 2210.0,
    }
}

fn decode(path: &str, p: &Params) -> Result<(), String> {
    let (mono, in_rate) = read_wav(path)?;
    let native = build_demod(p, 0.0)?.caps().native_rate;

    if p.scan {
        let samples = resample(mono, in_rate, native);
        println!("scan {path}: {in_rate} Hz -> {native} Hz, mode={}", p.mode);
        // Sweep the center to find where an unknown recording decodes.
        for c in (500..=2600).step_by(50) {
            let mut d = build_demod(p, c as f32)?;
            let text = run(&mut *d, &samples);
            if !text.trim().is_empty() {
                println!("  center={c:>4} Hz -> {:?}", trim(&text, 60));
            }
        }
        return Ok(());
    }

    // Center: explicit --center wins; otherwise estimate it from the spectrum so
    // an arbitrary recording "just works" without the operator knowing the tone.
    let coarse = p.center.or_else(|| estimate_center(&mono, in_rate, &p.mode));
    let samples = resample(mono, in_rate, native);
    println!(
        "decode {path}: {in_rate} Hz -> {native} Hz, {} samples ({:.1}s), mode={}",
        samples.len(),
        samples.len() as f32 / native as f32,
        p.mode,
    );
    let (center, how) = match (p.center, coarse) {
        (Some(c), _) => (c, "given".to_string()),
        (None, Some(c)) => (c, "auto-detected".to_string()),
        (None, None) => (default_center(&p.mode), "default".to_string()),
    };
    let mut d = build_demod(p, center)?;
    let text = run(&mut *d, &samples);
    let hint = if how == "auto-detected" {
        format!(" (auto-detected; override with --center {center:.0})")
    } else {
        String::new()
    };
    println!("center={center:.0} Hz{hint} decoded:\n{text}");
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
    let center = p.center.unwrap_or_else(|| default_center(&p.mode));
    let mut m: Box<dyn Modulator> = match p.mode.as_str() {
        "rtty" => Box::new(RttyMod::with_center(p.baud, p.shift, center)),
        "psk31" => Box::new(Psk31Mod::new(center)),
        "cw" => Box::new(CwMod::new(p.wpm, center)),
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
        center,
        p.mode,
    );
    Ok(())
}

/// FFT the recording and report where the signal energy actually sits, so you
/// can pick the right --center (or see that the file isn't what you think).
fn analyze(path: &str) -> Result<(), String> {
    let (mono, rate) = read_wav(path)?;
    println!("analyze {path}: {rate} Hz, {} samples ({:.1}s)", mono.len(), mono.len() as f32 / rate as f32);
    let (power, bin_hz) = power_spectrum(&mono, rate).ok_or("file too short to analyze")?;
    let peak = power.iter().cloned().fold(0.0f64, f64::max).max(1e-12);
    println!("dominant frequencies (bin {bin_hz:.1} Hz):");
    for (k, p) in spectral_peaks(&power).iter().take(6) {
        println!("  {:>6.0} Hz  {:>6.1} dB", *k as f32 * bin_hz, 10.0 * (p / peak).log10());
    }
    Ok(())
}

/// Average Hann-windowed power spectrum (first half) and the bin width in Hz.
fn power_spectrum(mono: &[f32], rate: u32) -> Option<(Vec<f64>, f32)> {
    use rustfft::{num_complex::Complex, FftPlanner};
    let n = 8192.min(mono.len().next_power_of_two());
    if n < 64 {
        return None;
    }
    let fft = FftPlanner::new().plan_fft_forward(n);
    let mut power = vec![0.0f64; n / 2];
    let (hop, mut pos, mut frames) = (n / 2, 0usize, 0usize);
    while pos + n <= mono.len() {
        let mut buf: Vec<Complex<f32>> = (0..n)
            .map(|i| {
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos();
                Complex::new(mono[pos + i] * w, 0.0)
            })
            .collect();
        fft.process(&mut buf);
        for (k, p) in power.iter_mut().enumerate() {
            *p += buf[k].norm_sqr() as f64;
        }
        frames += 1;
        pos += hop;
    }
    (frames > 0).then(|| (power, rate as f32 / n as f32))
}

/// Local-maxima bins, strongest first.
fn spectral_peaks(power: &[f64]) -> Vec<(usize, f64)> {
    let mut peaks: Vec<(usize, f64)> = (1..power.len().saturating_sub(1))
        .filter(|&k| power[k] >= power[k - 1] && power[k] >= power[k + 1])
        .map(|k| (k, power[k]))
        .collect();
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    peaks
}

/// Sub-bin peak frequency via parabolic interpolation of the three bins around
/// `k` — the spectral peak rarely lands exactly on a bin, and PSK31's carrier
/// loop is sensitive to a fraction of a bin.
fn interp_freq(power: &[f64], k: usize, bin_hz: f32) -> f32 {
    if k == 0 || k + 1 >= power.len() {
        return k as f32 * bin_hz;
    }
    let (a, b, c) = (power[k - 1], power[k], power[k + 1]);
    let denom = a - 2.0 * b + c;
    let delta = if denom.abs() > f64::EPSILON {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    (k as f32 + delta as f32) * bin_hz
}

/// Estimate the audio center of a recording from its spectrum so decode works
/// without the operator knowing the tone. RTTY is two tones (mark/space) → the
/// center is their midpoint; single-carrier modes (PSK31/CW) sit on one peak.
fn estimate_center(mono: &[f32], rate: u32, mode: &str) -> Option<f32> {
    let (power, bin_hz) = power_spectrum(mono, rate)?;
    let in_band = |k: usize| {
        let f = k as f32 * bin_hz;
        (300.0..3000.0).contains(&f)
    };
    let peaks: Vec<(usize, f64)> =
        spectral_peaks(&power).into_iter().filter(|&(k, _)| in_band(k)).collect();
    let (k0, _) = *peaks.first()?;
    let f0 = interp_freq(&power, k0, bin_hz);
    if mode == "rtty" {
        // The other FSK tone: the strongest peak a plausible shift away (≈ the
        // 170–1000 Hz amateur range), on either side. Center = midpoint.
        if let Some(&(k1, _)) = peaks.iter().find(|&&(k, _)| {
            let d = (k as f32 * bin_hz - f0).abs();
            (80.0..1200.0).contains(&d)
        }) {
            return Some((f0 + interp_freq(&power, k1, bin_hz)) / 2.0);
        }
        return Some(f0);
    }
    // PSK31/CW: the carrier is the symmetry point of the spectrum. The bare peak
    // can land on a reversal-idle sideband (carrier ±15.6 Hz), so take the energy
    // centroid in a narrow ±40 Hz window around the peak — that averages the two
    // sidebands back to the carrier while staying clear of other signals/noise.
    let half = (40.0 / bin_hz) as usize;
    let lo = k0.saturating_sub(half);
    let hi = (k0 + half).min(power.len() - 1);
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (k, &p) in power.iter().enumerate().take(hi + 1).skip(lo) {
        num += k as f64 * p;
        den += p;
    }
    if den > 0.0 {
        return Some((num / den) as f32 * bin_hz);
    }
    Some(f0)
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
