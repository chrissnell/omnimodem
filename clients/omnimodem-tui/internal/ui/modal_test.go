package ui

import (
	"strings"
	"testing"
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
