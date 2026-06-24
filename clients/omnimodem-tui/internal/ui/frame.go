package ui

import "github.com/charmbracelet/lipgloss"

// Frame draws a titled, bordered pane sized to w×h (outer dimensions). When
// focused the border uses the accent color.
func Frame(title, body string, focused bool, w, h int) string {
	border := ColorDim
	if focused {
		border = ColorAccent
	}
	style := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(border).
		Width(max(1, w-2)).
		Height(max(1, h-2)).
		Padding(0, 1)
	titled := Title.Render(" "+title+" ") + "\n" + body
	return style.Render(titled)
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}
