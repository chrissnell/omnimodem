package app

import (
	"image"
	"math"
)

// pictureSend is a raster staged for the daemon's TransmitImage RPC: row-major
// interleaved RGB plus the dimensions/colour/spp the encoder needs. txSecs is the
// estimated on-air duration, used to size the TX watchdog for long facsimile sends.
type pictureSend struct {
	width, height uint32
	rgb           []byte
	color         bool
	txspp         uint32
	txSecs        float64
}

// buildPictureSend downsamples a staged image into a transmit raster sized for the
// channel's mode. Returns ok=false for modes the daemon can't carry a picture on
// (e.g. the Hell text-raster modes), so the caller can surface a clear message
// instead of a silent no-op. Only WEFAX is wired here today — the fldigi picture
// families (MFSK/THOR/IFKP/FSQ) are not yet exposed as image-shape in the TUI.
func buildPictureSend(modeLabel string, img image.Image) (pictureSend, bool) {
	// WEFAX stretches each source row to a fixed line width, so only the row count
	// drives duration. Cap rows to keep the on-air time near ~2.5 min, preserving
	// aspect. wefax576 runs 120 lpm (0.5 s/line); wefax288 runs 60 lpm (1 s/line).
	var maxW, maxRows int
	var secPerLine float64
	switch modeLabel {
	case "wefax576":
		maxW, maxRows, secPerLine = 800, 280, 0.5
	case "wefax288":
		maxW, maxRows, secPerLine = 400, 140, 1.0
	default:
		return pictureSend{}, false
	}

	b := img.Bounds()
	w, h := fitBox(b.Dx(), b.Dy(), maxW, maxRows)
	// APT tones + 20 phasing lines + start/stop overhead, generously rounded.
	overhead := float64(24)*secPerLine + 4
	return pictureSend{
		width:  uint32(w),
		height: uint32(h),
		rgb:    resampleToRGB(img, w, h),
		color:  false, // WEFAX is grayscale; the daemon folds RGB to luma.
		txspp:  0,
		txSecs: float64(h)*secPerLine + overhead,
	}, true
}

// fitBox scales (srcW,srcH) to fit within (maxW,maxH) preserving aspect ratio,
// never upscaling past the source. Degenerate inputs clamp to 1×1.
func fitBox(srcW, srcH, maxW, maxH int) (int, int) {
	if srcW <= 0 || srcH <= 0 {
		return 1, 1
	}
	scale := math.Min(float64(maxW)/float64(srcW), float64(maxH)/float64(srcH))
	if scale > 1 {
		scale = 1
	}
	return max(1, int(float64(srcW)*scale)), max(1, int(float64(srcH)*scale))
}

// resampleToRGB nearest-neighbour resamples img into a dstW×dstH row-major RGB
// buffer (dstW*dstH*3 bytes). Alpha is composited over black, matching the
// half-block preview so what the operator sees is what goes on the air.
func resampleToRGB(img image.Image, dstW, dstH int) []byte {
	b := img.Bounds()
	srcW, srcH := b.Dx(), b.Dy()
	out := make([]byte, dstW*dstH*3)
	for y := 0; y < dstH; y++ {
		sy := b.Min.Y + y*srcH/dstH
		for x := 0; x < dstW; x++ {
			sx := b.Min.X + x*srcW/dstW
			r, g, bl, a := img.At(sx, sy).RGBA()
			r8, g8, b8, a8 := r>>8, g>>8, bl>>8, a>>8
			if a8 < 255 {
				r8 = r8 * a8 / 255
				g8 = g8 * a8 / 255
				b8 = b8 * a8 / 255
			}
			i := (y*dstW + x) * 3
			out[i], out[i+1], out[i+2] = byte(r8), byte(g8), byte(b8)
		}
	}
	return out
}
