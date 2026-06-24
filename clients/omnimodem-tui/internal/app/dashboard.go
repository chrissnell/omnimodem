package app

import (
	"fmt"
	"sort"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
)

func (m *Model) updateDashboard(msg tea.Msg) (tea.Model, tea.Cmd) {
	key, ok := msg.(tea.KeyMsg)
	if !ok {
		return m, nil
	}
	switch key.String() {
	case "j", "down":
		m.sel = nextChannel(m.live, m.sel, +1)
	case "k", "up":
		m.sel = nextChannel(m.live, m.sel, -1)
	case "c":
		m.enterConfig()
		return m, devicesCmd(m.c)
	case "o":
		m.enterOperate()
		return m, nil
	}
	return m, nil
}

func (m *Model) viewDashboard() string {
	var b strings.Builder
	b.WriteString("Channels  (j/k select · c configure · o operate)\n\n")
	for _, ch := range sortedChannels(m.live) {
		cl := m.live[ch]
		cursor := "  "
		if ch == m.sel {
			cursor = "▸ "
		}
		b.WriteString(fmt.Sprintf("%sch%d  %-10s %-8s  RX %.0f dBFS\n",
			cursor, ch, orNone(cl.name), orNone(cl.mode), cl.rxDbfs))
	}
	if len(m.live) == 0 {
		b.WriteString("  (none — configure a channel)\n")
	}
	return b.String()
}

func sortedChannels(live map[uint32]*chanLive) []uint32 {
	out := make([]uint32, 0, len(live))
	for ch := range live {
		out = append(out, ch)
	}
	sort.Slice(out, func(i, j int) bool { return out[i] < out[j] })
	return out
}

func nextChannel(live map[uint32]*chanLive, cur uint32, dir int) uint32 {
	chs := sortedChannels(live)
	if len(chs) == 0 {
		return cur
	}
	idx := 0
	for i, c := range chs {
		if c == cur {
			idx = i
		}
	}
	idx = (idx + dir + len(chs)) % len(chs)
	return chs[idx]
}
