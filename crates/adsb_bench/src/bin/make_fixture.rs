//! make_fixture — regenerate the committed ADS-B CI regression clip.
//!
//! Builds a small, fully deterministic uint8 I/Q recording from the loopback
//! modulator (never off the air) and writes it where `tests/regression_gate.rs`
//! reads it. The clip carries a fixed set of Mode S frames — 5 DF11 all-call
//! replies and 3 DF17 identification squitters across 3 aircraft — modulated to
//! magnitude, rotated across I and Q by an exact Fs/4 quarter-turn so energy is
//! not parked on a single axis, dithered with a seeded low noise floor, and
//! quantized to uint8. That shape mirrors the KSLC reference slice
//! (5 DF11 / 3 DF17, 3 aircraft) so the gate's floor tracks real decoder yield.
//!
//! The clip is written at the 2 MHz **working** rate, not the 2.4 Msps capture
//! rate: a 0.5 µs Mode S pulse is a single sample at 2 MHz, and round-tripping
//! that impulse through a non-integer 2.0↔2.4 resample filters it below the
//! demod's preamble threshold. Writing at the working rate keeps the gate
//! deterministic and focused on the decode core (preamble → PPM slice → CRC →
//! message parse). The 2.4→2.0 resample stage is covered separately by the
//! resampler's own unit tests and by real-capture benchmarking during the
//! R-phases. `tests/regression_gate.rs` decodes this clip with `in_rate` equal
//! to the working rate accordingly.
//!
//! Bit-exact by construction: a seeded integer PRNG plus only IEEE-754
//! arithmetic that is identical across platforms — no `sin`/`cos` (whose libm
//! results vary by platform), no time or entropy. Rerunning reproduces the
//! committed bytes exactly on any IEEE-754 target.
//!
//! Usage: make_fixture [out.iq]
//!   (default: crates/adsb_bench/testdata/adsb_ci_clip.iq relative to CARGO_MANIFEST_DIR)

use omnimodem_dsp::mode::Modulator;
use omnimodem_dsp::modes::adsb::{
    encode_all_call_reply, encode_identification, AdsbMod, ADSB_RATE, CA_LEVEL2,
};
use omnimodem_dsp::types::Frame;

/// Pulse amplitude before dither. Below 1.0 so a peak pulse plus the noise floor
/// still fits the normalized [-1, 1] range and never saturates the uint8 clamp.
const AMP: f32 = 0.9;

/// The three aircraft in the clip — real US ICAO ranges, matching the KSLC
/// reference slice the bench was first calibrated against.
const AIRCRAFT: [(u32, &str); 3] =
    [(0x4BA956, "SWR221"), (0xA31A76, "UAL512"), (0xA6C88E, "DAL880")];

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("{}/testdata/adsb_ci_clip.iq", env!("CARGO_MANIFEST_DIR")));

    let mag = build_magnitude();
    let bytes = to_uint8_iq(&mag);
    std::fs::write(&out, &bytes).unwrap_or_else(|e| panic!("write {out}: {e}"));
    eprintln!("wrote {} ({} bytes, {} IQ pairs)", out, bytes.len(), bytes.len() / 2);
}

/// Modulate the fixed frame set to a magnitude waveform at [`ADSB_RATE`], with
/// quiet lead/inter-frame/tail gaps so the demod has margin to lock on.
fn build_magnitude() -> Vec<f32> {
    let spu = (ADSB_RATE / 1_000_000) as usize; // samples per microsecond @ 2 MHz
    let gap = |us: usize| std::iter::repeat_n(0.0f32, us * spu);

    let mut modu = AdsbMod::new();
    let mut mag: Vec<f32> = gap(50).collect(); // lead-in silence

    for frame in frame_set() {
        let wave = modu.modulate(&Frame::packet(frame)).expect("modulate");
        mag.extend_from_slice(&wave);
        mag.extend(gap(100)); // inter-frame quiet
    }
    mag.extend(gap(50)); // tail
    mag
}

/// The committed frame set: 5 DF11 all-call replies + 3 DF17 identification
/// squitters across the 3 aircraft (each aircraft appears in both a DF11 and a
/// DF17, so `unique_aircraft` == 3).
fn frame_set() -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    for &(icao, callsign) in &AIRCRAFT {
        frames.push(encode_all_call_reply(icao, CA_LEVEL2).to_vec());
        frames.push(encode_identification(icao, callsign).to_vec());
    }
    // Two extra all-call replies → 5 DF11 total, mirroring the reference shape.
    frames.push(encode_all_call_reply(AIRCRAFT[0].0, CA_LEVEL2).to_vec());
    frames.push(encode_all_call_reply(AIRCRAFT[1].0, CA_LEVEL2).to_vec());
    frames
}

/// Rotate the magnitude across I and Q by an exact Fs/4 quarter-turn (a 500 kHz
/// IF at 2 Msps) so energy spreads across both axes like a real capture, add a
/// seeded low noise floor, and quantize to interleaved uint8 centered at 127.5.
/// The rotation multipliers are only `{1, 0, -1}`, so every operation is exact
/// IEEE-754 arithmetic — bit-identical across platforms (unlike `sin`/`cos`).
/// The demod uses `|I+jQ|`, so the rotation does not affect the decode; it only
/// makes the bytes look like a genuine off-air recording.
fn to_uint8_iq(mag: &[f32]) -> Vec<u8> {
    // Quarter-turn phasors e^{j·k·π/2}: (cos, sin) for k mod 4.
    const ROT: [(f32, f32); 4] = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];

    let mut rng = Lcg::new(0x0AD5_B0C1_FACE_1090);
    let mut out = Vec::with_capacity(mag.len() * 2);
    let noise = 0.03f32; // peak dither amplitude, well below the AMP pulses

    for (k, &m) in mag.iter().enumerate() {
        let (ci, cq) = ROT[k & 3];
        let i = AMP * m * ci + noise * rng.bipolar();
        let q = AMP * m * cq + noise * rng.bipolar();
        out.push(quantize(i));
        out.push(quantize(q));
    }
    out
}

/// Map a normalized sample in ~[-1, 1] to uint8 centered at 127.5 (rtl_tcp
/// convention), clamped to the byte range.
fn quantize(v: f32) -> u8 {
    (127.5 + 127.5 * v).round().clamp(0.0, 255.0) as u8
}

/// Tiny deterministic LCG (PCG / Knuth MMIX 64-bit multiplier and increment) so
/// the fixture bytes are reproducible without pulling in an RNG dependency.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }

    /// A value in [-1, 1] (basic IEEE-754 arithmetic, so platform-independent).
    fn bipolar(&mut self) -> f32 {
        (self.next_u32() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}
