package ui

import "testing"

func TestStylesNonEmpty(t *testing.T) {
	if Accent.GetForeground() == nil {
		t.Fatal("Accent must set a foreground color")
	}
	if got := Title.Render("x"); got == "" {
		t.Fatal("Title style must render")
	}
}
