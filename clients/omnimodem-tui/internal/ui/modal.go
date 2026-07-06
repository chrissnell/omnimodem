package ui

import "github.com/charmbracelet/lipgloss"

// Modal renders content inside a titled, double-line dialog — the DOS picker/
// prompt look: a blue panel with a bright-white double border and a yellow
// title, standing apart from the surrounding text. `w` is the outer box width;
// `body` is the pre-rendered inner content (e.g. a bubbles list's View), which
// callers should size to `w-4` (the border and horizontal padding). Height is
// content-driven.
func Modal(title, body string, w int) string {
	// lipgloss Width() is the block width INCLUDING padding, so set it to the
	// outer width minus only the border. Padding(0,1) then leaves a text area of
	// w-4 — exactly what callers render to. Setting Width to w-4 (as before) made
	// the real text area w-6, so any body sized to w-4 wrapped every full-width
	// line (the picture preview doubled every scanline).
	inner := w - 2
	if inner < 8 {
		inner = 8
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
