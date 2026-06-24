package ui

import (
	"strings"
	"testing"
)

func TestFrameShowsTitle(t *testing.T) {
	out := Frame("Channels", "body", true, 40, 6)
	if !strings.Contains(out, "Channels") {
		t.Fatalf("frame must show its title, got:\n%s", out)
	}
}

func TestFooterShowsBindings(t *testing.T) {
	out := Footer([]Hint{{"enter", "operate"}, {"c", "configure"}}, 60)
	if !strings.Contains(out, "enter") || !strings.Contains(out, "configure") {
		t.Fatalf("footer must show hints, got: %s", out)
	}
}

func TestHeaderShowsState(t *testing.T) {
	if !strings.Contains(Header(true, "/x.sock", "v1", 60), "connected") {
		t.Fatal("header must show connection state")
	}
}
