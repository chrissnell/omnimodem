package app

import (
	"strings"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

// ramp maps a 0..255 intensity to a density glyph (low→high).
var ramp = []rune{' ', '·', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'}

type waterfall struct {
	last      *pb.SpectrumFrame
	freqStart float32
	freqStep  float32
	enabled   bool
}

func (w *waterfall) push(f *pb.SpectrumFrame) {
	w.last = f
	w.freqStart = f.GetFreqStartHz()
	w.freqStep = f.GetFreqStepHz()
}

// line renders the latest spectrum into `width` glyphs (resampling bins to fit).
func (w *waterfall) line(width int) string {
	if width <= 0 {
		return ""
	}
	if w.last == nil || len(w.last.GetBins()) == 0 {
		return strings.Repeat(" ", width)
	}
	bins := w.last.GetBins()
	var b strings.Builder
	for x := 0; x < width; x++ {
		bi := x * len(bins) / width
		v := bins[bi]
		g := ramp[int(v)*(len(ramp)-1)/255]
		b.WriteRune(g)
	}
	return b.String()
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
