//! ADS-B aircraft tracker: folds a stream of decoded Mode S frames into
//! per-aircraft state (callsign, position, altitude), pairing CPR even/odd
//! reports into a global position and aging out stale contacts.
//!
//! Pure and time-injected — the caller supplies a monotonic `now_ms` so this
//! has no clock dependency and unit-tests deterministically. The daemon owns an
//! instance, feeds it the ADS-B channel's decoded `Packet` frames, and turns
//! [`Aircraft`] snapshots into `AircraftReport` gRPC events (GRA-320).

use std::collections::HashMap;

use super::message::{cpr_decode_airborne, AirbornePosition, ModeS};

/// Public per-aircraft state — the projection the daemon/TUI render.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Aircraft {
    /// 24-bit ICAO address.
    pub icao: u32,
    /// Flight id / callsign from an identification message, if seen.
    pub callsign: Option<String>,
    /// Globally-decoded `(latitude, longitude)` in degrees, if an even/odd CPR
    /// pair has been resolved.
    pub position: Option<(f64, f64)>,
    /// Barometric altitude in feet, if seen.
    pub altitude_ft: Option<i32>,
    /// `now_ms` of the most recent frame from this aircraft.
    pub last_seen_ms: u64,
}

/// One buffered CPR half-report awaiting its complement.
#[derive(Clone, Copy)]
struct CprSlot {
    pos: AirbornePosition,
    ts: u64,
}

struct Entry {
    ac: Aircraft,
    even: Option<CprSlot>,
    odd: Option<CprSlot>,
}

/// Aggregates decoded Mode S frames into live aircraft state.
pub struct AircraftTracker {
    max_age_ms: u64,
    cpr_window_ms: u64,
    by_icao: HashMap<u32, Entry>,
}

impl AircraftTracker {
    /// `max_age_ms`: drop an aircraft not heard from within this window.
    /// `cpr_window_ms`: only pair an even+odd CPR report if their timestamps are
    /// within this window (positions decode from near-simultaneous frames).
    pub fn new(max_age_ms: u64, cpr_window_ms: u64) -> Self {
        AircraftTracker { max_age_ms, cpr_window_ms, by_icao: HashMap::new() }
    }

    /// Ingest a decoded Mode S frame at `now_ms`. Returns the ICAO when the
    /// frame was an extended squitter (DF17/18) this tracker understood, else
    /// `None`.
    pub fn ingest(&mut self, bytes: &[u8], now_ms: u64) -> Option<u32> {
        let m = ModeS::new(bytes);
        if !matches!(m.df(), 17 | 18) {
            return None;
        }
        let icao = m.icao();
        let cpr_window = self.cpr_window_ms;
        let entry = self.by_icao.entry(icao).or_insert_with(|| Entry {
            ac: Aircraft { icao, ..Default::default() },
            even: None,
            odd: None,
        });
        entry.ac.last_seen_ms = now_ms;

        match m.type_code() {
            Some(tc) if (1..=4).contains(&tc) => {
                if let Some(cs) = m.callsign() {
                    entry.ac.callsign = Some(cs);
                }
            }
            Some(_) => {
                if let Some(ap) = m.airborne_position() {
                    if let Some(alt) = ap.altitude {
                        entry.ac.altitude_ft = Some(alt);
                    }
                    let slot = CprSlot { pos: ap, ts: now_ms };
                    if ap.odd {
                        entry.odd = Some(slot);
                    } else {
                        entry.even = Some(slot);
                    }
                    if let (Some(e), Some(o)) = (entry.even, entry.odd) {
                        if e.ts.abs_diff(o.ts) <= cpr_window {
                            let newest_is_odd = o.ts >= e.ts;
                            if let Some(pos) = cpr_decode_airborne(&e.pos, &o.pos, newest_is_odd) {
                                entry.ac.position = Some(pos);
                            }
                        }
                    }
                }
            }
            None => {}
        }
        Some(icao)
    }

    /// Drop aircraft not seen within `max_age_ms` of `now_ms`.
    pub fn prune(&mut self, now_ms: u64) {
        let max = self.max_age_ms;
        self.by_icao.retain(|_, e| now_ms.saturating_sub(e.ac.last_seen_ms) <= max);
    }

    /// Snapshot of tracked aircraft, sorted by ICAO. Does not prune.
    pub fn aircraft(&self) -> Vec<Aircraft> {
        let mut v: Vec<Aircraft> = self.by_icao.values().map(|e| e.ac.clone()).collect();
        v.sort_by_key(|a| a.icao);
        v
    }

    /// Number of aircraft currently tracked.
    pub fn len(&self) -> usize {
        self.by_icao.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_icao.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::message::encode_identification;
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn tracks_callsign_from_identification() {
        let mut t = AircraftTracker::new(60_000, 10_000);
        let f = encode_identification(0x4840D6, "KLM1023 ");
        assert_eq!(t.ingest(&f, 1000), Some(0x4840D6));
        let ac = t.aircraft();
        assert_eq!(ac.len(), 1);
        assert_eq!(ac[0].icao, 0x4840D6);
        assert_eq!(ac[0].callsign.as_deref(), Some("KLM1023"));
        assert_eq!(ac[0].last_seen_ms, 1000);
    }

    #[test]
    fn pairs_cpr_into_position_and_altitude() {
        let mut t = AircraftTracker::new(60_000, 10_000);
        // Canonical pair (ICAO 40621D). Feed odd first, then even, so the newest
        // frame is even — matching the reference 52.2572 / 3.91937 anchoring.
        let odd = hex("8D40621D58C386435CC412692AD6");
        let even = hex("8D40621D58C382D690C8AC2863A7");
        assert_eq!(t.ingest(&odd, 0), Some(0x40621D));
        assert_eq!(t.ingest(&even, 100), Some(0x40621D));

        let ac = t.aircraft();
        assert_eq!(ac.len(), 1);
        let (lat, lon) = ac[0].position.expect("position resolved");
        assert!((lat - 52.2572).abs() < 1e-3, "lat={lat}");
        assert!((lon - 3.91937).abs() < 1e-3, "lon={lon}");
        assert_eq!(ac[0].altitude_ft, Some(38000));
    }

    #[test]
    fn cpr_not_paired_outside_window() {
        let mut t = AircraftTracker::new(600_000, 10_000);
        let odd = hex("8D40621D58C386435CC412692AD6");
        let even = hex("8D40621D58C382D690C8AC2863A7");
        t.ingest(&odd, 0);
        t.ingest(&even, 50_000); // 50 s apart > 10 s window
        assert_eq!(t.aircraft()[0].position, None);
    }

    #[test]
    fn ages_out_stale_aircraft() {
        let mut t = AircraftTracker::new(30_000, 10_000);
        let f = encode_identification(0x400000, "OLD1");
        t.ingest(&f, 1000);
        assert_eq!(t.len(), 1);
        t.prune(1000 + 30_000); // exactly at the edge: still kept
        assert_eq!(t.len(), 1);
        t.prune(1000 + 30_001); // one ms past: dropped
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn ignores_non_extended_squitter() {
        let mut t = AircraftTracker::new(60_000, 10_000);
        // DF11 short all-call frame — not DF17/18.
        let mut f = [0u8; 7];
        f[0] = (11 << 3) | 0x05;
        assert_eq!(t.ingest(&f, 1000), None);
        assert!(t.is_empty());
    }
}
