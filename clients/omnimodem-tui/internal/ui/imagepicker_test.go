package ui

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

// writePNG writes a small solid PNG to path for picker tests.
func writePNG(t *testing.T, path string, w, h int) {
	t.Helper()
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			img.Set(x, y, color.RGBA{uint8(x * 8), uint8(y * 8), 128, 255})
		}
	}
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
}

// pickerFixture builds a temp dir with a subdir, two images, and a non-image
// file that must be filtered out of the listing.
func pickerFixture(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.Mkdir(filepath.Join(dir, "album"), 0o755); err != nil {
		t.Fatal(err)
	}
	writePNG(t, filepath.Join(dir, "alpha.png"), 8, 6)
	writePNG(t, filepath.Join(dir, "bravo.png"), 8, 6)
	if err := os.WriteFile(filepath.Join(dir, "notes.txt"), []byte("hi"), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

func esc() tea.KeyMsg { return tea.KeyMsg{Type: tea.KeyEsc} }

func TestPickerListsDirsAndImagesOnly(t *testing.T) {
	dir := pickerFixture(t)
	p := NewImagePicker(dir)
	var names []string
	for _, e := range p.entries {
		names = append(names, e.name)
	}
	joined := strings.Join(names, ",")
	for _, want := range []string{"..", "album", "alpha.png", "bravo.png"} {
		if !contains(names, want) {
			t.Fatalf("entry %q missing from %q", want, joined)
		}
	}
	if contains(names, "notes.txt") {
		t.Fatalf("non-image notes.txt must be filtered out: %q", joined)
	}
}

func TestPickerSelectsFileAndPreviews(t *testing.T) {
	dir := pickerFixture(t)
	p := NewImagePicker(dir)
	// Walk down to the first image file and confirm a preview decoded.
	for p.cur() != nil && p.cur().isDir {
		p.Update(key("down"))
	}
	if p.prevImg == nil {
		t.Fatal("highlighting an image should decode a preview")
	}
	if act := p.Update(key("enter")); act != PickerSelected {
		t.Fatalf("enter on a file should select it, got %v", act)
	}
	if !strings.HasSuffix(p.Selected(), ".png") {
		t.Fatalf("selected path should be the .png, got %q", p.Selected())
	}
}

func TestPickerNavigatesIntoAndOutOfDir(t *testing.T) {
	dir := pickerFixture(t)
	p := NewImagePicker(dir)
	// Move to "album" (skip "..") and enter it.
	p.Update(key("down"))
	if p.cur().name != "album" {
		t.Fatalf("cursor should be on album, got %q", p.cur().name)
	}
	p.Update(key("enter"))
	if filepath.Base(p.Dir()) != "album" {
		t.Fatalf("enter on a dir should descend, dir=%q", p.Dir())
	}
	p.Update(key("left")) // back up to the fixture root
	if p.Dir() != dir {
		t.Fatalf("left should climb back to %q, got %q", dir, p.Dir())
	}
}

func TestPickerEscCancels(t *testing.T) {
	p := NewImagePicker(pickerFixture(t))
	if act := p.Update(esc()); act != PickerCancelled {
		t.Fatalf("esc should cancel, got %v", act)
	}
}

func TestPickerViewHasTitleAndBorder(t *testing.T) {
	out := NewImagePicker(pickerFixture(t)).View(80, 16)
	if !strings.Contains(out, "Send a picture") {
		t.Fatalf("picker view should show its title:\n%s", out)
	}
	if !strings.Contains(out, "═") {
		t.Fatalf("picker view should draw the modal border:\n%s", out)
	}
}

func contains(ss []string, want string) bool {
	for _, s := range ss {
		if s == want {
			return true
		}
	}
	return false
}
