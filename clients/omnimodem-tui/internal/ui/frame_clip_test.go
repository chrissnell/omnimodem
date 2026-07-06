package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

func TestFrameNeverExceedsHeight(t *testing.T) {
	hasTop := func(s string) bool { return strings.Contains(strings.SplitN(s, "\n", 2)[0], "╔") }
	hasBottom := func(s string) bool { ls := strings.Split(s, "\n"); return strings.Contains(ls[len(ls)-1], "╚") }

	// Correctly-sized (underfilled) body: borders intact, height == h.
	small := Frame("Title", strings.Repeat("x\n", 5), true, 40, 20)
	t.Logf("underfilled: h=%d top=%v bottom=%v", lipgloss.Height(small), hasTop(small), hasBottom(small))
	if lipgloss.Height(small) != 20 || !hasTop(small) || !hasBottom(small) {
		t.Errorf("underfilled frame should be exactly 20 with both borders")
	}
	// Over-tall body: clipped to h, never taller (top border must survive).
	big := Frame("Title", strings.Repeat("x\n", 60), true, 40, 20)
	t.Logf("overfilled: h=%d top=%v bottom=%v", lipgloss.Height(big), hasTop(big), hasBottom(big))
	if lipgloss.Height(big) > 20 {
		t.Errorf("overfilled frame height %d exceeds 20", lipgloss.Height(big))
	}
}
