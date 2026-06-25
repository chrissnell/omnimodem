package ui

import "github.com/charmbracelet/lipgloss"

// Modal renders content inside a titled, double-line dialog — the DOS picker/
// prompt look: a blue panel with a bright-white double border and a yellow
// title, standing apart from the surrounding text. `w` is the outer box width;
// `body` is the pre-rendered inner content (e.g. a bubbles list's View). Height
// is content-driven.
func Modal(title, body string, w int) string {
	const chrome = 4 // border (2) + horizontal padding (2)
	inner := w - chrome
	if inner < 6 {
		inner = 6
	}
	style := lipgloss.NewStyle().
		Border(lipgloss.DoubleBorder()).
		BorderForeground(ColorFg).
		BorderBackground(ColorPanel).
		Background(ColorPanel).
		Foreground(ColorFg).
		Width(inner).
		Padding(0, 1)
	return style.Render(Title.Background(ColorPanel).Render(title) + "\n" + body)
}
