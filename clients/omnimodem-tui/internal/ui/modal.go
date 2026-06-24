package ui

import "github.com/charmbracelet/lipgloss"

// Modal renders content inside a titled, line-bordered box — the standard dialog
// look for pickers and prompts, so a modal stands apart from the surrounding
// text. `w` is the outer box width; `body` is the pre-rendered inner content
// (e.g. a bubbles list's View). Height is content-driven.
func Modal(title, body string, w int) string {
	const chrome = 4 // border (2) + horizontal padding (2)
	inner := w - chrome
	if inner < 6 {
		inner = 6
	}
	style := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(ColorAccent).
		Width(inner).
		Padding(0, 1)
	return style.Render(Title.Render(title) + "\n" + body)
}
