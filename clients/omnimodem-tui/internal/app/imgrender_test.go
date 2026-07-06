package app

import (
	"strings"
	"testing"
)

// A 2x2 RGB image folds into a single half-block text row (two image rows per
// cell), carrying the top pixel as foreground and the bottom as background.
func TestRenderImageHalfBlockRGB(t *testing.T) {
	// row0: red, green ; row1: blue, white
	px := []byte{255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255}
	out := renderImageHalfBlock(px, 2, 3, 80, 40)
	if strings.Contains(out, "\n") {
		t.Fatalf("2-row image should be one half-block line, got %q", out)
	}
	if !strings.Contains(out, "▀") {
		t.Fatalf("expected half-block glyph, got %q", out)
	}
	// First cell: fg = red (top), bg = blue (bottom).
	if !strings.Contains(out, "\x1b[38;2;255;0;0;48;2;0;0;255m▀") {
		t.Fatalf("first cell should be fg-red/bg-blue, got %q", out)
	}
	// Second cell: fg = green (top), bg = white (bottom).
	if !strings.Contains(out, "\x1b[38;2;0;255;0;48;2;255;255;255m▀") {
		t.Fatalf("second cell should be fg-green/bg-white, got %q", out)
	}
}

// Grayscale (channels=1) maps a single sample to r=g=b.
func TestRenderImageHalfBlockGray(t *testing.T) {
	// 1 wide, 2 tall: top=200, bottom=50.
	out := renderImageHalfBlock([]byte{200, 50}, 1, 1, 80, 40)
	if !strings.Contains(out, "\x1b[38;2;200;200;200;48;2;50;50;50m▀") {
		t.Fatalf("gray should expand to equal RGB, got %q", out)
	}
}

// An odd final row uses black for the absent bottom pixel and still terminates.
func TestRenderImageHalfBlockOddHeight(t *testing.T) {
	// 1 wide, 1 tall: single white pixel.
	out := renderImageHalfBlock([]byte{255}, 1, 1, 80, 40)
	if !strings.Contains(out, "\x1b[38;2;255;255;255;48;2;0;0;0m▀") {
		t.Fatalf("odd row should pad bottom with black, got %q", out)
	}
}

// Downscaling never exceeds the character-cell budget.
func TestRenderImageHalfBlockScalesToFit(t *testing.T) {
	// 100x100 gray image into a 10x10 cell budget (20 px tall).
	px := make([]byte, 100*100)
	out := renderImageHalfBlock(px, 100, 1, 10, 10)
	for _, line := range strings.Split(out, "\n") {
		// Count glyphs, not escape bytes.
		if n := strings.Count(line, "▀"); n > 10 {
			t.Fatalf("line exceeds 10 cells wide: %d", n)
		}
	}
	if lines := strings.Count(out, "\n") + 1; lines > 10 {
		t.Fatalf("more than 10 text rows: %d", lines)
	}
}
