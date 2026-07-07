package app

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func writeTestPNG(t *testing.T, path string) {
	t.Helper()
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	img := image.NewRGBA(image.Rect(0, 0, 12, 8))
	for y := 0; y < 8; y++ {
		for x := 0; x < 12; x++ {
			img.Set(x, y, color.RGBA{uint8(x * 20), uint8(y * 20), 200, 255})
		}
	}
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
}

func imageOperateViewMode(t *testing.T, f client.ModemClient, mode string) *operateView {
	t.Helper()
	m := New(f, "x")
	m.live[0] = &chanLive{mode: mode} // an image-shape (facsimile) mode
	m.sel = 0
	v := newOperateView(m)
	if v.raster == nil {
		t.Fatalf("%s should build the image/raster surface", mode)
	}
	return v
}

func imageOperateView(t *testing.T, f client.ModemClient) *operateView {
	t.Helper()
	return imageOperateViewMode(t, f, "feldhell")
}

// The daemon reports a channel's mode as its full descriptor
// (e.g. "feldhell:center=1500"), not the bare label. The operate view must
// still resolve that to the image shape, or ctrl+o silently does nothing.
func TestDaemonModeDescriptorBuildsRaster(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "feldhell:center=1500"}
	m.sel = 0
	v := newOperateView(m)
	if v.raster == nil {
		t.Fatal("feldhell:center=1500 should build the image/raster surface")
	}
	if v.modeLabel != "feldhell" {
		t.Fatalf("modeLabel = %q, want the bare label feldhell", v.modeLabel)
	}
	v.Update(tea.KeyMsg{Type: tea.KeyCtrlO})
	if v.picker == nil {
		t.Fatal("ctrl+o should open the picker on a descriptor-form image mode")
	}
}

// ctrl+o opens the picker on a picture mode; the picker then owns all keys.
func TestImageModeOpensPicker(t *testing.T) {
	v := imageOperateView(t, &client.Fake{})
	v.Update(tea.KeyMsg{Type: tea.KeyCtrlO})
	if v.picker == nil {
		t.Fatal("ctrl+o should open the image picker")
	}
	out := v.Render(100, 24)
	if !strings.Contains(out, "Send a picture") {
		t.Fatalf("open picker should render the dialog:\n%s", out)
	}
}

// Selecting a file stages it (with a decoded preview) and closes the picker;
// pressing enter on a WEFAX channel then sends it through TransmitImage as a
// downsampled raster — NOT the text Transmit RPC that the modulator rejects.
func TestImageStageAndTransmit(t *testing.T) {
	dir := t.TempDir()
	pngPath := filepath.Join(dir, "pic.png")
	writeTestPNG(t, pngPath)

	f := &client.Fake{}
	v := imageOperateViewMode(t, f, "wefax576")

	v.stageImage(pngPath)
	if v.staged == nil {
		t.Fatal("stageImage should populate the staged slot")
	}
	if v.staged.img == nil {
		t.Fatal("staged image should decode for preview")
	}

	// The operate surface previews the staged picture before TX.
	if out := v.Render(100, 24); !strings.Contains(out, "Ready to send") || !strings.Contains(out, "pic.png") {
		t.Fatalf("staged preview should show the picture name:\n%s", out)
	}

	// Enter transmits: lease → TransmitImage(raster).
	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyEnter}); cmd != nil {
		cmd()
	}
	if v.tx.phase != txAcquiring {
		t.Fatalf("enter with a staged image should start TX, phase=%v", v.tx.phase)
	}
	if _, cmd := v.Update(leaseMsg{&pb.TxLeaseResponse{Granted: true}}); cmd != nil {
		cmd()
	}
	if len(f.TransmitCalls) != 0 {
		t.Fatalf("a picture must not go through the text Transmit RPC, got %d calls", len(f.TransmitCalls))
	}
	if len(f.TransmitImageCalls) != 1 {
		t.Fatalf("expected one TransmitImage call, got %d", len(f.TransmitImageCalls))
	}
	req := f.TransmitImageCalls[0]
	if req.Width == 0 || req.Height == 0 {
		t.Fatalf("TransmitImage dims must be non-zero, got %dx%d", req.Width, req.Height)
	}
	if req.Color {
		t.Fatal("wefax is grayscale; color flag should be false")
	}
	if want := int(req.Width) * int(req.Height) * 3; len(req.Rgb) != want {
		t.Fatalf("rgb length = %d, want width*height*3 = %d", len(req.Rgb), want)
	}
}

// On a mode the daemon can't carry a picture (the Hell text-raster modes), enter
// surfaces a clear message instead of silently doing nothing (or sending garbage).
func TestImageSendUnsupportedModeToasts(t *testing.T) {
	dir := t.TempDir()
	pngPath := filepath.Join(dir, "pic.png")
	writeTestPNG(t, pngPath)

	f := &client.Fake{}
	v := imageOperateViewMode(t, f, "feldhell")
	v.stageImage(pngPath)
	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyEnter}); cmd != nil {
		cmd()
	}
	if v.tx.active() {
		t.Fatal("feldhell can't send a picture; TX should not start")
	}
	if len(f.TransmitImageCalls) != 0 || len(f.TransmitCalls) != 0 {
		t.Fatal("no transmit RPC should fire for an unsupported picture mode")
	}
	if v.m.toast == nil {
		t.Fatal("an unsupported picture mode should surface a toast")
	}
}

// A picture is a one-shot send: once the transmit completes, the staged slot
// clears so a stray enter can't silently re-transmit the same file.
func TestImageStagedClearsAfterTransmit(t *testing.T) {
	dir := t.TempDir()
	pngPath := filepath.Join(dir, "pic.png")
	writeTestPNG(t, pngPath)

	v := imageOperateViewMode(t, &client.Fake{}, "wefax576")
	v.stageImage(pngPath)
	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyEnter}); cmd != nil {
		cmd()
	}
	v.Update(leaseMsg{&pb.TxLeaseResponse{Granted: true}})
	// Daemon reports the burst finished.
	v.Update(eventMsg{&pb.Event{Kind: &pb.Event_TransmitComplete{TransmitComplete: &pb.TransmitComplete{}}}})
	if v.staged != nil {
		t.Fatal("staged image should clear once its transmit completes")
	}
}

// Staging clears any half-typed compose text, and further keystrokes don't
// silently accumulate behind the preview.
func TestImageStagedSuppressesComposeInput(t *testing.T) {
	dir := t.TempDir()
	pngPath := filepath.Join(dir, "pic.png")
	writeTestPNG(t, pngPath)

	v := imageOperateView(t, &client.Fake{})
	v.compose = "half typed"
	v.stageImage(pngPath)
	if v.compose != "" {
		t.Fatalf("staging should clear compose, got %q", v.compose)
	}
	v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("xyz")})
	if v.compose != "" {
		t.Fatalf("keystrokes while staged must not grow compose, got %q", v.compose)
	}
}

// A file larger than the stage cap is refused rather than read/decoded whole.
func TestImageStageRejectsOversized(t *testing.T) {
	dir := t.TempDir()
	big := filepath.Join(dir, "huge.png")
	f, err := os.Create(big)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(maxStageBytes + 1); err != nil {
		t.Fatal(err)
	}
	f.Close()

	v := imageOperateView(t, &client.Fake{})
	v.stageImage(big)
	if v.staged != nil {
		t.Fatal("an oversized file must not be staged")
	}
}

// ctrl+x clears a staged picture when idle instead of transmitting it.
func TestImageCancelStaged(t *testing.T) {
	dir := t.TempDir()
	pngPath := filepath.Join(dir, "pic.png")
	writeTestPNG(t, pngPath)

	v := imageOperateView(t, &client.Fake{})
	v.stageImage(pngPath)
	v.Update(tea.KeyMsg{Type: tea.KeyCtrlX})
	if v.staged != nil {
		t.Fatal("ctrl+x should clear the staged image when idle")
	}
}
