package ui

import "github.com/charmbracelet/lipgloss"

// Frame draws a titled, double-line panel sized to w×h (outer dimensions) in the
// DOS-desktop style: a blue fill with a bright-cyan double border (white when
// focused) and a yellow title. Body text inherits the blue panel background.
func Frame(title, body string, focused bool, w, h int) string {
	border := ColorAccent
	if focused {
		border = ColorFg
	}
	style := lipgloss.NewStyle().
		Border(lipgloss.DoubleBorder()).
		BorderForeground(border).
		BorderBackground(ColorPanel).
		Background(ColorPanel).
		Foreground(ColorFg).
		Width(max(1, w-2)).
		Height(max(1, h-2)).
		// Cap the whole box (border included) at its allotted height h. For a
		// correctly-sized body this is a no-op; it only bites if a body overfills,
		// clipping its bottom rather than expanding the frame past h and scrolling
		// the screen off the top.
		MaxHeight(max(1, h)).
		Padding(0, 1)
	titled := Title.Background(ColorPanel).Render(" "+title+" ") + "\n" + body
	return style.Render(titled)
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}
