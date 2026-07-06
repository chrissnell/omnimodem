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

func imageOperateView(t *testing.T, f client.ModemClient) *operateView {
	t.Helper()
	m := New(f, "x")
	m.live[0] = &chanLive{mode: "feldhell"} // an image-shape (facsimile) mode
	m.sel = 0
	v := newOperateView(m)
	if v.raster == nil {
		t.Fatal("feldhell should build the image/raster surface")
	}
	return v
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
// pressing enter then transmits the raw image bytes over the mode.
func TestImageStageAndTransmit(t *testing.T) {
	dir := t.TempDir()
	pngPath := filepath.Join(dir, "pic.png")
	writeTestPNG(t, pngPath)

	f := &client.Fake{}
	v := imageOperateView(t, f)

	v.stageImage(pngPath)
	if v.staged == nil {
		t.Fatal("stageImage should populate the staged slot")
	}
	if v.staged.img == nil {
		t.Fatal("staged image should decode for preview")
	}
	raw, _ := os.ReadFile(pngPath)
	if len(v.staged.bytes) != len(raw) {
		t.Fatalf("staged bytes = %d, want the file's %d", len(v.staged.bytes), len(raw))
	}

	// The operate surface previews the staged picture before TX.
	if out := v.Render(100, 24); !strings.Contains(out, "Ready to send") || !strings.Contains(out, "pic.png") {
		t.Fatalf("staged preview should show the picture name:\n%s", out)
	}

	// Enter transmits: lease → Transmit(image bytes).
	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyEnter}); cmd != nil {
		cmd()
	}
	if v.tx.phase != txAcquiring {
		t.Fatalf("enter with a staged image should start TX, phase=%v", v.tx.phase)
	}
	if _, cmd := v.Update(leaseMsg{&pb.TxLeaseResponse{Granted: true}}); cmd != nil {
		cmd()
	}
	if len(f.TransmitCalls) != 1 {
		t.Fatalf("expected one Transmit call, got %d", len(f.TransmitCalls))
	}
	if got := len(f.TransmitCalls[0].Payload); got != len(raw) {
		t.Fatalf("transmitted %d bytes, want the image's %d", got, len(raw))
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
