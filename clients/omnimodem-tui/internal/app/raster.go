package app

import (
	"strings"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// rasterBuf accumulates the received image column stream for a facsimile mode
// (Hell, and later WEFAX / the picture sub-protocols). Each proto Image carries
// `width` pixels per column and a row-major gray buffer; successive `width`-byte
// runs are on-air columns, so we append them and render a scrolling raster.
type rasterBuf struct {
	width int      // pixels per column (image width); 0 until the first frame
	cols  [][]byte // each entry is one column of `width` gray bytes
}

// maxRasterCols bounds retained history so a long receive session can't grow the
// buffer without limit (only the visible tail is ever rendered).
const maxRasterCols = 4096

// push appends the columns carried by one Image frame.
func (r *rasterBuf) push(img *pb.Image) {
	if img == nil {
		return
	}
	w := int(img.GetWidth())
	if w == 0 {
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

// render draws the most recent `cols` columns as `width` rows of block glyphs.
// The top pixel row (highest index) is drawn first, so text reads upright; a
// pixel is "on" when its gray value exceeds mid-scale.
func (r *rasterBuf) render(cols int) string {
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
