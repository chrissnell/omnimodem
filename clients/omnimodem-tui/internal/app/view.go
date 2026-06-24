package app

import (
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	tea "github.com/charmbracelet/bubbletea"
)

// View is one screen in the window manager. Update returns the (possibly new)
// view state plus a command; Render draws into the content rect; Title labels
// the bordered pane; Hints feeds the footer. Views read shared state via the
// *Model they were constructed with.
type View interface {
	Update(tea.Msg) (View, tea.Cmd)
	Render(w, h int) string
	Title() string
	Hints() []ui.Hint
}

func (m *Model) push(v View) { m.stack = append(m.stack, v) }

func (m *Model) pop() {
	if len(m.stack) > 1 {
		m.stack = m.stack[:len(m.stack)-1]
	}
}

func (m *Model) top() View {
	if len(m.stack) == 0 {
		return nil
	}
	return m.stack[len(m.stack)-1]
}

func orNone(s string) string {
	if s == "" {
		return "none"
	}
	return s
}

func orDash(s string) string {
	if s == "" {
		return "—"
	}
	return s
}
