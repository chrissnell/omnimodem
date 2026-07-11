package app

import (
	"fmt"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

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

func (v *flightsView) rows() [][]string {
	live := v.m.aircraftForChannel(v.ch)
	rows := make([][]string, 0, len(live))
	for _, a := range live {
		rows = append(rows, []string{
			flightLabel(a),
			fmtCoord(a.hasPos, a.lat),
			fmtCoord(a.hasPos, a.lon),
			fmtMeasure(a.hasGS, a.gsKt),
			fmtInt(a.hasAlt, int64(a.altFt)),
		})
	}
	return rows
}

func (v *flightsView) Render(w, h int) string {
	rows := v.rows()
	if len(rows) == 0 {
		return "No aircraft heard yet."
	}
	// The header rule eats one line; cap body rows to what's left so a busy sky
	// can't overflow the framed pane (ui.Table doesn't scroll).
	if max := h - 1; max > 0 && len(rows) > max {
		rows = rows[:max]
	}
	return ui.TableInset(flightsCols, rows, -1)
}

func (v *flightsView) Title() string {
	return fmt.Sprintf("Flights CH%d (%d)", v.ch, len(v.m.aircraftForChannel(v.ch)))
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
