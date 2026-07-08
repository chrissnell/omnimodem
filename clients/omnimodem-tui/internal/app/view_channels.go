package app

import (
	"fmt"
	"sort"

	"github.com/charmbracelet/bubbles/table"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

// dosTableStyles paints the channel table for the black DOS panel: a yellow
// header, white cells, and a white-on-bright-blue highlighted row.
func dosTableStyles() table.Styles {
	s := table.DefaultStyles()
	s.Header = s.Header.Foreground(ui.ColorTitle).Bold(true).BorderForeground(ui.ColorAccent)
	// Leave cells uncolored (plain text): they then inherit the panel's
	// white-on-black, and — crucially — emit no per-cell reset that would punch
	// holes in the Selected highlight bar, so the row-level style fills the row.
	s.Selected = lipgloss.NewStyle().Foreground(ui.ColorFg).Background(ui.ColorSel).Bold(true)
	return s
}

type channelsView struct {
	m *Model
	t table.Model
}

func newChannelsView(m *Model) *channelsView {
	cols := []table.Column{
		{Title: "CH", Width: 4}, {Title: "NAME", Width: 10}, {Title: "MODE", Width: 12},
		{Title: "DEVICE", Width: 22}, {Title: "PTT", Width: 4}, {Title: "RX dBFS", Width: 8},
	}
	t := table.New(table.WithColumns(cols), table.WithFocused(true), table.WithStyles(dosTableStyles()))
	v := &channelsView{m: m, t: t}
	v.refresh()
	return v
}

func (v *channelsView) refresh() {
	chs := make([]uint32, 0, len(v.m.live))
	for ch := range v.m.live {
		chs = append(chs, ch)
	}
	sort.Slice(chs, func(i, j int) bool { return chs[i] < chs[j] })
	rows := make([]table.Row, 0, len(chs))
	for _, ch := range chs {
		cl := v.m.live[ch]
		ptt := "▢"
		if cl.pttKeyed {
			ptt = "▣"
		}
		rows = append(rows, table.Row{
			fmt.Sprintf("CH%d", ch), orNone(cl.name), orNone(displayMode(cl.mode)),
			orDash(cl.deviceID), ptt, fmt.Sprintf("%.0f", cl.rxDbfs),
		})
	}
	v.t.SetRows(rows)
}

func (v *channelsView) Update(msg tea.Msg) (View, tea.Cmd) {
	v.refresh() // reflect live-state deltas
	if k, ok := msg.(tea.KeyMsg); ok {
		switch k.String() {
		case "q":
			if v.m.cancel != nil {
				v.m.cancel()
			}
			return v, tea.Quit
		case "n":
			// Add a channel: target the lowest free id and open Configure on it.
			// The daemon creates the channel when the bind is applied.
			v.m.sel = v.nextFreeChannel()
			v.m.push(newConfigView(v.m))
			return v, devicesCmd(v.m.c)
		case "c":
			v.m.sel = v.selectedChannel()
			v.m.push(newConfigView(v.m))
			return v, devicesCmd(v.m.c)
		case "o", "enter":
			v.m.sel = v.selectedChannel()
			v.m.push(newOperateView(v.m))
			return v, enableSpectrumCmd(v.m.c, v.m.sel, 64)
		}
	}
	var cmd tea.Cmd
	v.t, cmd = v.t.Update(msg)
	return v, cmd
}

// nextFreeChannel returns the lowest channel id not currently present, so a new
// channel fills gaps left by earlier ones (ch0, ch1, ch2 …).
func (v *channelsView) nextFreeChannel() uint32 {
	for ch := uint32(0); ; ch++ {
		if _, ok := v.m.live[ch]; !ok {
			return ch
		}
	}
}

func (v *channelsView) selectedChannel() uint32 {
	var ch uint32
	if r := v.t.SelectedRow(); len(r) > 0 {
		fmt.Sscanf(r[0], "CH%d", &ch)
	}
	return ch
}

func (v *channelsView) Render(w, h int) string {
	v.t.SetWidth(w)
	v.t.SetHeight(h)
	if len(v.t.Rows()) == 0 {
		return "No channels yet. Press <n> to add a channel."
	}
	return v.t.View()
}

func (v *channelsView) Title() string { return fmt.Sprintf("Channels (%d)", len(v.m.live)) }

func (v *channelsView) Hints() []ui.Hint {
	return []ui.Hint{
		{Key: "enter/o", Action: "operate"},
		{Key: "n", Action: "add"},
		{Key: "c", Action: "configure"},
		{Key: "q", Action: "quit"},
	}
}
