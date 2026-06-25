package ui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// Header is the top menu bar (black on light gray, DOS style): the app name in
// Borland red, then the connection dot + address + version, filling the width.
func Header(connected bool, addr, version string, w int) string {
	name := lipgloss.NewStyle().Background(ColorMenu).Foreground(lipgloss.Color("1")).
		Bold(true).Render(" omnimodem ")
	dotColor, glyph, state := ColorDim, "○", "connecting"
	if connected {
		dotColor, glyph, state = ColorOK, "●", "connected"
	}
	dot := lipgloss.NewStyle().Background(ColorMenu).Foreground(dotColor).Render(glyph)
	right := dot + MenuBar.Render(fmt.Sprintf(" %s · %s · %s ", state, addr, version))
	gap := max(0, w-lipgloss.Width(name)-lipgloss.Width(right))
	return name + MenuBar.Render(strings.Repeat(" ", gap)) + right
}

// Hint is one footer key→action pair.
type Hint struct{ Key, Action string }

// Footer renders the contextual hotkey strip as a full-width cyan status bar.
func Footer(hints []Hint, w int) string {
	parts := make([]string, 0, len(hints)+1)
	parts = append(parts, StatusBar.Render(" "))
	for _, h := range hints {
		parts = append(parts, FooterKey.Render("‹"+h.Key+"›")+FooterText.Render(" "+h.Action+"  "))
	}
	line := strings.Join(parts, "")
	if used := lipgloss.Width(line); used < w {
		line += StatusBar.Render(strings.Repeat(" ", w-used))
	} else if used > w {
		line = lipgloss.NewStyle().MaxWidth(w).Render(line)
	}
	return line
}
