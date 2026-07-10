package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

func TestModalHasTitleAndBorder(t *testing.T) {
	out := Modal("Pick device", "item1\nitem2", 40)
	if !strings.Contains(out, "Pick device") {
		t.Fatalf("modal must show its title:\n%s", out)
	}
	if !strings.Contains(out, "═") || !strings.Contains(out, "║") {
		t.Fatalf("modal must draw a double-line border:\n%s", out)
	}
	if !strings.Contains(out, "item1") {
		t.Fatalf("modal must contain its body:\n%s", out)
	}
}

// A body sized to w-4 (border + padding) must fit on one line, and the rendered
// box must be exactly w wide. Regression: lipgloss Width() includes padding, so
// sizing the style to w-4 left a w-6 text area that wrapped every full-width line
// — the picture preview doubled every scanline and spilled out of the box.
func TestModalDoesNotWrapFullWidthBody(t *testing.T) {
	for _, w := range []int{40, 60, 88} {
		body := strings.Repeat("x", w-4) // the full advertised text width
		out := Modal("T", body, w)
		if gw := lipgloss.Width(out); gw != w {
			t.Errorf("modal rendered %d cols wide, want %d", gw, w)
		}
		// title(1) + border(2) + the single body line = 4 rows; a wrap would add more.
		if gh := lipgloss.Height(out); gh != 4 {
			t.Errorf("w=%d: modal is %d rows tall, want 4 (a w-4 body line wrapped)", w, gh)
		}
	}
}
