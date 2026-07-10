package app

import (
	"image"
	"math"
	"strings"
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

// buildPictureSend downsamples a staged image into a transmit raster for the
// channel's mode. Returns ok=false only for image-shape modes the daemon can't
// carry a picture on (the Hell text-raster modes), so the caller can surface a
// clear message instead of a silent no-op. WEFAX gets a grayscale, row-capped
// raster; every other picture mode (SSTV and any future fixed-geometry mode) gets
// a colour raster that the daemon resamples to the submode's native size.
func buildPictureSend(modeLabel string, img image.Image) (pictureSend, bool) {
	b := img.Bounds()
	switch {
	case modeLabel == "wefax576":
		return wefaxSend(img, b, 800, 280, 0.5), true
	case modeLabel == "wefax288":
		return wefaxSend(img, b, 400, 140, 1.0), true
	case strings.Contains(modeLabel, "hell"):
		// Hell paints text as a pixel raster; it isn't a picture-send mode.
		return pictureSend{}, false
	default:
		// SSTV (and any future fixed-geometry colour picture mode): send a colour
		// raster; the daemon fits it to the submode's native geometry. The source
		// box is generous enough to cover the largest SSTV raster (800×616) without
		// upscaling. The watchdog budget spans the slowest SSTV modes (~5 min).
		w, h := fitBox(b.Dx(), b.Dy(), 800, 616)
		return pictureSend{
			width:  uint32(w),
			height: uint32(h),
			rgb:    resampleToRGB(img, w, h),
			color:  true,
			txSecs: 320,
		}, true
	}
}

// wefaxSend builds a grayscale WEFAX raster: rows are capped (maxRows) to keep the
// on-air facsimile time bounded, preserving aspect. secPerLine sizes the watchdog
// estimate (wefax576 runs 120 lpm = 0.5 s/line; wefax288 runs 60 lpm = 1 s/line).
func wefaxSend(img image.Image, b image.Rectangle, maxW, maxRows int, secPerLine float64) pictureSend {
	w, h := fitBox(b.Dx(), b.Dy(), maxW, maxRows)
	// APT tones + 20 phasing lines + start/stop overhead, generously rounded.
	overhead := float64(24)*secPerLine + 4
	return pictureSend{
		width:  uint32(w),
		height: uint32(h),
		rgb:    resampleToRGB(img, w, h),
		color:  false, // WEFAX is grayscale; the daemon folds RGB to luma.
		txSecs: float64(h)*secPerLine + overhead,
	}
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
