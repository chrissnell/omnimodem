package app

import tea "github.com/charmbracelet/bubbletea"

// configState is the configuration-screen form state. Fleshed out in Phase 2
// (device pickers, mode params, gain). Phase 1 ships a minimal shell so screen
// routing compiles and is navigable.
type configState struct{}

func (m *Model) enterConfig() {
	m.screen = screenConfig
	m.cfg = &configState{}
}

func (m *Model) updateConfig(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case devicesMsg:
		return m, nil
	case tea.KeyMsg:
		if msg.String() == "esc" {
			m.screen = screenDashboard
		}
	}
	return m, nil
}

func (m *Model) viewConfig() string {
	return "Configure ch (Phase 2)\n(esc to go back)"
}
