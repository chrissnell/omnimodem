package app

import (
	"fmt"
	"strings"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
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
		s[i] = lipgloss.NewStyle().Foreground(lipgloss.Color(c))
	}
	return s
}()

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

// render draws up to `rows` lines of waterfall history (resampled to `width`),
// newest at the bottom, with blank lines padding the top until enough history
// accumulates — so it occupies a fixed block that doesn't jump around.
func (w *waterfall) render(width, rows int) string {
	if width <= 0 || rows <= 0 {
		return ""
	}
	var b strings.Builder
	have := len(w.rows)
	for i := 0; i < rows; i++ {
		idx := have - rows + i // bottom row is the newest frame
		if idx < 0 {
			b.WriteString(strings.Repeat(" ", width))
		} else {
			b.WriteString(spectrumLine(w.rows[idx], width))
		}
		if i < rows-1 {
			b.WriteByte('\n')
		}
	}
	return b.String()
}

// spectrumLine renders one frame's bins into `width` density glyphs.
func spectrumLine(bins []byte, width int) string {
	if len(bins) == 0 {
		return strings.Repeat(" ", width)
	}
	// Color each glyph by intensity, coalescing consecutive same-color cells into
	// one styled span so a line emits few escape codes. Blank (silent) cells get
	// no color — a space carries no ink anyway.
	var out strings.Builder
	var run strings.Builder
	cur := -1 // current colormap index, -1 == uncolored
	flush := func() {
		if run.Len() == 0 {
			return
		}
		if cur < 0 {
			out.WriteString(run.String())
		} else {
			out.WriteString(wfStyles[cur].Render(run.String()))
		}
		run.Reset()
	}
	for x := 0; x < width; x++ {
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
func (w *waterfall) axis(width int) string {
	if w.freqStep == 0 || len(w.rows) == 0 {
		return ui.Accent.Render(" waterfall idle — transmit, or feed a signal to the RX device")
	}
	n := len(w.rows[len(w.rows)-1])
	lo := int(w.freqStart)
	if lo < 0 {
		lo = 0
	}
	hi := int(w.freqStart + w.freqStep*float32(n))
	left := fmt.Sprintf("%d Hz", lo)
	right := fmt.Sprintf("%d Hz", hi)
	gap := width - len(left) - len(right)
	if gap < 1 {
		gap = 1
	}
	return ui.Dim.Render(left + strings.Repeat(" ", gap) + right)
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

func disableSpectrumCmd(c client.ModemClient, ch uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		_, _ = c.ConfigureSpectrum(ctx, &pb.ConfigureSpectrumRequest{Channel: ch, Enable: false})
		return rpcOKMsg{what: "spectrum-off"}
	}
}
