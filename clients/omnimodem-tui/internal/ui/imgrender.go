package ui

import (
	"fmt"
	"image"
	"os"
	"strings"

	// Register the decoders the picker accepts, so image.Decode handles them.
	_ "image/gif"
	_ "image/jpeg"
	_ "image/png"

	"github.com/charmbracelet/lipgloss"
)

// DecodeImageFile opens and decodes an image file into an image.Image. Only the
// formats the picker offers (PNG/JPEG/GIF) are registered, so anything else
// returns an "unknown format" error rather than a partial read.
func DecodeImageFile(path string) (image.Image, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	img, _, err := image.Decode(f)
	return img, err
}

// RenderImageHalfBlock renders an image as a block of truecolor text using the
// Unicode upper-half block (▀): each character cell stacks two vertical pixels —
// the top pixel is the glyph's foreground, the bottom pixel its background — so a
// cell grid of cols×rows shows an image cols wide and rows*2 tall. The source is
// nearest-neighbour scaled to fit within (cols, rows*2) preserving aspect ratio.
// Colours are emitted as 24-bit values; lipgloss/termenv degrades them to the
// terminal's real colour depth, so this stays faithful on truecolor terminals
// and legible on 256/16-colour ones.
func RenderImageHalfBlock(img image.Image, cols, rows int) string {
	if img == nil || cols < 1 || rows < 1 {
		return ""
	}
	b := img.Bounds()
	srcW, srcH := b.Dx(), b.Dy()
	if srcW <= 0 || srcH <= 0 {
		return ""
	}
	// Fit within cols × (rows*2) pixels, preserving aspect ratio. Never upscale
	// past the source — a tiny thumbnail stays crisp rather than blowing up.
	scale := min(float64(cols)/float64(srcW), float64(rows*2)/float64(srcH))
	if scale > 1 {
		scale = 1
	}
	outW := max(1, int(float64(srcW)*scale))
	outH := max(1, int(float64(srcH)*scale))

	// Adjacent pixels in real photos repeat, so cache one style per fg/bg pair to
	// keep the escape-sequence churn down.
	cache := map[[2]uint32]lipgloss.Style{}
	cell := func(fg, bg uint32) string {
		k := [2]uint32{fg, bg}
		st, ok := cache[k]
		if !ok {
			st = lipgloss.NewStyle().
				Foreground(lipgloss.Color(hex(fg))).
				Background(lipgloss.Color(hex(bg)))
			cache[k] = st
		}
		return st.Render("▀")
	}

	var sb strings.Builder
	for oy := 0; oy < outH; oy += 2 {
		for ox := 0; ox < outW; ox++ {
			top := sampleRGB(img, b, srcW, srcH, outW, outH, ox, oy)
			// The last odd row has no bottom pixel; fall back to panel black so the
			// trailing half-cell blends into the surrounding dialog.
			var bottom uint32
			if oy+1 < outH {
				bottom = sampleRGB(img, b, srcW, srcH, outW, outH, ox, oy+1)
			}
			sb.WriteString(cell(top, bottom))
		}
		if oy+2 < outH {
			sb.WriteByte('\n')
		}
	}
	return sb.String()
}

// sampleRGB nearest-neighbour maps output pixel (ox,oy) back to the source and
// returns its colour packed as 0xRRGGBB. Alpha is composited over black so
// transparent PNGs read as they would against the dialog's panel.
func sampleRGB(img image.Image, b image.Rectangle, srcW, srcH, outW, outH, ox, oy int) uint32 {
	sx := b.Min.X + ox*srcW/outW
	sy := b.Min.Y + oy*srcH/outH
	r, g, bl, a := img.At(sx, sy).RGBA()
	r8, g8, b8, a8 := r>>8, g>>8, bl>>8, a>>8
	if a8 < 255 {
		r8 = r8 * a8 / 255
		g8 = g8 * a8 / 255
		b8 = b8 * a8 / 255
	}
	return r8<<16 | g8<<8 | b8
}

func hex(rgb uint32) string { return fmt.Sprintf("#%06x", rgb&0xffffff) }
