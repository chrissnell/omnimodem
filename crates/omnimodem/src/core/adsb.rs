//! ADS-B channel post-processor: fold an ADS-B channel's decoded Mode S packets
//! into per-aircraft state and surface each reportable change as an
//! `AircraftReport` telemetry event.
//!
//! This wraps the pure [`AircraftTracker`] from `omnimodem-dsp` with the daemon
//! concerns it stays clear of: the channel id, the `AircraftReport` event shape,
//! and a stale-contact TTL. It remains time-injected — the caller passes the
//! clock (ms) on each packet and prune — so it is unit-testable with synthetic
//! frames. Phase 2 owns the wall clock: it spawns one `AdsbReporter` per ADS-B
//! channel, feeds it the channel's decoded `Packet` frames, and prunes on a
//! timer.

use crate::core::event::TelemetryEvent;
use crate::ids::ChannelId;
use omnimodem_dsp::modes::adsb::{Aircraft, AircraftTracker, Ingest};

/// Default age-out for a contact that stops transmitting (60 s: several missed
/// squitter intervals, so a briefly-faded aircraft is not dropped).
pub const DEFAULT_TTL_MS: u64 = 60_000;

/// Per-channel ADS-B reporter: drives an [`AircraftTracker`] and mints
/// `AircraftReport` telemetry.
pub struct AdsbReporter {
    channel: ChannelId,
    tracker: AircraftTracker,
    ttl_ms: u64,
}

impl AdsbReporter {
    /// A reporter for `channel` that ages contacts out after `ttl_ms`.
    pub fn new(channel: ChannelId, ttl_ms: u64) -> Self {
        Self { channel, tracker: AircraftTracker::new(), ttl_ms }
    }

    /// Ingest one decoded Mode S packet (raw message bytes, CRC already checked
    /// by the demod) at time `now_ms`. Returns an `AircraftReport` telemetry
    /// event when the aircraft's reportable state changed, else `None`.
    pub fn on_packet(&mut self, bytes: &[u8], now_ms: u64) -> Option<TelemetryEvent> {
        match self.tracker.ingest(bytes, now_ms) {
            Ingest::Updated(icao) => {
                self.tracker.get(icao).map(|ac| report(self.channel, ac))
            }
            Ingest::Unchanged(_) | Ingest::Ignored => None,
        }
    }

    /// Drop contacts not heard from within the TTL. Returns the ICAO addresses
    /// retired, so the caller can clear them from any display. There is no
    /// removal event on the wire in v1 — a client ages a contact out itself
    /// once its `last_seen_ms` falls behind.
    pub fn prune(&mut self, now_ms: u64) -> Vec<u32> {
        self.tracker.prune(now_ms, self.ttl_ms)
    }
}

/// Project a tracked [`Aircraft`] onto the `AircraftReport` telemetry event.
fn report(channel: ChannelId, ac: &Aircraft) -> TelemetryEvent {
    TelemetryEvent::AircraftReport {
        channel,
        icao: ac.icao,
        callsign: ac.callsign.clone(),
        latitude: ac.latitude,
        longitude: ac.longitude,
        altitude_ft: ac.altitude_ft,
        ground_speed_kt: ac.ground_speed_kt,
        track_deg: ac.track_deg,
        vertical_rate_fpm: ac.vertical_rate_fpm,
        last_seen_ms: ac.last_seen_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnimodem_dsp::modes::adsb::encode_identification;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn cpr_pair_emits_a_positioned_aircraft_report() {
        let mut r = AdsbReporter::new(ChannelId(2), DEFAULT_TTL_MS);
        // Odd half first: report before a position can be solved.
        let ev = r.on_packet(&hex("8D40621D58C386435CC412692AD6"), 1_000).unwrap();
        let TelemetryEvent::AircraftReport { channel, icao, latitude, .. } = ev else {
            panic!("expected AircraftReport");
        };
        assert_eq!(channel, ChannelId(2));
        assert_eq!(icao, 0x40621D);
        assert!(latitude.is_none());

        // Even half arrives newest and solves the canonical position + altitude.
        let ev = r.on_packet(&hex("8D40621D58C382D690C8AC2863A7"), 1_200).unwrap();
        let TelemetryEvent::AircraftReport { latitude, longitude, altitude_ft, .. } = ev else {
            panic!("expected AircraftReport");
        };
        assert_eq!(altitude_ft, Some(38000));
        assert!((latitude.unwrap() - 52.2572).abs() < 1e-3);
        assert!((longitude.unwrap() - 3.91937).abs() < 1e-3);
    }

    #[test]
    fn identification_report_carries_the_callsign() {
        let mut r = AdsbReporter::new(ChannelId(0), DEFAULT_TTL_MS);
        let ev = r.on_packet(&encode_identification(0x484010, "TEST42"), 0).unwrap();
        let TelemetryEvent::AircraftReport { callsign, .. } = ev else {
            panic!("expected AircraftReport");
        };
        assert_eq!(callsign.as_deref(), Some("TEST42"));
    }

    #[test]
    fn duplicate_packet_emits_no_report() {
        let mut r = AdsbReporter::new(ChannelId(0), DEFAULT_TTL_MS);
        let f = encode_identification(0x484010, "DUP");
        assert!(r.on_packet(&f, 0).is_some());
        assert!(r.on_packet(&f, 100).is_none());
    }

    #[test]
    fn prune_retires_stale_contacts() {
        let mut r = AdsbReporter::new(ChannelId(0), DEFAULT_TTL_MS);
        r.on_packet(&encode_identification(0xAAAAAA, "OLD"), 0);
        assert_eq!(r.prune(DEFAULT_TTL_MS + 1), vec![0xAAAAAA]);
        assert!(r.prune(DEFAULT_TTL_MS + 2).is_empty());
    }
}
