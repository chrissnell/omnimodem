package app

import (
	"testing"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// The colormap must span its full range: silence at the low end, peak at the top.
func TestWaterfallColorIdx(t *testing.T) {
	if got := wfColorIdx(0); got != 0 {
		t.Fatalf("silence must map to the lowest color, got %d", got)
	}
	if got := wfColorIdx(255); got != len(wfStyles)-1 {
		t.Fatalf("max intensity must map to the highest color (%d), got %d", len(wfStyles)-1, got)
	}
}

func TestWaterfallHoldsThroughSilence(t *testing.T) {
	var w waterfall
	sig := make([]byte, 8)
	for i := range sig {
		sig[i] = 200
	}
	silent := make([]byte, 8) // all zero
	w.push(&pb.SpectrumFrame{Bins: sig, FreqStepHz: 1})
	w.push(&pb.SpectrumFrame{Bins: silent, FreqStepHz: 1})
	w.push(&pb.SpectrumFrame{Bins: silent, FreqStepHz: 1})
	if len(w.rows) != 1 {
		t.Fatalf("silent frames must be skipped so the burst stays visible, rows=%d", len(w.rows))
	}
}
