package app

import (
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

// A live toast is drawn below the footer; it must not push the view past the
// terminal. Regression for the picker "still running off the top" report: the
// picker fills the surface, and a concurrent RSID/TX toast overflowed it.
func TestViewFitsWithToast(t *testing.T) {
	for _, wh := range [][2]int{{120, 40}, {200, 50}, {100, 30}, {80, 24}, {150, 45}} {
		for _, withToast := range []bool{false, true} {
			m := New(&client.Fake{}, "x")
			m.connected = true
			m.width, m.height = wh[0], wh[1]
			m.live[0] = &chanLive{mode: "feldhell:center=1500"}
			m.sel = 0
			v := newOperateView(m)
			v.Update(tea.KeyMsg{Type: tea.KeyCtrlO})
			m.stack = []View{v}
			if withToast {
				m.toast = ui.NewToast("RSID: feldhell @ 1500 Hz", ui.SeverityInfo)
			}
			if gh := lipgloss.Height(m.View()); gh > wh[1] {
				t.Errorf("term %dx%d toast=%v: View %d rows, overflows by %d", wh[0], wh[1], withToast, gh, gh-wh[1])
			}
		}
	}
}
