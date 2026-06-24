package ui

import (
	"time"

	"github.com/charmbracelet/lipgloss"
)

type Severity int

const (
	SeverityInfo Severity = iota
	SeverityWarn
	SeverityError
)

// Toast is a transient, severity-colored message with a TTL. The window manager
// renders it (Line) below the chrome and drops it once Expired.
type Toast struct {
	msg string
	sev Severity
	exp time.Time
}

func NewToast(msg string, sev Severity) *Toast {
	return &Toast{msg: msg, sev: sev, exp: time.Now().Add(4 * time.Second)}
}

func (t *Toast) Expired() bool { return time.Now().After(t.exp) }

func (t *Toast) Line() string {
	color := ColorAccent
	switch t.sev {
	case SeverityWarn:
		color = lipgloss.Color("214")
	case SeverityError:
		color = ColorError
	}
	return lipgloss.NewStyle().Foreground(color).Render(" ⚑ " + t.msg)
}
