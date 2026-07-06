package app

import (
	"fmt"
	"strings"
)

// renderImageHalfBlock renders a raster image to truecolor terminal text using
// the Unicode upper-half-block glyph "▀": each character cell stacks two
// vertical pixels — the top pixel becomes the glyph's foreground colour, the
// bottom pixel its background colour — so one text row carries two image rows.
// This is the portable way to show pictures in a TUI (works in any 24-bit-color
// terminal; no sixel/kitty protocol needed) and is what the picture
// sub-protocols (MFSK/THOR/IFKP/FSQ) and SSTV surfaces render into as scan lines
// arrive.
//
// pixels is row-major interleaved 8-bit samples, `channels` per pixel (1 =
// grayscale, 3 = RGB), matching the gRPC Image message. The image is
// nearest-neighbour scaled to fit maxCols×maxRows character cells (i.e.
// maxCols×2·maxRows pixels) without upscaling and without changing aspect. A
// terminal cell is ~2× taller than wide, so a pixel *pair* per cell is roughly
// square. Escapes are emitted directly (not via lipgloss) so the output is
// deterministic regardless of the terminal-profile probe — a dense pixel grid
// wants raw ANSI, not per-cell style objects.
func renderImageHalfBlock(pixels []byte, width, channels, maxCols, maxRows int) string {
	if width <= 0 || channels <= 0 || maxCols <= 0 || maxRows <= 0 {
		return ""
	}
	stride := width * channels
	rows := len(pixels) / stride
	if rows == 0 {
		return ""
	}

	// Fit into the cell budget (maxCols wide × maxRows*2 pixels tall) preserving
	// aspect; never upscale.
	maxPxH := maxRows * 2
	scale := 1.0
	if s := float64(maxCols) / float64(width); s < scale {
		scale = s
	}
	if s := float64(maxPxH) / float64(rows); s < scale {
		scale = s
	}
	outW := int(float64(width) * scale)
	outH := int(float64(rows) * scale)
	if outW < 1 {
		outW = 1
	}
	if outH < 1 {
		outH = 1
	}

	// sample returns the RGB of the source pixel nearest output coord (ox, oy).
	sample := func(ox, oy int) (byte, byte, byte) {
		sx := ox * width / outW
		sy := oy * rows / outH
		o := sy*stride + sx*channels
		if channels == 1 {
			v := pixels[o]
			return v, v, v
		}
		return pixels[o], pixels[o+1], pixels[o+2]
	}

	var b strings.Builder
	for ty := 0; ty*2 < outH; ty++ {
		for ox := 0; ox < outW; ox++ {
			tr, tg, tb := sample(ox, ty*2)
			// Bottom pixel; black when the image has an odd final row.
			var br, bg, bb byte
			if ty*2+1 < outH {
				br, bg, bb = sample(ox, ty*2+1)
			}
			fmt.Fprintf(&b, "\x1b[38;2;%d;%d;%d;48;2;%d;%d;%dm▀", tr, tg, tb, br, bg, bb)
		}
		b.WriteString("\x1b[0m")
		if (ty+1)*2 < outH {
			b.WriteByte('\n')
		}
	}
	return b.String()
}
