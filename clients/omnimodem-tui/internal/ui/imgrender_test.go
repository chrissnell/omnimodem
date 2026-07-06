package ui

import (
	"image"
	"image/color"
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
)

// solidImage returns a w×h image filled with one colour, for deterministic tests.
func solidImage(w, h int, c color.Color) image.Image {
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			img.Set(x, y, c)
		}
	}
	return img
}

func TestRenderImageHalfBlockShape(t *testing.T) {
	// A 4×4 image at cols=4, rows=4 fits 1:1: four columns, two half-block rows.
	out := RenderImageHalfBlock(solidImage(4, 4, color.RGBA{255, 0, 0, 255}), 4, 4)
	if out == "" {
		t.Fatal("expected non-empty render")
	}
	if !strings.Contains(out, "▀") {
		t.Fatalf("render should use the upper-half block glyph:\n%q", out)
	}
	if lines := strings.Count(out, "\n") + 1; lines != 2 {
		t.Fatalf("4×4 image should render as 2 half-block rows, got %d", lines)
	}
}

func TestRenderImageHalfBlockFitsWithinBounds(t *testing.T) {
	// A wide image must be scaled to fit the column budget, never exceed it.
	out := RenderImageHalfBlock(solidImage(200, 40, color.RGBA{0, 255, 0, 255}), 20, 10)
	for _, line := range strings.Split(out, "\n") {
		if w := lipgloss.Width(line); w > 20 {
			t.Fatalf("rendered row width %d exceeds the 20-col budget", w)
		}
	}
}

func TestRenderImageHalfBlockDegenerate(t *testing.T) {
	if RenderImageHalfBlock(nil, 10, 10) != "" {
		t.Fatal("nil image should render empty")
	}
	if RenderImageHalfBlock(solidImage(4, 4, color.White), 0, 0) != "" {
		t.Fatal("zero size should render empty")
	}
}
