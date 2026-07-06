package ui

import (
	"fmt"
	"image"
	"os"
	"path/filepath"
	"sort"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

// PickerAction is the outcome of feeding one key to the ImagePicker.
type PickerAction int

const (
	PickerNone      PickerAction = iota // still open, keep routing keys here
	PickerCancelled                     // esc — dismiss with no selection
	PickerSelected                      // a file was chosen; call Selected() for the path
)

// imageExts are the file suffixes the picker lists and can preview.
var imageExts = map[string]bool{".png": true, ".jpg": true, ".jpeg": true, ".gif": true}

// ImagePicker is a self-contained file browser with a live truecolor preview,
// styled as a DOS dialog. It walks the real filesystem (directories + image
// files only), previews the highlighted image as half-block art, and reports a
// chosen path back to the host view. It owns no TX or network concerns — the
// caller reads Selected() and does what it likes with the path.
type ImagePicker struct {
	dir     string
	entries []pentry
	cursor  int
	top     int // first visible row (scroll offset)
	loadErr error

	// preview cache for the entry under the cursor
	prevPath string
	prevImg  image.Image
	prevErr  error
	prevW    int
	prevH    int
	prevSize int64

	selected string // full path once PickerSelected fires
}

type pentry struct {
	name  string
	isDir bool
	size  int64
}

// NewImagePicker opens the browser at start (falling back to the working dir,
// then "/"), listing that directory's subdirectories and image files.
func NewImagePicker(start string) *ImagePicker {
	if start == "" {
		if wd, err := os.Getwd(); err == nil {
			start = wd
		} else {
			start = "/"
		}
	}
	if abs, err := filepath.Abs(start); err == nil {
		start = abs
	}
	p := &ImagePicker{dir: start}
	p.load()
	return p
}

// load reads the current directory, keeping subdirectories and image files, and
// resets the cursor to the top. A ".." entry leads back up unless we're at root.
func (p *ImagePicker) load() {
	p.entries = nil
	p.cursor = 0
	p.top = 0
	ents, err := os.ReadDir(p.dir)
	if err != nil {
		p.loadErr = err
		p.refreshPreview()
		return
	}
	p.loadErr = nil
	if parent := filepath.Dir(p.dir); parent != p.dir {
		p.entries = append(p.entries, pentry{name: "..", isDir: true})
	}
	var dirs, files []pentry
	for _, e := range ents {
		name := e.Name()
		if strings.HasPrefix(name, ".") {
			continue // hide dotfiles; keep the browser uncluttered
		}
		if e.IsDir() {
			dirs = append(dirs, pentry{name: name, isDir: true})
			continue
		}
		if imageExts[strings.ToLower(filepath.Ext(name))] {
			var size int64
			if fi, err := e.Info(); err == nil {
				size = fi.Size()
			}
			files = append(files, pentry{name: name, size: size})
		}
	}
	sort.Slice(dirs, func(i, j int) bool { return dirs[i].name < dirs[j].name })
	sort.Slice(files, func(i, j int) bool { return files[i].name < files[j].name })
	p.entries = append(p.entries, dirs...)
	p.entries = append(p.entries, files...)
	p.refreshPreview()
}

// Update advances the picker for one key. It returns PickerSelected when a file
// is chosen (path via Selected()), PickerCancelled on esc, else PickerNone.
func (p *ImagePicker) Update(msg tea.KeyMsg) PickerAction {
	switch msg.String() {
	case "esc":
		return PickerCancelled
	case "up", "k", "ctrl+p":
		p.move(-1)
	case "down", "j", "ctrl+n":
		p.move(1)
	case "pgup":
		p.move(-10)
	case "pgdown":
		p.move(10)
	case "home", "g":
		p.move(-len(p.entries))
	case "end", "G":
		p.move(len(p.entries))
	case "left", "h", "backspace":
		p.up()
	case "right", "l":
		if e := p.cur(); e != nil && e.isDir {
			p.enter()
		}
	case "enter":
		if e := p.cur(); e != nil {
			if e.isDir {
				p.enter()
			} else {
				p.selected = filepath.Join(p.dir, e.name)
				return PickerSelected
			}
		}
	}
	return PickerNone
}

// Selected is the full path chosen once Update returned PickerSelected.
func (p *ImagePicker) Selected() string { return p.selected }

// Dir is the directory currently being browsed (useful for tests and hints).
func (p *ImagePicker) Dir() string { return p.dir }

func (p *ImagePicker) cur() *pentry {
	if p.cursor < 0 || p.cursor >= len(p.entries) {
		return nil
	}
	return &p.entries[p.cursor]
}

func (p *ImagePicker) move(d int) {
	if len(p.entries) == 0 {
		return
	}
	p.cursor = clamp(p.cursor+d, 0, len(p.entries)-1)
	p.refreshPreview()
}

// enter descends into the highlighted subdirectory.
func (p *ImagePicker) enter() {
	e := p.cur()
	if e == nil || !e.isDir {
		return
	}
	if e.name == ".." {
		p.up()
		return
	}
	p.dir = filepath.Join(p.dir, e.name)
	p.load()
}

// up climbs to the parent directory, leaving the cursor at the top.
func (p *ImagePicker) up() {
	parent := filepath.Dir(p.dir)
	if parent == p.dir {
		return // already at root
	}
	p.dir = parent
	p.load()
}

// refreshPreview decodes the highlighted image (if it is one) into the cache,
// skipping the work when the cursor hasn't moved off the same file.
func (p *ImagePicker) refreshPreview() {
	e := p.cur()
	if e == nil || e.isDir {
		p.prevPath, p.prevImg, p.prevErr = "", nil, nil
		return
	}
	path := filepath.Join(p.dir, e.name)
	if path == p.prevPath {
		return
	}
	p.prevPath = path
	p.prevSize = e.size
	img, err := DecodeImageFile(path)
	p.prevImg, p.prevErr = img, err
	if img != nil {
		b := img.Bounds()
		p.prevW, p.prevH = b.Dx(), b.Dy()
	}
}

// View renders the whole dialog box (Modal-wrapped, not yet centered) sized to an
// outer width w with body height h rows for the browser/preview panes. The host
// view centers the returned box over its surface.
func (p *ImagePicker) View(w, h int) string {
	if w < 40 {
		w = 40
	}
	if h < 6 {
		h = 6
	}
	inner := w - 4 // Modal chrome: border(2)+padding(2)
	const gap = 2
	listW := inner * 2 / 5
	if listW < 16 {
		listW = 16
	}
	if listW > 34 {
		listW = 34
	}
	prevW := inner - listW - gap

	path := Dim.Render(compactPath(p.dir, inner))
	list := p.renderList(listW, h)
	preview := p.renderPreview(prevW, h)
	gapBlock := lipgloss.NewStyle().Background(ColorPanel).Width(gap).Height(h).Render("")
	panes := lipgloss.JoinHorizontal(lipgloss.Top, list, gapBlock, preview)

	hint := Dim.Render("↑/↓ move · → open dir · enter send · ← up · esc cancel")
	body := path + "\n" + panes + "\n" + hint
	return Modal("Send a picture", body, w)
}

// renderList draws the file column: a full-width blue selection bar on the
// cursor row, directories in yellow with a trailing slash, files in white.
func (p *ImagePicker) renderList(w, h int) string {
	base := lipgloss.NewStyle().Background(ColorPanel).Width(w)
	if p.loadErr != nil {
		return base.Height(h).Foreground(ColorError).Render("cannot read directory:\n" + p.loadErr.Error())
	}
	if len(p.entries) == 0 {
		return base.Height(h).Foreground(ColorDim).Render("(no images here)")
	}
	// Keep the cursor inside the visible window.
	if p.cursor < p.top {
		p.top = p.cursor
	}
	if p.cursor >= p.top+h {
		p.top = p.cursor - h + 1
	}
	end := min(p.top+h, len(p.entries))
	var rows []string
	for i := p.top; i < end; i++ {
		e := p.entries[i]
		label := e.name
		if e.isDir {
			label += "/"
		}
		label = " " + truncate(label, w-1)
		row := lipgloss.NewStyle().Background(ColorPanel).Foreground(ColorFg).Width(w)
		switch {
		case i == p.cursor:
			row = row.Background(ColorSel).Foreground(ColorFg).Bold(true)
		case e.isDir:
			row = row.Foreground(ColorTitle)
		}
		rows = append(rows, row.Render(label))
	}
	// Pad the column to full height so the preview pane lines up.
	for len(rows) < h {
		rows = append(rows, base.Render(""))
	}
	return strings.Join(rows, "\n")
}

// renderPreview draws the truecolor thumbnail plus a metadata footer, or a
// placeholder when nothing is selected or the file won't decode.
func (p *ImagePicker) renderPreview(w, h int) string {
	center := func(s string, fg lipgloss.Color) string {
		return lipgloss.Place(w, h, lipgloss.Center, lipgloss.Center,
			lipgloss.NewStyle().Foreground(fg).Background(ColorPanel).Render(s),
			lipgloss.WithWhitespaceBackground(ColorPanel))
	}
	e := p.cur()
	if e == nil || e.isDir {
		return center("choose an image\nto preview it", ColorDim)
	}
	if p.prevErr != nil {
		return center("cannot preview\n"+truncate(p.prevErr.Error(), w-2), ColorError)
	}
	if p.prevImg == nil {
		return center("(decoding…)", ColorDim)
	}
	const footerH = 2 // filename + dimensions/size line
	imgRows := h - footerH
	if imgRows < 1 {
		imgRows = 1
	}
	art := RenderImageHalfBlock(p.prevImg, w, imgRows)
	art = lipgloss.Place(w, imgRows, lipgloss.Center, lipgloss.Center, art,
		lipgloss.WithWhitespaceBackground(ColorPanel))
	name := lipgloss.NewStyle().Background(ColorPanel).Foreground(ColorFg).Bold(true).
		Width(w).Align(lipgloss.Center).Render(truncate(e.name, w))
	meta := fmt.Sprintf("%d×%d · %s", p.prevW, p.prevH, humanSize(p.prevSize))
	metaLine := lipgloss.NewStyle().Background(ColorPanel).Foreground(ColorAccent).
		Width(w).Align(lipgloss.Center).Render(meta)
	return art + "\n" + name + "\n" + metaLine
}

// --- small helpers ---

func clamp(v, lo, hi int) int {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}

func truncate(s string, w int) string {
	if w <= 0 {
		return ""
	}
	if lipgloss.Width(s) <= w {
		return s
	}
	if w == 1 {
		return "…"
	}
	// Trim by runes until it fits, then add an ellipsis.
	r := []rune(s)
	for len(r) > 0 && lipgloss.Width(string(r))+1 > w {
		r = r[:len(r)-1]
	}
	return string(r) + "…"
}

// compactPath shortens a long directory path from the left so the tail (the part
// that changes as you navigate) stays visible.
func compactPath(path string, w int) string {
	if lipgloss.Width(path) <= w {
		return path
	}
	r := []rune(path)
	keep := w - 1
	if keep < 1 {
		keep = 1
	}
	return "…" + string(r[len(r)-keep:])
}

func humanSize(n int64) string {
	switch {
	case n >= 1<<20:
		return fmt.Sprintf("%.1f MB", float64(n)/(1<<20))
	case n >= 1<<10:
		return fmt.Sprintf("%.1f KB", float64(n)/(1<<10))
	default:
		return fmt.Sprintf("%d B", n)
	}
}
