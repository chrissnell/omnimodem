package app

import (
	"fmt"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

// confidentAfterReports is how many AircraftReport events a contact must
// accumulate before it is trusted. Below it the row renders red: a mis-decoded
// frame mints a one-off random ICAO that never repeats, so a contact heard only
// a handful of times is likely a decode error, not a real aircraft. Real
// aircraft cross this bar within a couple of seconds (position updates nearly
// every squitter).
const confidentAfterReports = 3

// vsThresholdFpm is the vertical-rate magnitude (feet/min) a contact must exceed
// before the climb/descend arrow shows — above the ADS-B 64 fpm quantum so
// level-flight jitter reads as level, not a flapping arrow.
const vsThresholdFpm = 128

// flightsView is the ADS-B flights table: a live, read-only list of aircraft
// heard on one ADS-B channel, folded from AircraftReport events on the shared
// event stream. It issues no fetch of its own — reports already arrive on
// SubscribeEvents (opened at connect) — and simply renders the model's
// per-channel aircraft map, which the tick clock ages out. Columns are the ones
// the owner specified: flight, latitude, longitude, ground speed, altitude.
type flightsView struct {
	m  *Model
	ch uint32
}

func newFlightsView(m *Model) *flightsView {
	return &flightsView{m: m, ch: m.sel}
}

var flightsCols = []ui.Column{
	{Title: "FLIGHT", Width: 8},
	{Title: "LAT", Width: 9},
	{Title: "LON", Width: 10},
	{Title: "GS kt", Width: 6},
	{Title: "ALT ft", Width: 7},
	{Title: "V/S", Width: 3},
	{Title: "SEEN", Width: 6},
}

func (v *flightsView) Update(msg tea.Msg) (View, tea.Cmd) {
	if k, ok := msg.(tea.KeyMsg); ok {
		// esc pops back to the channels list, where `q` quits; a subview never
		// quits the app itself (only ctrl+c does), matching the SDR/operate views.
		if k.String() == "esc" {
			v.m.pop()
		}
	}
	return v, nil
}

// rowsFlagged builds the table rows and a parallel low-confidence flag per row
// (index-aligned with `aircraftForChannel`). `now` is the client clock used for
// the "last seen" age.
func (v *flightsView) rowsFlagged(now time.Time) ([][]string, []bool) {
	live := v.m.aircraftForChannel(v.ch)
	rows := make([][]string, 0, len(live))
	flagged := make([]bool, 0, len(live))
	for _, a := range live {
		rows = append(rows, []string{
			flightLabel(a),
			fmtCoord(a.hasPos, a.lat),
			fmtCoord(a.hasPos, a.lon),
			fmtMeasure(a.hasGS, a.gsKt),
			fmtInt(a.hasAlt, int64(a.altFt)),
			vertArrow(a.hasVR, a.vrFpm),
			fmtAge(now.Sub(a.lastHeard)),
		})
		flagged = append(flagged, lowConfidence(a))
	}
	return rows, flagged
}

func (v *flightsView) Render(w, h int) string {
	rows, flagged := v.rowsFlagged(time.Now())
	if len(rows) == 0 {
		return "No aircraft heard yet."
	}
	// The header rule eats one line; cap body rows to what's left so a busy sky
	// can't overflow the framed pane (ui.Table doesn't scroll).
	if max := h - 1; max > 0 && len(rows) > max {
		rows = rows[:max]
		flagged = flagged[:max]
	}
	return ui.TableInsetFlagged(flightsCols, rows, -1, flagged)
}

func (v *flightsView) Title() string {
	return fmt.Sprintf("Flights CH%d (%d)", v.ch, v.m.countForChannel(v.ch))
}

func (v *flightsView) Hints() []ui.Hint {
	return []ui.Hint{
		{Key: "esc", Action: "back"},
	}
}

// flightLabel is the aircraft's callsign, or its 24-bit ICAO in hex when no
// identification squitter has been heard yet, so every row stays identifiable.
func flightLabel(a *aircraftLive) string {
	if a.flight != "" {
		return a.flight
	}
	return fmt.Sprintf("%06X", a.icao)
}

// fmtCoord renders a lat/lon degree value, or an em-dash until a CPR pair has
// fixed the position.
func fmtCoord(has bool, deg float64) string {
	if !has {
		return "—"
	}
	return fmt.Sprintf("%.4f", deg)
}

func fmtMeasure(has bool, v float64) string {
	if !has {
		return "—"
	}
	return fmt.Sprintf("%.0f", v)
}

func fmtInt(has bool, v int64) string {
	if !has {
		return "—"
	}
	return fmt.Sprintf("%d", v)
}

// vertArrow renders the climb/descend trend from the barometric vertical rate:
// ↑ climbing, ↓ descending, blank when level (or no velocity squitter yet).
func vertArrow(hasVR bool, vrFpm int32) string {
	switch {
	case !hasVR:
		return ""
	case vrFpm > vsThresholdFpm:
		return "↑"
	case vrFpm < -vsThresholdFpm:
		return "↓"
	default:
		return ""
	}
}

// fmtAge renders how long ago a contact was last heard, compactly: "45s",
// "1m32s", "2h05m". Negative/zero ages (a report that just landed) read "0s".
func fmtAge(d time.Duration) string {
	if d < 0 {
		d = 0
	}
	secs := int(d.Seconds())
	switch {
	case secs < 60:
		return fmt.Sprintf("%ds", secs)
	case secs < 3600:
		return fmt.Sprintf("%dm%02ds", secs/60, secs%60)
	default:
		return fmt.Sprintf("%dh%02dm", secs/3600, (secs%3600)/60)
	}
}

// lowConfidence marks a contact heard too few times to trust (likely a decode
// error): its row renders red. See confidentAfterReports.
func lowConfidence(a *aircraftLive) bool {
	return a.reports < confidentAfterReports
}
