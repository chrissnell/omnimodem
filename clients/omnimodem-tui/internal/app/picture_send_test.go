package app

import (
	"image"
	"image/color"
	"testing"
)

func solidImage(w, h int) image.Image {
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			img.Set(x, y, color.RGBA{uint8(x), uint8(y), 128, 255})
		}
	}
	return img
}

// A large source is downsampled within the mode's row cap, aspect preserved, and
// the RGB buffer is exactly width*height*3 bytes.
func TestBuildPictureSendWefax576(t *testing.T) {
	ps, ok := buildPictureSend("wefax576", solidImage(2000, 1500))
	if !ok {
		t.Fatal("wefax576 should be picture-capable")
	}
	if ps.height > 280 {
		t.Fatalf("height %d exceeds the wefax576 row cap of 280", ps.height)
	}
	if ps.color {
		t.Fatal("wefax is grayscale")
	}
	if want := int(ps.width) * int(ps.height) * 3; len(ps.rgb) != want {
		t.Fatalf("rgb %d != width*height*3 %d", len(ps.rgb), want)
	}
	// 2000x1500 (4:3) capped to 280 rows → ~373 wide; aspect within a pixel.
	got := float64(ps.width) / float64(ps.height)
	if want := 2000.0 / 1500.0; got < want-0.05 || got > want+0.05 {
		t.Fatalf("aspect %.3f, want ~%.3f", got, want)
	}
	if ps.txSecs <= 0 {
		t.Fatal("txSecs should estimate the on-air duration for the watchdog")
	}
}

// Modes the daemon can't carry a picture on report not-ok so the caller can warn.
func TestBuildPictureSendUnsupported(t *testing.T) {
	if _, ok := buildPictureSend("feldhell", solidImage(64, 64)); ok {
		t.Fatal("feldhell is not a picture-send mode")
	}
}

// Never upscale a source smaller than the cap.
func TestBuildPictureSendNoUpscale(t *testing.T) {
	ps, _ := buildPictureSend("wefax288", solidImage(40, 30))
	if ps.width != 40 || ps.height != 30 {
		t.Fatalf("small source should pass through unscaled, got %dx%d", ps.width, ps.height)
	}
}
