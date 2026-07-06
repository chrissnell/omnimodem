package app

import (
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
)

// The picker fills the whole operate surface. It must not render taller than the
// terminal — an over-tall modal scrolls the frame (and the modal's own title)
// off the top. Regression for the "picker box too big for its container" report.
func TestPickerFitsWithinTerminal(t *testing.T) {
	for _, wh := range [][2]int{{120, 40}, {200, 50}, {100, 30}, {240, 60}, {80, 24}, {150, 45}} {
		m := New(&client.Fake{}, "x")
		m.connected = true
		m.width, m.height = wh[0], wh[1]
		m.live[0] = &chanLive{mode: "feldhell:center=1500"}
		m.sel = 0
		v := newOperateView(m)
		v.Update(tea.KeyMsg{Type: tea.KeyCtrlO})
		if v.picker == nil {
			t.Fatalf("%dx%d: picker should be open", wh[0], wh[1])
		}
		m.stack = []View{v}
		out := m.View()
		if gh := lipgloss.Height(out); gh > wh[1] {
			t.Errorf("term %dx%d: View is %d rows tall, overflows by %d", wh[0], wh[1], gh, gh-wh[1])
		}
		if gw := lipgloss.Width(out); gw > wh[0] {
			t.Errorf("term %dx%d: View is %d cols wide, overflows by %d", wh[0], wh[1], gw, gw-wh[0])
		}
	}
}
