package app

import tea "github.com/charmbracelet/bubbletea"

// operateState is the operate-screen state. Fleshed out in Phase 3 (ragchew:
// compose/transcript/macros/TX + waterfall) and Phase 4 (FT8). Phase 1 ships a
// minimal shell so screen routing compiles and the event stream keeps draining.
type operateState struct{}

func (m *Model) enterOperate() {
	m.screen = screenOperate
	m.op = &operateState{}
}

func (m *Model) updateOperate(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case eventMsg:
		m.applyEvent(msg.ev)
		return m, waitForEvent(m.events)
	case tea.KeyMsg:
		if msg.String() == "esc" {
			m.screen = screenDashboard
		}
	}
	return m, nil
}

func (m *Model) viewOperate() string {
	return "Operate ch (Phase 3/4)\n(esc to go back)"
}
