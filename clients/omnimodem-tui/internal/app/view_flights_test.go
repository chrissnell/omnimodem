package app

import (
	"fmt"
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

// End-to-end: an AircraftReport arriving as an event on Model.Update populates
// the flights table live — the acceptance path — without any per-view fetch.
func TestFlightsViewLiveUpdateFromEvent(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.stack = []View{newChannelsView(m)}
	m.sel = 2
	m.push(newFlightsView(m))

	m.Update(eventMsg{&pb.Event{Kind: &pb.Event_AircraftReport{AircraftReport: &pb.AircraftReport{
		Channel: 2, Icao: 0xABCDEF, Flight: "KLM1023",
		Latitude: f64(52.2572), Longitude: f64(3.91937),
		AltitudeFt: i32(38000), GroundSpeedKt: f64(420),
	}}}})

	out := m.top().Render(80, 20)
	if !strings.Contains(out, "KLM1023") || !strings.Contains(out, "52.2572") {
		t.Fatalf("an AircraftReport event must populate the flights table live; got:\n%s", out)
	}
}

// Render caps rows to the available height (ui.Table doesn't scroll), so a busy
// sky can't overflow the framed pane.
func TestFlightsViewCapsRowsToHeight(t *testing.T) {
	m := New(&client.Fake{}, "x")
	now := time.Unix(1_700_000_000, 0)
	for i := uint32(0); i < 30; i++ {
		m.applyAircraft(&pb.AircraftReport{Channel: 2, Icao: 0x400000 + i, Flight: fmt.Sprintf("FL%02d", i)}, now)
	}
	m.sel = 2
	// h=5 → header rule + 4 body rows; the sorted-first four show, the rest don't.
	out := newFlightsView(m).Render(80, 5)
	if !strings.Contains(out, "FL00") {
		t.Fatal("the first sorted contact must render")
	}
	if strings.Contains(out, "FL29") {
		t.Fatal("rows beyond the height budget must be dropped, not overflow the pane")
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

func TestVertArrow(t *testing.T) {
	cases := []struct {
		has  bool
		vr   int32
		want string
	}{
		{false, 0, ""},     // no velocity squitter yet
		{true, 0, ""},      // level
		{true, 100, ""},    // below threshold → level, not a flapping arrow
		{true, -100, ""},   // below threshold (descending slightly) → level
		{true, 1500, "↑"},  // climbing
		{true, -1500, "↓"}, // descending
	}
	for _, c := range cases {
		if got := vertArrow(c.has, c.vr); got != c.want {
			t.Errorf("vertArrow(%v, %d) = %q, want %q", c.has, c.vr, got, c.want)
		}
	}
}

func TestFmtAge(t *testing.T) {
	cases := []struct {
		d    time.Duration
		want string
	}{
		{-time.Second, "0s"}, // a report that just landed
		{0, "0s"},
		{45 * time.Second, "45s"},
		{92 * time.Second, "1m32s"},
		{(2*3600 + 5*60) * time.Second, "2h05m"},
	}
	for _, c := range cases {
		if got := fmtAge(c.d); got != c.want {
			t.Errorf("fmtAge(%v) = %q, want %q", c.d, got, c.want)
		}
	}
}

// A contact heard only once is low-confidence (likely a one-off mis-decode); one
// heard enough times to corroborate the ICAO is not.
func TestLowConfidenceUntilCorroborated(t *testing.T) {
	m := New(&client.Fake{}, "x")
	now := time.Unix(1_700_000_000, 0)

	m.applyAircraft(&pb.AircraftReport{Channel: 2, Icao: 0xA11111, Flight: "ONE"}, now)
	for i := 0; i < confidentAfterReports; i++ {
		m.applyAircraft(&pb.AircraftReport{Channel: 2, Icao: 0xB22222, Flight: "MANY"}, now)
	}

	if !lowConfidence(m.aircraft[0xA11111]) {
		t.Fatal("a single-report contact must be low-confidence")
	}
	if lowConfidence(m.aircraft[0xB22222]) {
		t.Fatal("a repeatedly-heard contact must not be low-confidence")
	}
}

// The PKTS column shows the daemon's per-plane packet count, and it tracks the
// latest report's cumulative total rather than counting client-side reports.
func TestFlightsViewPacketCount(t *testing.T) {
	m := New(&client.Fake{}, "x")
	now := time.Unix(1_700_000_000, 0)
	m.applyAircraft(&pb.AircraftReport{Channel: 2, Icao: 0xABCDEF, Flight: "CNT1", Messages: 7}, now)
	m.applyAircraft(&pb.AircraftReport{Channel: 2, Icao: 0xABCDEF, Flight: "CNT1", Messages: 42}, now)
	// A stale/reordered report with a lower total must not walk the count backward.
	m.applyAircraft(&pb.AircraftReport{Channel: 2, Icao: 0xABCDEF, Flight: "CNT1", Messages: 40}, now)

	m.sel = 2
	rows, _ := newFlightsView(m).rowsFlagged(now)
	// Columns: FLIGHT, LAT, LON, GS, ALT, V/S, PKTS, SEEN.
	if got := rows[0][6]; got != "42" {
		t.Errorf("PKTS column must show the daemon packet count, got %q (row %v)", got, rows[0])
	}
}

// The rendered rows must carry the climb/descend arrow, the last-seen age, and a
// low-confidence flag aligned with each row.
func TestFlightsViewVerticalTrendAgeAndFlag(t *testing.T) {
	m := New(&client.Fake{}, "x")
	now := time.Unix(1_700_000_000, 0)
	m.applyAircraft(&pb.AircraftReport{
		Channel: 2, Icao: 0xABCDEF, Flight: "CLIMB1",
		AltitudeFt: i32(10000), VertRateFpm: i32(1500),
	}, now)

	m.sel = 2
	rows, flagged := newFlightsView(m).rowsFlagged(now.Add(92 * time.Second))
	if len(rows) != 1 || len(flagged) != 1 {
		t.Fatalf("want 1 row, got %d rows / %d flags", len(rows), len(flagged))
	}
	// Columns: FLIGHT, LAT, LON, GS, ALT, V/S, PKTS, SEEN.
	if row := rows[0]; row[5] != "↑" {
		t.Errorf("V/S column must show the climb arrow, got %q (row %v)", row[5], row)
	} else if row[7] != "1m32s" {
		t.Errorf("SEEN column must show the age, got %q", row[7])
	}
	if !flagged[0] {
		t.Error("a single-report contact must be flagged low-confidence")
	}
}
