//! ADS-B decode regression gate.
//!
//! Decodes the committed reference clip (`testdata/adsb_ci_clip.iq`) through the
//! same path `adsb_bench` uses and asserts the yield stays at or above a floor.
//! The clip is a deterministic loopback recording (see `src/bin/make_fixture.rs`)
//! carrying 5 DF11 all-call replies and 3 DF17 identification squitters across
//! 3 aircraft — 8 CRC-valid frames. If a change to the demod, CRC, or message
//! path silently drops decodes, these assertions fail in CI.
//!
//! Floors, not exact matches: the point is to catch regressions (a broken
//! decoder yields ~0), not to pin the number. A change that *increases* yield is
//! fine and should ratchet the floor up here. The clip is stored at the 2 MHz
//! working rate, so decode at that `in_rate` (no resample) — see the generator's
//! header for why the 2.4 Msps capture rate is not used for this fixture.

use std::collections::BTreeMap;

use adsb_bench::decode_iq;

/// The fixture is generated at the working rate, so decode with no resample.
const IN_RATE: u32 = 2_000_000;

/// Full CRC-valid yield of the committed clip. The floors below sit at this
/// value: the clip is synthetic and lossless, so every frame must decode. Raise
/// these in lockstep if the fixture ever gains frames.
const EXPECT_FRAMES: u64 = 8;
const EXPECT_AIRCRAFT: usize = 3;
const EXPECT_DF11: u64 = 5;
const EXPECT_DF17: u64 = 3;

fn load_report() -> adsb_bench::Report {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/adsb_ci_clip.iq");
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"));
    decode_iq(&bytes, IN_RATE)
}

#[test]
fn frame_yield_meets_floor() {
    let r = load_report();
    assert!(
        r.frames_valid >= EXPECT_FRAMES,
        "ADS-B decode regressed: {} CRC-valid frames, floor is {EXPECT_FRAMES}. \
         The reference clip is lossless — a drop means the demod/CRC/message path broke.",
        r.frames_valid,
    );
    assert!(
        r.unique_aircraft() >= EXPECT_AIRCRAFT,
        "ADS-B decode regressed: {} unique aircraft, floor is {EXPECT_AIRCRAFT}.",
        r.unique_aircraft(),
    );
}

#[test]
fn downlink_format_mix_is_intact() {
    let r = load_report();
    let df: &BTreeMap<u8, u64> = &r.df_hist;
    assert!(
        df.get(&11).copied().unwrap_or(0) >= EXPECT_DF11,
        "DF11 all-call yield regressed: {:?} (floor {EXPECT_DF11})",
        df.get(&11),
    );
    assert!(
        df.get(&17).copied().unwrap_or(0) >= EXPECT_DF17,
        "DF17 extended-squitter yield regressed: {:?} (floor {EXPECT_DF17})",
        df.get(&17),
    );
    // No CRC-valid false positives: the clip only contains DF11 and DF17.
    let stray: Vec<u8> = df.keys().copied().filter(|d| !matches!(d, 11 | 17)).collect();
    assert!(stray.is_empty(), "unexpected CRC-valid downlink formats decoded: {stray:?}");
}
