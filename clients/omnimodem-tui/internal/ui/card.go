package ui

import (
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// Card is the reusable panel the TUI composes screens from: a rounded-border box
// with a titled header and a hairline rule under it. The border and title glow
// in the accent colour when the card holds focus and sit quiet grey otherwise,
// so a screen full of cards reads at a glance — the live one is obvious.
//
// w is the OUTER width (border included). Height is content-driven. body should
// already be wrapped to CardInnerWidth(w); longer lines are clipped by the box.
func Card(title, body string, focused bool, w int) string {
	border, titleColor := ColorDim, ColorDim
	if focused {
		border, titleColor = ColorAccent, ColorTitle
	}
	// Match ui.Modal's width contract: Width() is the block width including
	// padding, so the outer box is Width+2 (the border). inner is the text area.
	block := w - 2
	if block < 6 {
		block = 6
	}
	inner := block - 2 // minus Padding(0,1)

	header := lipgloss.NewStyle().
		Foreground(titleColor).Background(ColorPanel).Bold(true).
		Width(inner).MaxWidth(inner).Render(title)
	rule := lipgloss.NewStyle().
		Foreground(border).Background(ColorPanel).
		Render(strings.Repeat("─", inner))

	content := header + "\n" + rule
	if body != "" {
		content += "\n" + body
	}
	return lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(border).
		BorderBackground(ColorPanel).
		Background(ColorPanel).
		Foreground(ColorFg).
		Width(block).
		Padding(0, 1).
		Render(content)
}

// CardInnerWidth is the usable text width inside a Card of outer width w — what
// callers should wrap their body to (border + one column of padding each side).
func CardInnerWidth(w int) int {
	inner := w - 4
	if inner < 2 {
		inner = 2
	}
	return inner
}
