//! ADS-B mode tests, including canonical Mode S vectors and streaming behavior.

use super::message::{
    cpr_decode_airborne, encode_all_call_reply, encode_identification, ModeS, CA_LEVEL2,
};
use super::ppm::{ParallelDemodulator, PpmDemodulator, PpmModulator};
use super::*;
use crate::mode::{Demodulator, Modulator};
use crate::types::{Frame, FramePayload};

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// Canonical DF17 identification frame: ICAO 4840D6, callsign "KLM1023".
const KLM1023: &str = "8D4840D6202CC371C32CE0576098";

fn packet_bytes(f: &Frame) -> &[u8] {
    match &f.payload {
        FramePayload::Packet(b) => b,
        _ => panic!("expected packet payload"),
    }
}

#[test]
fn crc_valid_frame_checksums_zero() {
    assert_eq!(crc::checksum(&hex(KLM1023)), 0);
}

#[test]
fn append_parity_reproduces_known_parity() {
    let mut frame = hex(KLM1023);
    crc::append_parity(&mut frame);
    assert_eq!(&frame[11..14], &[0x57, 0x60, 0x98]);
}

#[test]
fn encode_identification_matches_reference() {
    assert_eq!(encode_identification(0x4840D6, "KLM1023 ").to_vec(), hex(KLM1023));
}

#[test]
fn modes_fields_decode() {
    let bytes = hex(KLM1023);
    let m = ModeS::new(&bytes);
    assert_eq!(m.df(), 17);
    assert_eq!(m.ca(), 5);
    assert_eq!(m.icao(), 0x4840D6);
    assert_eq!(m.type_code(), Some(4));
    assert_eq!(m.callsign().as_deref(), Some("KLM1023"));
}

#[test]
fn ppm_roundtrip_multiple_rates() {
    let frame = hex(KLM1023);
    for spu in [2usize, 4, 8] {
        let wave = PpmModulator::new(spu).modulate_padded(&frame, 4, 4);
        let frames = PpmDemodulator::new(spu).scan(&wave, true).0;
        assert_eq!(frames.len(), 1, "spu={spu}");
        assert!(frames[0].crc_ok());
        assert_eq!(frames[0].bytes, frame, "spu={spu}");
    }
}

#[test]
fn mode_roundtrip_via_traits() {
    let frame = encode_identification(0x3C6444, "TEST42");
    let wave = AdsbMod::new().modulate(&Frame::packet(frame.to_vec())).unwrap();

    let mut demod = AdsbDemod::new();
    let mut frames = demod.feed(&wave);
    frames.extend(demod.flush());
    assert_eq!(frames.len(), 1);
    assert!(frames[0].meta.crc_ok);
    assert_eq!(packet_bytes(&frames[0]), &frame[..]);
    assert_eq!(ModeS::new(packet_bytes(&frames[0])).callsign().as_deref(), Some("TEST42"));
}

#[test]
fn streaming_recovers_frame_split_across_feeds() {
    let frame = hex(KLM1023);
    let wave = AdsbMod::new().modulate(&Frame::packet(frame.clone())).unwrap();

    // Split at an awkward point so the preamble+frame straddles two feeds.
    let mut demod = AdsbDemod::new();
    for cut in [1usize, wave.len() / 3, wave.len() / 2, 5] {
        demod.reset();
        let (a, b) = wave.split_at(cut.min(wave.len()));
        let mut frames = demod.feed(a);
        frames.extend(demod.feed(b));
        frames.extend(demod.flush());
        assert_eq!(frames.len(), 1, "cut={cut}");
        assert_eq!(packet_bytes(&frames[0]), &frame[..], "cut={cut}");
    }
}

#[test]
fn ensemble_dedups_single_frame_across_phases() {
    // A clean modulated frame lands on the integer grid, so every slicer phase
    // decodes it. The DedupWindow must collapse the copies to one emission.
    let frame = hex(KLM1023);
    let wave = PpmModulator::new(2).modulate_padded(&frame, 4, 4);
    for phases in [1usize, 2, 4, 6] {
        let frames = ParallelDemodulator::new(2, phases).scan(&wave, true).0;
        assert_eq!(frames.len(), 1, "phases={phases}");
        assert!(frames[0].crc_ok());
        assert_eq!(frames[0].bytes, frame, "phases={phases}");
    }
}

#[test]
fn ensemble_keeps_distinct_frames_far_apart() {
    // Two different frames spaced well beyond the dedup window must both survive
    // — the window only collapses cross-phase copies of one physical frame. A
    // quiet lead longer than the detector's warmup lets the floor seed on the
    // quiet region rather than the first burst (as it does off-air).
    let a = hex(KLM1023);
    let b = encode_identification(0x3C6444, "TEST42").to_vec();
    let modu = PpmModulator::new(2);
    let mut wave = vec![0.0f32; 256];
    wave.extend(modu.modulate_padded(&a, 4, 8));
    wave.extend(modu.modulate_padded(&b, 8, 4));
    let frames = ParallelDemodulator::new(2, 4).scan(&wave, true).0;
    assert!(frames.iter().any(|f| f.bytes == a), "frame a missing");
    assert!(frames.iter().any(|f| f.bytes == b), "frame b missing");
    // Each physical frame emitted exactly once despite four phases decoding it.
    assert_eq!(frames.iter().filter(|f| f.bytes == a).count(), 1);
    assert_eq!(frames.iter().filter(|f| f.bytes == b).count(), 1);
}

#[test]
fn ensemble_recovers_off_grid_frame_single_phase_misses() {
    // The core R3 win: a frame whose bit timing lands off the 2 MHz integer grid
    // that the single-phase slicer cannot decode, but a sub-sample phase does.
    //
    // Ideal square pulses can't demonstrate this — every phase reads the flat
    // pulse top, so a shift changes nothing. Off-air the pulses are band-limited,
    // so their energy smears across slot boundaries; then a fractional timing
    // offset pushes the integer-grid slot centers into the wrong side and the
    // slice flips. Reproduce that: modulate 8× oversampled, low-pass with a
    // ~1-sample moving average to round the edges, then sample at 2 MHz from a
    // ¾-sample offset. A quiet lead seeds the noise floor past its warmup.
    let frame = hex(KLM1023);
    let fine_spu = 16usize;
    let fine = PpmModulator::new(fine_spu).modulate_padded(&frame, 6, 6);
    let smooth = fine_spu; // ±8 fine taps ≈ one 2 MHz sample of edge rounding
    let rounded: Vec<f32> = (0..fine.len())
        .map(|i| {
            let lo = i.saturating_sub(smooth / 2);
            let hi = (i + smooth / 2).min(fine.len() - 1);
            fine[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32
        })
        .collect();
    let decim = fine_spu / 2; // 16× fine -> 2 MHz
    let phase_off = 6usize; // 6/8 of a 2 MHz sample — off the integer grid
    let mut wave = vec![0.0f32; 300];
    wave.extend(rounded.iter().skip(phase_off).step_by(decim).copied());

    // Isolate the R3 timing mechanism from the R4 soft-decision gate: this
    // deliberately over-smoothed frame is mushier (eye ~0.30) than any real
    // off-air frame on the reference recording (all ≥0.39), so the default gate
    // would reject it. Disable the gate here — the gate's own accept/reject is
    // covered by `soft_gate_*` below — so this asserts purely that a sub-sample
    // phase recovers the timing a single phase misses.
    let mut single = ParallelDemodulator::new(2, 1);
    single.set_min_confidence(0.0);
    assert!(
        !single.scan(&wave, true).0.iter().any(|f| f.crc_ok() && f.bytes == frame),
        "single-phase should miss the off-grid frame"
    );
    let mut ensemble = ParallelDemodulator::new(2, 4);
    ensemble.set_min_confidence(0.0);
    assert_eq!(
        ensemble.scan(&wave, true).0.iter().filter(|f| f.crc_ok() && f.bytes == frame).count(),
        1,
        "ensemble should recover it exactly once"
    );
}

/// Raise each data bit's empty half-slot to `fill`× the pulse level, closing the
/// PPM eye without flipping any bit (the pulse slot still dominates). Models the
/// mushy-eye envelope of a CRC-lucky ghost sliced out of near-noise: the frame
/// still checksums, but every bit is marginal.
fn mush_eye(wave: &mut [f32], lead_us: usize, nbits: usize, fill: f32) {
    let data_start = lead_us * 2 + PREAMBLE_SLOTS; // slot_len = 1 at spu=2
    for j in 0..nbits {
        let base = data_start + j * DATA_SLOTS_PER_BIT;
        let (a, b) = (wave[base], wave[base + 1]);
        let hi = a.max(b);
        let lo_idx = if a >= b { base + 1 } else { base };
        wave[lo_idx] = fill * hi;
    }
}

#[test]
fn soft_confidence_high_for_clean_frame() {
    // Ideal square pulses drive one slot to the pulse level and the other to
    // zero, so every bit's eye is 1.0 and the frame confidence is ~1.0 — well
    // clear of any gate.
    let frame = hex(KLM1023);
    let wave = PpmModulator::new(2).modulate_padded(&frame, 4, 4);
    let mut d = PpmDemodulator::new(2);
    d.min_confidence = 0.0;
    let frames = d.scan(&wave, true).0;
    assert_eq!(frames.len(), 1);
    assert!(frames[0].confidence > 0.99, "clean eye should be ~1.0, got {}", frames[0].confidence);
}

#[test]
fn soft_gate_rejects_mushy_frame_keeps_clean() {
    // The R4 accept/reject in isolation. A clean frame (eye 1.0) and a
    // bit-identical copy whose eye is closed to ~0.30 both still pass CRC. The
    // default gate keeps the clean one and rejects the mushy one; disabling the
    // gate keeps both — proving the discrimination is the gate, not the parity.
    let frame = hex(KLM1023);
    let clean = PpmModulator::new(2).modulate_padded(&frame, 4, 4);
    let mut mushy = clean.clone();
    mush_eye(&mut mushy, 4, LONG_FRAME_BITS, 0.7); // eye → (1 - 0.7) = 0.30, below 0.32

    // Gate off: both decode, and the mushy copy's confidence lands below the gate.
    let mut open = PpmDemodulator::new(2);
    open.min_confidence = 0.0;
    let clean_conf = open.scan(&clean, true).0[0].confidence;
    let mushy_frames = open.scan(&mushy, true).0;
    assert_eq!(mushy_frames.len(), 1, "mushy frame still checksums");
    assert_eq!(mushy_frames[0].bytes, frame, "no bit flipped — only the eye closed");
    assert!(mushy_frames[0].confidence < clean_conf);
    assert!(mushy_frames[0].confidence < ADSB_MIN_CONFIDENCE);

    // Gate on (default): clean survives, mushy is rejected.
    assert_eq!(PpmDemodulator::new(2).scan(&clean, true).0.len(), 1);
    assert!(PpmDemodulator::new(2).scan(&mushy, true).0.is_empty());
}

#[test]
fn soft_gate_keeps_fading_frame() {
    // The "fading aircraft" case the DFB-AGC exists for. Ramp the whole burst
    // from full amplitude down to 0.2 across its span — a heavy fade. Because the
    // AGC tracks the falling pulse level, each bit's eye stays wide and the frame
    // clears the gate; a fixed first-bit reference would divide the faded tail by
    // the loud head and score it near 0.2, dragging the mean under the threshold.
    let frame = hex(KLM1023);
    let mut wave = PpmModulator::new(2).modulate_padded(&frame, 4, 4);
    let lead = 4 * 2; // lead_us * samples_per_us
    let burst = PREAMBLE_SLOTS + LONG_FRAME_BITS * DATA_SLOTS_PER_BIT; // slot_len = 1 at spu=2
    for (k, s) in wave.iter_mut().enumerate().skip(lead).take(burst) {
        let scale = 1.0 - 0.8 * ((k - lead) as f32 / burst as f32);
        *s *= scale;
    }
    let frames = PpmDemodulator::new(2).scan(&wave, true).0; // gate on (default)
    assert_eq!(frames.len(), 1, "faded frame should still clear the gate");
    assert_eq!(frames[0].bytes, frame, "fade scales both slots — no bit flips");
    // ~0.89 in practice (the AGC lags the fall slightly); far above both the 0.32
    // gate and the ~0.6 mean a fixed first-bit reference would score on this ramp.
    assert!(
        frames[0].confidence > 0.85,
        "DFB-AGC should keep the fading eye wide, got {}",
        frames[0].confidence
    );
}

#[test]
fn short_frame_roundtrip() {
    // 56-bit short frame, non-extended DF (DF11 all-call). Valid parity, so the
    // demod must select the short length from the DF rather than over-reading.
    let mut frame = [0u8; 7];
    frame[0] = (11 << 3) | 0x05;
    frame[1] = 0x3C;
    frame[2] = 0x64;
    frame[3] = 0x44;
    crc::append_parity(&mut frame);
    assert_eq!(crc::checksum(&frame), 0);

    let wave = PpmModulator::new(2).modulate_padded(&frame, 4, 4);
    let frames = PpmDemodulator::new(2).scan(&wave, true).0;
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].df, 11);
    assert_eq!(frames[0].bytes.len(), 7);
    assert_eq!(frames[0].bytes, frame);
}

#[test]
fn encode_all_call_reply_roundtrips_via_traits() {
    let frame = encode_all_call_reply(0x3C6444, CA_LEVEL2);
    assert_eq!(crc::checksum(&frame), 0);

    let wave = AdsbMod::new().modulate(&Frame::packet(frame.to_vec())).unwrap();
    let mut demod = AdsbDemod::new();
    let mut frames = demod.feed(&wave);
    frames.extend(demod.flush());
    assert_eq!(frames.len(), 1);
    assert!(frames[0].meta.crc_ok);
    let out = packet_bytes(&frames[0]);
    assert_eq!(out.len(), 7);
    assert_eq!(ModeS::new(out).df(), 11);
    assert_eq!(ModeS::new(out).icao(), 0x3C6444);
}

#[test]
fn require_crc_gates_corrupted_frames() {
    let mut frame = encode_identification(0x484010, "GARBLED");
    frame[6] ^= 0x20; // flip a data bit -> CRC no longer clears
    assert_ne!(crc::checksum(&frame), 0);
    let wave = PpmModulator::new(2).modulate_padded(&frame, 4, 4);

    // Strict (default) drops it.
    assert!(PpmDemodulator::new(2).scan(&wave, true).0.is_empty());

    // Lenient returns it with a non-zero residual and intact bytes.
    let mut lenient = PpmDemodulator::new(2);
    lenient.require_crc = false;
    let frames = lenient.scan(&wave, true).0;
    assert_eq!(frames.len(), 1);
    assert!(!frames[0].crc_ok());
    assert_eq!(frames[0].bytes, frame.to_vec());
}

#[test]
fn callsign_preserves_embedded_space() {
    let frame = encode_identification(0x400000, "AB CD");
    assert_eq!(ModeS::new(&frame).callsign().as_deref(), Some("AB CD"));
}

#[test]
fn rejects_pure_noise() {
    let wave = vec![0.0f32; 4096];
    assert!(PpmDemodulator::new(2).scan(&wave, true).0.is_empty());
    let mut demod = AdsbDemod::new();
    assert!(demod.feed(&wave).is_empty());
}

#[test]
fn tolerates_leaked_energy_in_pulse_adjacent_slot() {
    // On a real off-air envelope the guard slot right next to a preamble pulse
    // carries leaked pulse energy. The old strict correlator required *every*
    // guard slot below every pulse, so a single elevated adjacent slot vetoed
    // the match; the noise-floor-relative detector ignores the pulse-adjacent
    // slots (they are not in PREAMBLE_QUIET_SLOTS) and still decodes the frame.
    let frame = hex(KLM1023);
    let mut wave = PpmModulator::new(2).modulate_padded(&frame, 4, 4);
    // Preamble starts after the 4 µs (8-sample) lead; slot 1 is one sample in,
    // adjacent to the pulse in slot 0. Lift it to the pulse level.
    wave[8 + 1] = 1.0;
    let frames = PpmDemodulator::new(2).scan(&wave, true).0;
    assert_eq!(frames.len(), 1);
    assert!(frames[0].crc_ok());
    assert_eq!(frames[0].bytes, frame);
}

#[test]
fn streaming_decodes_over_nonzero_noise_floor() {
    // Feed a frame sitting on a constant noise pedestal through AdsbDemod in
    // small chunks (so the burst straddles feed boundaries). The adaptive floor
    // persists across feeds and settles on the pedestal, so the per-pulse
    // (`p <= noise`) and mean (`high_mean <= noise * gate`) gates run against a
    // real, above-zero floor and the cross-feed continuity is exercised — the
    // path the reference recording validates but the one-shot tests do not.
    const PED: f32 = 0.05;
    let frame = hex(KLM1023);
    let body = PpmModulator::new(2).modulate_padded(&frame, 4, 4);
    // Deterministic low-amplitude noise on a small pedestal — non-flat, so it
    // resembles real off-air noise rather than a DC region. A quiet lead longer
    // than the detector warmup lets the floor seed on the noise before the burst.
    let noise = |k: usize| PED + 0.02 * (((k as u32).wrapping_mul(2654435761) >> 24) as f32 / 255.0);
    let mut wave: Vec<f32> = (0..512).map(noise).collect();
    wave.extend(body.iter().enumerate().map(|(k, &s)| s + noise(512 + k)));
    let tail = wave.len();
    wave.extend((0..512).map(|k| noise(tail + k)));

    let mut demod = AdsbDemod::new();
    let mut frames = Vec::new();
    for chunk in wave.chunks(300) {
        frames.extend(demod.feed(chunk));
    }
    frames.extend(demod.flush());
    assert_eq!(frames.len(), 1);
    assert!(frames[0].meta.crc_ok);
    assert_eq!(packet_bytes(&frames[0]), &frame[..]);
}

#[test]
fn rejects_preamble_with_loud_guard_slot() {
    // Energy in a *tested* guard slot (the gap before the data, slot 11) above
    // the ceiling means the four pulse positions aren't the dominant energy —
    // not a real preamble. The detector rejects it.
    let frame = hex(KLM1023);
    let mut wave = PpmModulator::new(2).modulate_padded(&frame, 4, 4);
    wave[8 + 11] = 2.0; // slot 11, well above QUIET_CEIL_RATIO × the pulse level
    assert!(PpmDemodulator::new(2).scan(&wave, true).0.is_empty());
}

#[test]
fn airborne_position_cpr_global_decode() {
    let even = hex("8D40621D58C382D690C8AC2863A7");
    let odd = hex("8D40621D58C386435CC412692AD6");
    let pe = ModeS::new(&even).airborne_position().unwrap();
    let po = ModeS::new(&odd).airborne_position().unwrap();
    assert!(!pe.odd);
    assert!(po.odd);

    let (lat, lon) = cpr_decode_airborne(&pe, &po, false).unwrap();
    assert!((lat - 52.2572).abs() < 1e-3, "lat={lat}");
    assert!((lon - 3.91937).abs() < 1e-3, "lon={lon}");
}

#[test]
fn airborne_altitude_decode() {
    let bytes = hex("8D40621D58C382D690C8AC2863A7");
    assert_eq!(ModeS::new(&bytes).airborne_position().unwrap().altitude, Some(38000));
}

#[test]
fn airborne_velocity_ground_speed_decode() {
    // mode-s.org subtype-1 ground-speed vector: 159.2 kt, 182.88 deg, -832 fpm.
    let v = ModeS::new(&hex("8D485020994409940838175B284F")).airborne_velocity().unwrap();
    assert!((v.ground_speed.unwrap() - 159.20).abs() < 0.1, "gs={:?}", v.ground_speed);
    assert!((v.track.unwrap() - 182.88).abs() < 0.1, "track={:?}", v.track);
    assert_eq!(v.vertical_rate, Some(-832));
}

#[test]
fn airborne_velocity_airspeed_heading_decode() {
    // pyModeS subtype-3 (airspeed + magnetic heading) vector: 375 kt, 243.98 deg,
    // -2304 fpm. Guards the ME bit-45 heading offset.
    let v = ModeS::new(&hex("8DA05F219B06B6AF189400CBC33F")).airborne_velocity().unwrap();
    assert!((v.ground_speed.unwrap() - 375.0).abs() < 0.5, "as={:?}", v.ground_speed);
    assert!((v.track.unwrap() - 243.98).abs() < 0.1, "hdg={:?}", v.track);
    assert_eq!(v.vertical_rate, Some(-2304));
}

// --- R5 Lever 1: native (higher-rate) decode -------------------------------

#[test]
fn native_rate_decode_roundtrips() {
    // Modulate at 4 samples/µs (the 4 MHz native-preserving working rate) and
    // decode with a demod built at that rate — the whole PPM stack is rate-
    // parameterized, so a frame round-trips at the native rate as it does at 2 MHz.
    let frame = hex(KLM1023);
    let wave = PpmModulator::new(4).modulate_padded(&frame, 4, 4);
    let mut demod =
        AdsbDemod::with_rate_phases_min_conf(ADSB_NATIVE_RATE, ADSB_SLICER_PHASES, ADSB_MIN_CONFIDENCE);
    let mut frames = demod.feed(&wave);
    frames.extend(demod.flush());
    assert_eq!(frames.len(), 1);
    assert!(frames[0].meta.crc_ok);
    assert_eq!(packet_bytes(&frames[0]), &frame[..]);
}

// --- R5 Lever 2a: single-bit CRC repair ------------------------------------

#[test]
fn single_bit_repair_recovers_frame() {
    let clean = hex(KLM1023);
    let mut corrupt = clean.clone();
    corrupt[7] ^= 0x04; // one data-bit flip -> parity no longer clears
    assert_ne!(crc::checksum(&corrupt), 0);
    let wave = PpmModulator::new(2).modulate_padded(&corrupt, 4, 4);

    // Default demod (repair off) gates the corrupted frame out entirely.
    let mut off = AdsbDemod::new();
    let mut none = off.feed(&wave);
    none.extend(off.flush());
    assert!(none.is_empty());

    // With repair on, the single-bit error is corrected back to the clean frame.
    let mut on = AdsbDemod::new();
    on.set_repair(true);
    let mut got = on.feed(&wave);
    got.extend(on.flush());
    assert_eq!(got.len(), 1);
    assert!(got[0].meta.crc_ok);
    assert_eq!(packet_bytes(&got[0]), &clean[..]);
}

// --- R5 Lever 2b: ICAO-roster-gated address-overlaid recovery --------------

/// An address-overlaid long frame (DF20, Comm-B) with arbitrary content, and the
/// CRC residual it checksums to — the value the roster gate reads as its ICAO.
fn overlaid_df20() -> (Vec<u8>, u32) {
    let mut f = vec![0u8; 14];
    f[0] = 20 << 3; // DF20
    f[1] = 0xE1;
    f[2] = 0x99;
    f[3] = 0x10;
    f[4] = 0x8D;
    f[5] = 0x27;
    f[6] = 0x4C;
    let r = crc::checksum(&f);
    (f, r)
}

#[test]
fn roster_gate_recovers_overlaid_frame_and_reports_address() {
    // Direct PpmDemodulator scan so the recovered address is observable. A flat
    // zero noise floor suffices for a clean full-amplitude modulated frame.
    let (frame, r) = overlaid_df20();
    assert_ne!(r, 0);
    let wave = PpmModulator::new(2).modulate_padded(&frame, 8, 8);
    let noise = vec![0.0f32; wave.len()];
    let demod = PpmDemodulator::new(2);

    // Empty roster: an address-overlaid frame has nothing to validate against,
    // so it is rejected — no fabricated aircraft from a CRC-overlay slice.
    let empty = IcaoRoster::new(8);
    assert!(demod.scan_with_floor(&wave, &noise, Some(&empty), true).0.is_empty());

    // Roster holding the frame's address: it is recovered, carrying the address.
    let mut roster = IcaoRoster::new(8);
    roster.note(r);
    let frames = demod.scan_with_floor(&wave, &noise, Some(&roster), true).0;
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].df, 20);
    assert_eq!(frames[0].crc_residual, r);
    assert_eq!(frames[0].recovered_icao, Some(r));
    assert!(frames[0].valid());
    assert!(!frames[0].crc_ok()); // valid by roster, not by a zero checksum
}

/// Stream one modulated frame through the demod, then a quiet tail long enough to
/// drain the decoded frame's samples out of the internal buffer (the ensemble
/// consumes only the phase-minimum per scan, so a frame lingers until quiet
/// scanning advances past it). Returns the frames emitted across the whole push —
/// so a subsequent frame in a later call is scanned against a clean buffer and the
/// roster this call populated.
fn stream_frame(demod: &mut AdsbDemod, wave: &[f32]) -> Vec<Frame> {
    let mut out = demod.feed(wave);
    out.extend(demod.feed(&vec![0.0f32; 4096]));
    out
}

#[test]
fn roster_gate_recovers_overlaid_frame_from_seen_address() {
    // End-to-end: a clean DF17 for address r seeds the roster in one feed, so the
    // address-overlaid DF20 whose residual is r is recovered in the next.
    let (overlaid, r) = overlaid_df20();
    assert_ne!(r, 0);
    let seed = encode_identification(r, "SEED").to_vec();
    let modu = PpmModulator::new(2);
    let seed_wave = modu.modulate_padded(&seed, 8, 8);
    let ov_wave = modu.modulate_padded(&overlaid, 8, 8);

    let mut demod = AdsbDemod::new();
    demod.set_roster(true);
    let s = stream_frame(&mut demod, &seed_wave);
    assert_eq!(s.len(), 1);
    assert_eq!(ModeS::new(packet_bytes(&s[0])).icao(), r);

    let mut got = stream_frame(&mut demod, &ov_wave);
    got.extend(demod.flush());
    assert_eq!(got.len(), 1);
    assert!(got[0].meta.crc_ok, "roster-confirmed overlaid frame is accepted");
    assert_eq!(ModeS::new(packet_bytes(&got[0])).df(), 20);
    assert_eq!(packet_bytes(&got[0]), &overlaid[..]);
}

#[test]
fn roster_gate_rejects_overlaid_frame_from_unseen_address() {
    // The roster holds a *different* address, so the overlaid frame stays gated —
    // no aircraft fabricated from a CRC-overlay slice of an unknown address.
    let (overlaid, r) = overlaid_df20();
    let other = (r ^ 0x01) & 0x00FF_FFFF;
    let seed = encode_identification(other, "OTHER").to_vec();
    let modu = PpmModulator::new(2);
    let seed_wave = modu.modulate_padded(&seed, 8, 8);
    let ov_wave = modu.modulate_padded(&overlaid, 8, 8);

    let mut demod = AdsbDemod::new();
    demod.set_roster(true);
    let s = stream_frame(&mut demod, &seed_wave);
    assert_eq!(s.len(), 1);

    let mut got = stream_frame(&mut demod, &ov_wave);
    got.extend(demod.flush());
    assert!(got.is_empty());
}
