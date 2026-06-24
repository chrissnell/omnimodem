package ui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// Header is the top bar: app name, connection dot + address, version.
func Header(connected bool, addr, version string, w int) string {
	dot, state := Dim.Render("○"), "connecting"
	if connected {
		dot = lipgloss.NewStyle().Foreground(ColorOK).Render("●")
		state = "connected"
	}
	left := Title.Render(" omnimodem ")
	right := fmt.Sprintf("%s %s · %s · %s ", dot, state, addr, version)
	gap := max(1, w-lipgloss.Width(left)-lipgloss.Width(right))
	return left + strings.Repeat(" ", gap) + right
}

// Hint is one footer key→action pair.
type Hint struct{ Key, Action string }

// Footer renders the contextual hotkey strip.
func Footer(hints []Hint, w int) string {
	parts := make([]string, 0, len(hints))
	for _, h := range hints {
		parts = append(parts, FooterKey.Render("<"+h.Key+">")+" "+FooterText.Render(h.Action))
	}
	line := " " + strings.Join(parts, "  ")
	if lipgloss.Width(line) > w {
		line = lipgloss.NewStyle().MaxWidth(w).Render(line)
	}
	return line
}
