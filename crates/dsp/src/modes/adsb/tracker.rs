//! ICAO-keyed aircraft tracker: fold decoded Mode S extended squitters into
//! per-aircraft state (callsign, position, altitude, velocity) and age out
//! contacts that stop transmitting.
//!
//! The tracker is **pure and time-injected**: every [`AircraftTracker::ingest`]
//! and [`AircraftTracker::prune`] takes a caller-supplied monotonic clock in
//! milliseconds, so the pairing/aging logic is deterministic and unit-testable
//! without a real clock. The daemon side (`core::adsb`) owns the wall clock and
//! turns each reportable change into an `AircraftReport` event.
//!
//! Position needs a global CPR solve, which requires an even and an odd frame
//! close together in time (see [`CPR_PAIR_WINDOW_MS`]); until both arrive an
//! aircraft is still tracked and reported, just without a lat/lon.

use super::message::ModeS;
use super::AirbornePosition;
use std::collections::HashMap;

/// A monotonic clock value in milliseconds, supplied by the caller.
pub type Millis = u64;

/// Maximum age difference between the even and odd CPR frames of a pair. Beyond
/// this the two halves may have moved into different latitude zones, so they are
/// not solved together.
pub const CPR_PAIR_WINDOW_MS: Millis = 10_000;

/// Public per-aircraft state, emitted on change and pruned when stale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Aircraft {
    /// 24-bit ICAO address.
    pub icao: u32,
    /// Callsign / flight id from an identification squitter (TC 1-4).
    pub callsign: Option<String>,
    /// Latitude in degrees once an even/odd CPR pair has been solved.
    pub latitude: Option<f64>,
    /// Longitude in degrees once an even/odd CPR pair has been solved.
    pub longitude: Option<f64>,
    /// Barometric altitude in feet (TC 9-18).
    pub altitude_ft: Option<i32>,
    /// Ground speed in knots (TC 19).
    pub ground_speed_kt: Option<f64>,
    /// Track angle, degrees clockwise from true north (TC 19).
    pub track_deg: Option<f64>,
    /// Barometric vertical rate in feet/min, + climb / - descent (TC 19).
    pub vertical_rate_fpm: Option<i32>,
    /// Clock value (ms) of the most recent decode for this aircraft.
    pub last_seen_ms: Millis,
}

/// Working state for one aircraft: its public snapshot plus the pending CPR
/// halves awaiting a partner.
struct Track {
    ac: Aircraft,
    even: Option<(AirbornePosition, Millis)>,
    odd: Option<(AirbornePosition, Millis)>,
}

/// Outcome of ingesting one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ingest {
    /// A tracked aircraft's reportable state changed; emit a fresh report. Holds
    /// the ICAO address so the caller can look the aircraft up with [`get`].
    ///
    /// [`get`]: AircraftTracker::get
    Updated(u32),
    /// A recognized squitter that added nothing new (e.g. an exact duplicate or a
    /// lone CPR half that could not yet be solved).
    Unchanged(u32),
    /// Not a frame the tracker folds in (wrong DF, non-ADS-B type code, garbage).
    Ignored,
}

/// ICAO-keyed store of live aircraft.
#[derive(Default)]
pub struct AircraftTracker {
    tracks: HashMap<u32, Track>,
}

impl AircraftTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one decoded Mode S frame (raw message bytes, parity already verified
    /// by the demod) into per-aircraft state at time `now_ms`. Only DF17/18
    /// extended squitters are tracked.
    pub fn ingest(&mut self, frame: &[u8], now_ms: Millis) -> Ingest {
        // Long extended squitters are 14 bytes; anything shorter has no ME field.
        if frame.len() < 14 {
            return Ingest::Ignored;
        }
        let msg = ModeS::new(frame);
        // DF17 is a genuine ADS-B squitter. DF18 (TIS-B/ADS-R) reuses the same
        // ME layout but its CA byte is a Control Field; some CF values carry no
        // real ICAO position. TODO(phase2): gate DF18 on CF once the live SDR
        // feed can surface them — the loopback/canonical path is DF17 only.
        if !matches!(msg.df(), 17 | 18) {
            return Ingest::Ignored;
        }
        let Some(tc) = msg.type_code() else {
            return Ingest::Ignored;
        };
        let icao = msg.icao();

        // Recognize the message before touching the map, so an unknown TC never
        // creates an empty track. TC 1-4 = identification, 9-18/20-22 = airborne
        // position, 19 = airborne velocity.
        if !matches!(tc, 1..=4 | 9..=22) {
            return Ingest::Ignored;
        }

        let track = self.tracks.entry(icao).or_insert_with(|| Track {
            ac: Aircraft { icao, ..Default::default() },
            even: None,
            odd: None,
        });
        let before = track.ac.clone();
        track.ac.last_seen_ms = now_ms;

        match tc {
            1..=4 => {
                track.ac.callsign = msg.callsign();
            }
            9..=18 | 20..=22 => {
                if let Some(pos) = msg.airborne_position() {
                    // Only TC 9-18 carry a barometric AC12 altitude. TC 20-22
                    // encode a GNSS height in metres, which `decode_ac12` would
                    // misread — take the position but leave altitude untouched.
                    if (9..=18).contains(&tc) {
                        if let Some(alt) = pos.altitude {
                            track.ac.altitude_ft = Some(alt);
                        }
                    }
                    Self::solve_position(track, pos, now_ms);
                }
            }
            19 => {
                if let Some(vel) = msg.airborne_velocity() {
                    if let Some(gs) = vel.ground_speed {
                        track.ac.ground_speed_kt = Some(gs);
                    }
                    if let Some(t) = vel.track {
                        track.ac.track_deg = Some(t);
                    }
                    if let Some(vr) = vel.vertical_rate {
                        track.ac.vertical_rate_fpm = Some(vr);
                    }
                }
            }
            _ => unreachable!("recognized gate above"),
        }

        if reportable_change(&before, &track.ac) {
            Ingest::Updated(icao)
        } else {
            Ingest::Unchanged(icao)
        }
    }

    /// Store the new CPR half and, if its partner is present and fresh, solve a
    /// global position into the aircraft's lat/lon.
    fn solve_position(track: &mut Track, pos: AirbornePosition, now_ms: Millis) {
        if pos.odd {
            track.odd = Some((pos, now_ms));
        } else {
            track.even = Some((pos, now_ms));
        }
        let (Some((even, t_even)), Some((odd, t_odd))) = (track.even, track.odd) else {
            return;
        };
        if t_even.abs_diff(t_odd) > CPR_PAIR_WINDOW_MS {
            return;
        }
        let newest_is_odd = t_odd >= t_even;
        if let Some((lat, lon)) = super::cpr_decode_airborne(&even, &odd, newest_is_odd) {
            track.ac.latitude = Some(lat);
            track.ac.longitude = Some(lon);
        }
    }

    /// Latest state for an aircraft, if tracked.
    pub fn get(&self, icao: u32) -> Option<&Aircraft> {
        self.tracks.get(&icao).map(|t| &t.ac)
    }

    /// Every currently-tracked aircraft.
    pub fn iter(&self) -> impl Iterator<Item = &Aircraft> {
        self.tracks.values().map(|t| &t.ac)
    }

    /// Number of tracked aircraft.
    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Drop aircraft not heard from within `ttl_ms` of `now_ms`. Returns the
    /// ICAO addresses removed, so the caller can retire them from any display.
    pub fn prune(&mut self, now_ms: Millis, ttl_ms: Millis) -> Vec<u32> {
        let mut removed = Vec::new();
        self.tracks.retain(|&icao, t| {
            let keep = now_ms.saturating_sub(t.ac.last_seen_ms) <= ttl_ms;
            if !keep {
                removed.push(icao);
            }
            keep
        });
        removed
    }
}

/// True when a reportable field (anything but `last_seen_ms`) differs, so a lone
/// keepalive or exact-duplicate squitter does not churn the event stream.
fn reportable_change(before: &Aircraft, after: &Aircraft) -> bool {
    before.callsign != after.callsign
        || before.latitude != after.latitude
        || before.longitude != after.longitude
        || before.altitude_ft != after.altitude_ft
        || before.ground_speed_kt != after.ground_speed_kt
        || before.track_deg != after.track_deg
        || before.vertical_rate_fpm != after.vertical_rate_fpm
}

#[cfg(test)]
mod tests {
    use super::super::message::encode_identification;
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    // Canonical airborne-position pair (ICAO 40621D), mode-s.org: 52.2572 /
    // 3.91937 at 38000 ft.
    const POS_EVEN: &str = "8D40621D58C382D690C8AC2863A7";
    const POS_ODD: &str = "8D40621D58C386435CC412692AD6";

    #[test]
    fn cpr_pair_yields_position_altitude_and_reports_on_change() {
        let mut t = AircraftTracker::new();
        // First (odd) half: recognized, but no position yet.
        assert_eq!(t.ingest(&hex(POS_ODD), 1_000), Ingest::Updated(0x40621D));
        assert!(t.get(0x40621D).unwrap().latitude.is_none());

        // Even half arrives newest, so it anchors the solve at the canonical
        // even-frame position (mode-s.org: 52.2572 / 3.91937, 38000 ft).
        assert_eq!(t.ingest(&hex(POS_EVEN), 1_500), Ingest::Updated(0x40621D));
        let ac = t.get(0x40621D).unwrap();
        assert_eq!(ac.altitude_ft, Some(38000));
        let lat = ac.latitude.unwrap();
        let lon = ac.longitude.unwrap();
        assert!((lat - 52.2572).abs() < 1e-3, "lat={lat}");
        assert!((lon - 3.91937).abs() < 1e-3, "lon={lon}");
        assert_eq!(ac.last_seen_ms, 1_500);
    }

    #[test]
    fn identification_populates_callsign() {
        let mut t = AircraftTracker::new();
        let frame = encode_identification(0x40621D, "KLM1023");
        assert_eq!(t.ingest(&frame, 0), Ingest::Updated(0x40621D));
        assert_eq!(t.get(0x40621D).unwrap().callsign.as_deref(), Some("KLM1023"));
    }

    #[test]
    fn velocity_populates_speed_track_vertical_rate() {
        // mode-s.org ground-speed example: 159 kt, 182.88 deg, -832 ft/min.
        let mut t = AircraftTracker::new();
        assert_eq!(t.ingest(&hex("8D485020994409940838175B284F"), 0), Ingest::Updated(0x485020));
        let ac = t.get(0x485020).unwrap();
        assert!((ac.ground_speed_kt.unwrap() - 159.2).abs() < 0.5, "gs={:?}", ac.ground_speed_kt);
        assert!((ac.track_deg.unwrap() - 182.88).abs() < 0.1, "track={:?}", ac.track_deg);
        assert_eq!(ac.vertical_rate_fpm, Some(-832));
    }

    #[test]
    fn duplicate_frame_is_unchanged() {
        let mut t = AircraftTracker::new();
        let frame = encode_identification(0x484010, "DUP");
        assert_eq!(t.ingest(&frame, 0), Ingest::Updated(0x484010));
        assert_eq!(t.ingest(&frame, 100), Ingest::Unchanged(0x484010));
    }

    #[test]
    fn stale_cpr_half_does_not_solve() {
        let mut t = AircraftTracker::new();
        t.ingest(&hex(POS_EVEN), 0);
        // Odd half arrives well beyond the pairing window: no position.
        t.ingest(&hex(POS_ODD), CPR_PAIR_WINDOW_MS + 1);
        assert!(t.get(0x40621D).unwrap().latitude.is_none());
    }

    #[test]
    fn prune_ages_out_stale_contacts() {
        let mut t = AircraftTracker::new();
        t.ingest(&encode_identification(0xAAAAAA, "OLD"), 0);
        t.ingest(&encode_identification(0xBBBBBB, "NEW"), 50_000);
        let removed = t.prune(61_000, 60_000);
        assert_eq!(removed, vec![0xAAAAAA]);
        assert!(t.get(0xAAAAAA).is_none());
        assert!(t.get(0xBBBBBB).is_some());
    }

    #[test]
    fn non_adsb_frames_are_ignored() {
        let mut t = AircraftTracker::new();
        // DF11 all-call (short frame) — not an extended squitter.
        assert_eq!(t.ingest(&[0x5D, 0x3C, 0x64, 0x44, 0, 0, 0], 0), Ingest::Ignored);
        assert!(t.is_empty());
    }
}
