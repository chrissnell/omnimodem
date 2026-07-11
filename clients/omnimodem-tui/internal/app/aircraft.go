package app

import (
	"sort"
	"time"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// aircraftTTL mirrors the daemon's stale-contact TTL (core/adsb.rs
// DEFAULT_TTL_MS = 60s): a contact not reported within this window is dropped
// from the flights table. The daemon prunes on its own timer and then stops
// emitting, so the client ages rows out on the same clock to keep the two in
// step even after reports for an aircraft cease.
const aircraftTTL = 60 * time.Second

// aircraftLive is one tracked aircraft, folded from AircraftReport events keyed
// by ICAO. AircraftReport is LOSSY and additive: any single report may carry
// only the fields whose squitter just arrived (position needs a matched even/odd
// CPR pair; velocity and altitude their own frames), so fields are merged — a
// report that omits a value never clears one already decoded.
type aircraftLive struct {
	channel   uint32
	icao      uint32
	flight    string
	lat, lon  float64
	hasPos    bool
	altFt     int32
	hasAlt    bool
	gsKt      float64
	hasGS     bool
	lastHeard time.Time
}

// applyAircraft folds one AircraftReport into the per-ICAO aircraft map. now is
// the client receive time, used to age the contact out later — the report's
// last_seen_ms is the daemon's tracker clock, not comparable to wall time.
func (m *Model) applyAircraft(r *pb.AircraftReport, now time.Time) {
	a := m.aircraft[r.GetIcao()]
	if a == nil {
		a = &aircraftLive{icao: r.GetIcao()}
		m.aircraft[r.GetIcao()] = a
	}
	// Keyed by ICAO alone: an aircraft is heard on exactly one ADS-B channel
	// (one receiver per channel), so the channel is stored for the view's filter
	// rather than folded into the key.
	a.channel = r.GetChannel()
	if f := r.GetFlight(); f != "" {
		a.flight = f
	}
	if r.Latitude != nil && r.Longitude != nil {
		a.lat, a.lon, a.hasPos = r.GetLatitude(), r.GetLongitude(), true
	}
	if r.AltitudeFt != nil {
		a.altFt, a.hasAlt = r.GetAltitudeFt(), true
	}
	if r.GroundSpeedKt != nil {
		a.gsKt, a.hasGS = r.GetGroundSpeedKt(), true
	}
	a.lastHeard = now
	m.pruneAircraft(now)
}

// pruneAircraft drops contacts not heard from within aircraftTTL. Driven by the
// tick clock so stale flights vanish even after the daemon stops reporting them.
func (m *Model) pruneAircraft(now time.Time) {
	for icao, a := range m.aircraft {
		if now.Sub(a.lastHeard) > aircraftTTL {
			delete(m.aircraft, icao)
		}
	}
}

// aircraftForChannel returns the live contacts on one channel, sorted by flight
// then ICAO so rows hold a stable order as they update.
func (m *Model) aircraftForChannel(ch uint32) []*aircraftLive {
	out := make([]*aircraftLive, 0, len(m.aircraft))
	for _, a := range m.aircraft {
		if a.channel == ch {
			out = append(out, a)
		}
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].flight != out[j].flight {
			return out[i].flight < out[j].flight
		}
		return out[i].icao < out[j].icao
	})
	return out
}
