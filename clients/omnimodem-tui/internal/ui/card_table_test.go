package ui

import (
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

// Card draws a rounded, titled box; focus swaps the border/title to the accent.
func TestCardRendersTitledRoundedBox(t *testing.T) {
	out := Card("STATION", "body line", true, 30)
	if !strings.Contains(out, "STATION") {
		t.Fatalf("card must show its title:\n%s", out)
	}
	if !strings.Contains(out, "╭") || !strings.Contains(out, "╯") {
		t.Fatalf("card must have a rounded border:\n%s", out)
	}
	// Every rendered line is the same (outer) width — a well-formed box.
	w := -1
	for _, ln := range strings.Split(out, "\n") {
		if w == -1 {
			w = lipgloss.Width(ln)
		} else if lipgloss.Width(ln) != w {
			t.Fatalf("card lines must share one width; got %d vs %d", w, lipgloss.Width(ln))
		}
	}
	if w != 30 {
		t.Fatalf("card outer width must equal the requested 30, got %d", w)
	}
}

// Table renders headers, rows, and keeps every line at the width TableWidth
// predicts, with cells truncated to their column budget.
func TestTableWidthAndTruncation(t *testing.T) {
	cols := []Column{{"DEVICE", 12}, {"ID", 10}, {"I/O", 5}}
	rows := [][]string{{"A very long device name", "some:long:id:here", "RX·TX"}, {"Mic", "m", "RX"}}
	out := Table(cols, rows, 0)
	want := TableWidth(cols)
	for _, ln := range strings.Split(out, "\n") {
		if lipgloss.Width(ln) != want {
			t.Fatalf("table line width %d != TableWidth %d:\n%s", lipgloss.Width(ln), want, out)
		}
	}
	if !strings.Contains(out, "…") {
		t.Fatalf("an over-long cell must be truncated with an ellipsis:\n%s", out)
	}
	if !strings.Contains(out, "DEVICE") {
		t.Fatalf("table must render its headers:\n%s", out)
	}
}

// The inset table drops the outer frame (for embedding in a Card) but keeps rows.
func TestTableInsetHasNoOuterFrame(t *testing.T) {
	cols := []Column{{"A", 4}, {"B", 4}}
	out := TableInset(cols, [][]string{{"x", "y"}}, -1)
	if strings.Contains(out, "╭") || strings.Contains(out, "╰") {
		t.Fatalf("inset table must not draw an outer frame:\n%s", out)
	}
	if !strings.Contains(out, "x") {
		t.Fatalf("inset table must still render rows:\n%s", out)
	}
}
