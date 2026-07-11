//! ADS-B mode tests, including canonical Mode S vectors and streaming behavior.

use super::message::{cpr_decode_airborne, encode_identification, ModeS};
use super::ppm::{PpmDemodulator, PpmModulator};
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
