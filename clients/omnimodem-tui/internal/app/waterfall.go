package app

import (
	"fmt"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

// ramp maps a 0..255 intensity to a density glyph (low→high).
var ramp = []rune{' ', '·', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'}

// wfStyles is a classic waterfall colormap (low→high: navy → blue → cyan →
// green → yellow → orange → red), built once from fixed 256-palette indices so
// the colors don't shift with the terminal theme.
var wfStyles = func() []lipgloss.Style {
	idx := []string{"17", "19", "21", "33", "45", "51", "46", "118", "226", "220", "208", "196"}
	s := make([]lipgloss.Style, len(idx))
	for i, c := range idx {
		// Set the true-black background on every cell so the colored glyphs don't
		// leave the surrounding panel's grey showing between styled spans.
		s[i] = lipgloss.NewStyle().Foreground(lipgloss.Color(c)).Background(ui.ColorPanel)
	}
	return s
}()

// wfBlank fills silent cells with a true-black background (a bare space would
// otherwise expose the terminal's default background between colored spans).
var wfBlank = lipgloss.NewStyle().Background(ui.ColorPanel)

// wfColorIdx maps a 0..255 intensity to a colormap index.
func wfColorIdx(v byte) int {
	i := int(v) * len(wfStyles) / 256
	if i >= len(wfStyles) {
		i = len(wfStyles) - 1
	}
	return i
}

// wfHistory bounds how many recent spectrum lines the waterfall keeps.
const wfHistory = 16

type waterfall struct {
	rows      [][]byte // recent frames' bins, oldest first; newest appended
	freqStart float32
	freqStep  float32
}

func (w *waterfall) push(f *pb.SpectrumFrame) {
	w.freqStart = f.GetFreqStartHz()
	w.freqStep = f.GetFreqStepHz()
	// Push every frame so the waterfall scrolls continuously: a transmission
	// scrolls up and off, and an idle channel flattens to the noise floor (black
	// for a digitally-silent input) instead of freezing on the last burst.
	w.rows = append(w.rows, f.GetBins())
	if len(w.rows) > wfHistory {
		w.rows = w.rows[len(w.rows)-wfHistory:]
	}
}

// pushBlank scrolls a silent line in — used to drain the TX waterfall to black
// between transmissions so a finished burst scrolls off instead of pausing.
func (w *waterfall) pushBlank() {
	n := 64
	if len(w.rows) > 0 {
		n = len(w.rows[len(w.rows)-1])
	}
	w.rows = append(w.rows, make([]byte, n))
	if len(w.rows) > wfHistory {
		w.rows = w.rows[len(w.rows)-wfHistory:]
	}
}

// hasSignal reports whether any retained line still carries signal (above the
// silence floor), so the drain knows when the pane has fully scrolled to black.
func (w *waterfall) hasSignal() bool {
	for _, r := range w.rows {
		for _, v := range r {
			if v >= 2 {
				return true
			}
		}
	}
	return false
}

// render draws up to `rows` lines of waterfall history (resampled to `width`),
// newest at the bottom, with blank lines padding the top until enough history
// accumulates — so it occupies a fixed block that doesn't jump around.
func (w *waterfall) render(width, rows int) string {
	return w.renderCursor(width, rows, -1)
}

// renderCursor is render with a demod-channel marker painted down `cursorCol`
// (a display column, from cursorColumn). cursorCol < 0 draws no marker, so
// render delegates here with -1.
func (w *waterfall) renderCursor(width, rows, cursorCol int) string {
	if width <= 0 || rows <= 0 {
		return ""
	}
	var b strings.Builder
	have := len(w.rows)
	for i := 0; i < rows; i++ {
		idx := have - rows + i // bottom row is the newest frame
		if idx < 0 {
			b.WriteString(spectrumLineCursor(nil, width, cursorCol))
		} else {
			b.WriteString(spectrumLineCursor(w.rows[idx], width, cursorCol))
		}
		if i < rows-1 {
			b.WriteByte('\n')
		}
	}
	return b.String()
}

// wfCursor styles the demod-channel marker column: bright white on the panel
// background so it reads over any waterfall color.
var wfCursor = lipgloss.NewStyle().Foreground(lipgloss.Color("231")).Background(ui.ColorPanel).Bold(true)

// cursorColumn maps an absolute RF frequency to a display column in a
// `width`-wide waterfall line, via the current frame's freqStart/freqStep axis.
// Returns -1 when there is no axis yet or the frequency falls outside the shown
// span, so callers can skip drawing a marker cleanly. It bins against the newest
// frame's width; if the daemon reconfigures the bin count mid-stream, older
// retained rows can be a column off until they scroll out (wfHistory frames).
func (w *waterfall) cursorColumn(width int, freqHz float64) int {
	if width <= 0 || w.freqStep == 0 || len(w.rows) == 0 {
		return -1
	}
	nBins := len(w.rows[len(w.rows)-1])
	if nBins == 0 {
		return -1
	}
	bin := (freqHz - float64(w.freqStart)) / float64(w.freqStep)
	if bin < 0 || bin >= float64(nBins) {
		return -1
	}
	// spectrumLine samples bin[x*nBins/width] at column x; invert that so the
	// marker lands on the column that renders the target bin.
	col := int(bin * float64(width) / float64(nBins))
	if col < 0 {
		col = 0
	}
	if col >= width {
		col = width - 1
	}
	return col
}

// spectrumLine renders one frame's bins into `width` density glyphs.
func spectrumLine(bins []byte, width int) string {
	return spectrumLineCursor(bins, width, -1)
}

// spectrumLineCursor is spectrumLine with a marker glyph forced at cursorCol
// (a display column). cursorCol < 0 draws no marker, so spectrumLine passes -1.
func spectrumLineCursor(bins []byte, width, cursorCol int) string {
	if len(bins) == 0 {
		if cursorCol < 0 || cursorCol >= width {
			return wfBlank.Render(strings.Repeat(" ", width))
		}
		// No signal yet, but still show where the demod channel sits.
		return wfBlank.Render(strings.Repeat(" ", cursorCol)) +
			wfCursor.Render("│") +
			wfBlank.Render(strings.Repeat(" ", width-cursorCol-1))
	}
	// Color each glyph by intensity, coalescing consecutive same-color cells into
	// one styled span so a line emits few escape codes. Silent cells render on the
	// black background so the line is solid black end to end. The cursor column
	// breaks any run and emits its own marker span.
	var out strings.Builder
	var run strings.Builder
	cur := -1 // current colormap index, -1 == uncolored
	flush := func() {
		if run.Len() == 0 {
			return
		}
		if cur < 0 {
			out.WriteString(wfBlank.Render(run.String()))
		} else {
			out.WriteString(wfStyles[cur].Render(run.String()))
		}
		run.Reset()
	}
	for x := 0; x < width; x++ {
		if x == cursorCol {
			flush()
			out.WriteString(wfCursor.Render("│"))
			cur = -1
			continue
		}
		v := bins[x*len(bins)/width]
		g := ramp[int(v)*(len(ramp)-1)/255]
		col := -1
		if g != ' ' {
			col = wfColorIdx(v)
		}
		if col != cur {
			flush()
			cur = col
		}
		run.WriteRune(g)
	}
	flush()
	return out.String()
}

// axis labels the displayed frequency span under the waterfall.
// axis labels colored like the rest of the panel: red numbers, green "Hz" units,
// all on the black background.
var (
	wfNum  = lipgloss.NewStyle().Foreground(lipgloss.Color("196")).Background(ui.ColorPanel) // red
	wfUnit = lipgloss.NewStyle().Foreground(lipgloss.Color("46")).Background(ui.ColorPanel)  // green
)

func (w *waterfall) axis(width int) string {
	if w.freqStep == 0 || len(w.rows) == 0 {
		msg := " waterfall idle — transmit, or feed a signal to the RX device"
		if len(msg) < width {
			msg += strings.Repeat(" ", width-len(msg))
		}
		return lipgloss.NewStyle().Foreground(ui.ColorAccent).Background(ui.ColorPanel).Render(msg)
	}
	n := len(w.rows[len(w.rows)-1])
	lo := int(w.freqStart)
	if lo < 0 {
		lo = 0
	}
	hi := int(w.freqStart + w.freqStep*float32(n))
	loStr := fmt.Sprintf("%d", lo)
	hiStr := fmt.Sprintf("%d", hi)
	// " Hz" suffix is 3 visible cells; size the gap from the plain widths.
	gap := width - (len(loStr) + 3) - (len(hiStr) + 3)
	if gap < 1 {
		gap = 1
	}
	sp := wfBlank.Render(" ")
	label := func(num string) string { return wfNum.Render(num) + sp + wfUnit.Render("Hz") }
	return label(loStr) + wfBlank.Render(strings.Repeat(" ", gap)) + label(hiStr)
}

// enableSpectrumCmd asks the daemon to start the per-channel spectrum stream
// for the operate screen (default sizing; the daemon clamps + echoes actuals).
func enableSpectrumCmd(c client.ModemClient, ch uint32, binCount uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.ConfigureSpectrum(ctx, &pb.ConfigureSpectrumRequest{
			Channel: ch, Enable: true, BinCount: binCount, FreqHiHz: 3000,
		})
		if err != nil {
			return rpcErrMsg{err}
		}
		return spectrumCfgMsg{resp}
	}
}

// enableRFSpectrumCmd starts the wideband RF waterfall for an SDR-bound channel.
// Unlike enableSpectrumCmd it leaves the passband window unset (freq_hi_hz == 0):
// the daemon drives the axis from the source type and emits RF-referenced frames
// spanning the full captured band, so a passband clamp would zoom to a few kHz.
func enableRFSpectrumCmd(c client.ModemClient, ch uint32, binCount uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.ConfigureSpectrum(ctx, &pb.ConfigureSpectrumRequest{
			Channel: ch, Enable: true, BinCount: binCount,
		})
		if err != nil {
			return rpcErrMsg{err}
		}
		return spectrumCfgMsg{resp}
	}
}

func disableSpectrumCmd(c client.ModemClient, ch uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		_, _ = c.ConfigureSpectrum(ctx, &pb.ConfigureSpectrumRequest{Channel: ch, Enable: false})
		return rpcOKMsg{what: "spectrum-off"}
	}
}
