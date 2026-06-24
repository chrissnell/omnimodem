package app

import (
	"fmt"

	"github.com/charmbracelet/lipgloss"
)

var statusStyle = lipgloss.NewStyle().Reverse(true)

// statusBar renders the always-on bottom line: channel, mode, PTT, clock, levels.
func (m *Model) statusBar() string {
	cl := m.live[m.sel]
	if cl == nil {
		if m.err != "" {
			return statusStyle.Render(" omnimodem · " + m.err + " ")
		}
		return statusStyle.Render(" omnimodem · no channel ")
	}
	ptt := "▢"
	if cl.pttKeyed {
		ptt = "▣ TX"
	}
	clk := "clk ✗"
	if cl.clockSync {
		clk = "clk ✓"
	}
	s := fmt.Sprintf(" omnimodem · ch%d ▸ %s · PTT %s · %s · RX %.0f dBFS ",
		m.sel, orNone(cl.mode), ptt, clk, cl.rxDbfs)
	if m.err != "" {
		s += "· " + m.err + " "
	}
	return statusStyle.Render(s)
}

func orNone(s string) string {
	if s == "" {
		return "none"
	}
	return s
}
