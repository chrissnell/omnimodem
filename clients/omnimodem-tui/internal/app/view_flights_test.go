package app

import (
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func f64(v float64) *float64 { return &v }
func i32(v int32) *int32     { return &v }

// AircraftReport is additive: a position-only report followed by a velocity/
// altitude-only report (no position) must merge into one contact, not clobber
// the earlier fields. Callsign, once heard, survives a later report that omits
// it.
func TestApplyAircraftMergesFields(t *testing.T) {
	m := New(&client.Fake{}, "x")
	now := time.Unix(1_700_000_000, 0)

	// First: identity + a fixed position.
	m.applyAircraft(&pb.AircraftReport{
		Channel: 3, Icao: 0xABCDEF, Flight: "KLM1023",
		Latitude: f64(52.2572), Longitude: f64(3.91937), AltitudeFt: i32(38000),
	}, now)
	// Then: velocity only, no position, empty flight.
	m.applyAircraft(&pb.AircraftReport{
		Channel: 3, Icao: 0xABCDEF, GroundSpeedKt: f64(420),
	}, now.Add(time.Second))

	a := m.aircraft[0xABCDEF]
	if a == nil {
		t.Fatal("aircraft must be tracked")
	}
	if a.flight != "KLM1023" {
		t.Fatalf("callsign must survive a later report that omits it, got %q", a.flight)
	}
	if !a.hasPos || a.lat != 52.2572 || a.lon != 3.91937 {
		t.Fatalf("position must survive a velocity-only report, got %+v", a)
	}
	if !a.hasAlt || a.altFt != 38000 {
		t.Fatalf("altitude must be retained, got %+v", a)
	}
	if !a.hasGS || a.gsKt != 420 {
		t.Fatalf("ground speed must be folded in, got %+v", a)
	}
}

// A contact not heard within the TTL is pruned; one heard recently is kept.
func TestPruneAircraftAgesOut(t *testing.T) {
	m := New(&client.Fake{}, "x")
	now := time.Unix(1_700_000_000, 0)
	m.applyAircraft(&pb.AircraftReport{Channel: 0, Icao: 0xAAAAAA}, now)

	m.pruneAircraft(now.Add(aircraftTTL - time.Second))
	if _, ok := m.aircraft[0xAAAAAA]; !ok {
		t.Fatal("a fresh contact must not be pruned")
	}
	m.pruneAircraft(now.Add(aircraftTTL + time.Second))
	if _, ok := m.aircraft[0xAAAAAA]; ok {
		t.Fatal("a stale contact must be pruned")
	}
}

// The flights view renders a row per contact on its channel, with the callsign,
// position, and speed/altitude columns; a contact with no callsign yet shows its
// ICAO in hex so the row stays identifiable.
func TestFlightsViewRendersRows(t *testing.T) {
	m := New(&client.Fake{}, "x")
	now := time.Unix(1_700_000_000, 0)
	m.applyAircraft(&pb.AircraftReport{
		Channel: 2, Icao: 0xABCDEF, Flight: "KLM1023",
		Latitude: f64(52.2572), Longitude: f64(3.91937),
		AltitudeFt: i32(38000), GroundSpeedKt: f64(420),
	}, now)
	m.applyAircraft(&pb.AircraftReport{Channel: 2, Icao: 0x484200}, now) // no ident yet

	m.sel = 2
	v := newFlightsView(m)
	out := v.Render(80, 20)

	for _, want := range []string{"KLM1023", "52.2572", "3.9194", "420", "38000", "484200"} {
		if !strings.Contains(out, want) {
			t.Fatalf("flights table must contain %q; got:\n%s", want, out)
		}
	}
	// The unidentified 0x484200 contact carries no position/speed/altitude, so
	// those columns must render the em-dash placeholder rather than a zero value.
	if !strings.Contains(out, "—") {
		t.Fatalf("absent fields must render as an em-dash placeholder; got:\n%s", out)
	}
}

// The view only lists contacts on its own channel.
func TestFlightsViewFiltersByChannel(t *testing.T) {
	m := New(&client.Fake{}, "x")
	now := time.Unix(1_700_000_000, 0)
	m.applyAircraft(&pb.AircraftReport{Channel: 2, Icao: 0x111111, Flight: "ONCH2"}, now)
	m.applyAircraft(&pb.AircraftReport{Channel: 5, Icao: 0x222222, Flight: "ONCH5"}, now)

	m.sel = 2
	out := newFlightsView(m).Render(80, 20)
	if !strings.Contains(out, "ONCH2") {
		t.Fatal("channel 2 view must show its own contact")
	}
	if strings.Contains(out, "ONCH5") {
		t.Fatal("channel 2 view must not show a channel 5 contact")
	}
}

// esc pops the flights view off the stack, returning to the channels list.
func TestFlightsViewEscPops(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.stack = []View{newChannelsView(m)}
	m.sel = 2
	m.push(newFlightsView(m))
	if _, ok := m.top().(*flightsView); !ok {
		t.Fatal("flights view must be on top after push")
	}
	v := m.top()
	v.Update(tea.KeyMsg{Type: tea.KeyEsc})
	if _, ok := m.top().(*channelsView); !ok {
		t.Fatal("esc must pop back to the channels view")
	}
}
