package app

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// rasterBuf accumulates a received image for a facsimile mode. Two shapes share
// it, distinguished by which field the proto Image sets:
//   - mono column stream (Hell): `gray` — successive `width`-byte runs are on-air
//     columns; we append them and render a scrolling raster.
//   - colour frame (SSTV): `rgb` — a whole row-major 3-bytes/pixel frame arrives
//     at end-of-picture; we keep the latest and render it downsampled in colour.
type rasterBuf struct {
	// Mono column stream (Hell).
	width int      // pixels per column (image width); 0 until the first frame
	cols  [][]byte // each entry is one column of `width` gray bytes

	// Colour frame (SSTV).
	rgbWidth int
	rgb      []byte // row-major, 3 bytes/pixel
	gen      int    // bumped on each new colour frame (render cache key)

	cachedCols, cachedGen int
	cached                string
}

// maxRasterCols bounds retained mono history so a long receive session can't grow
// the buffer without limit (only the visible tail is ever rendered).
const maxRasterCols = 4096

// push folds one Image frame into the buffer.
func (r *rasterBuf) push(img *pb.Image) {
	if img == nil {
		return
	}
	w := int(img.GetWidth())
	if w == 0 {
		return
	}
	if rgb := img.GetRgb(); len(rgb) >= w*3 {
		// Colour frame (SSTV): replace with the latest full picture.
		r.rgbWidth = w
		r.rgb = append(r.rgb[:0], rgb...)
		r.gen++
		return
	}
	r.width = w
	g := img.GetGray()
	for i := 0; i+w <= len(g); i += w {
		col := make([]byte, w)
		copy(col, g[i:i+w])
		r.cols = append(r.cols, col)
	}
	if len(r.cols) > maxRasterCols {
		r.cols = r.cols[len(r.cols)-maxRasterCols:]
	}
}

// isColor reports whether a colour frame has been received.
func (r *rasterBuf) isColor() bool { return r.rgbWidth > 0 && len(r.rgb) > 0 }

// status is the one-line descriptor shown in the raster header.
func (r *rasterBuf) status() string {
	if r.isColor() {
		return fmt.Sprintf("colour %dx%d", r.rgbWidth, len(r.rgb)/(r.rgbWidth*3))
	}
	return fmt.Sprintf("%d cols", len(r.cols))
}

// render draws the raster into at most `cols` terminal columns.
func (r *rasterBuf) render(cols int) string {
	if r.isColor() {
		return r.renderColor(cols)
	}
	return r.renderMono(cols)
}

// renderColor downsamples the latest colour frame to `cols` wide and draws it with
// truecolor half-block glyphs (▀: foreground = upper pixel, background = lower),
// so each character row shows two image rows. Result is cached per `cols`/frame.
func (r *rasterBuf) renderColor(cols int) string {
	if cols < 1 {
		cols = 1
	}
	if cols == r.cachedCols && r.gen == r.cachedGen && r.cached != "" {
		return r.cached
	}
	w := r.rgbWidth
	rows := len(r.rgb) / (w * 3)
	if rows == 0 {
		return "(waiting for image…)"
	}
	outCols := cols
	if outCols > w {
		outCols = w
	}
	// Square the pixels (terminal cells are ~twice as tall as wide, and each glyph
	// packs two vertical pixels): pixel rows shown ≈ rows * outCols / w.
	outPixRows := rows * outCols / w
	if outPixRows < 2 {
		outPixRows = 2
	}
	sample := func(px, py int) (uint8, uint8, uint8) {
		sx := px * w / outCols
		sy := py * rows / outPixRows
		if sy >= rows {
			sy = rows - 1
		}
		i := (sy*w + sx) * 3
		return r.rgb[i], r.rgb[i+1], r.rgb[i+2]
	}
	hexOf := func(cr, cg, cb uint8) lipgloss.Color {
		return lipgloss.Color(fmt.Sprintf("#%02x%02x%02x", cr, cg, cb))
	}
	var b strings.Builder
	charRows := outPixRows / 2
	for ci := 0; ci < charRows; ci++ {
		for cj := 0; cj < outCols; cj++ {
			tr, tg, tb := sample(cj, ci*2)
			br, bg, bb := sample(cj, ci*2+1)
			st := lipgloss.NewStyle().Foreground(hexOf(tr, tg, tb)).Background(hexOf(br, bg, bb))
			b.WriteString(st.Render("▀"))
		}
		if ci < charRows-1 {
			b.WriteByte('\n')
		}
	}
	r.cached = b.String()
	r.cachedCols = cols
	r.cachedGen = r.gen
	return r.cached
}

// renderMono draws the most recent `cols` mono columns as `width` rows of block
// glyphs. The top pixel row (highest index) is drawn first so text reads upright;
// a pixel is "on" when its gray value exceeds mid-scale.
func (r *rasterBuf) renderMono(cols int) string {
	if r.width == 0 || len(r.cols) == 0 {
		return "(waiting for raster…)"
	}
	if cols < 1 {
		cols = 1
	}
	start := 0
	if len(r.cols) > cols {
		start = len(r.cols) - cols
	}
	show := r.cols[start:]
	var b strings.Builder
	for row := r.width - 1; row >= 0; row-- {
		for _, c := range show {
			if row < len(c) && c[row] > 127 {
				b.WriteByte('#')
			} else {
				b.WriteByte(' ')
			}
		}
		if row > 0 {
			b.WriteByte('\n')
		}
	}
	return b.String()
}
