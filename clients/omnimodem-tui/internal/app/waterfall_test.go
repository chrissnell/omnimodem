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

// Every frame scrolls in (including silent ones) so an idle channel flattens
// instead of freezing on the last burst.
func TestWaterfallScrollsEveryFrame(t *testing.T) {
	var w waterfall
	for i := 0; i < 4; i++ {
		w.push(&pb.SpectrumFrame{Bins: make([]byte, 8), FreqStepHz: 1})
	}
	if len(w.rows) != 4 {
		t.Fatalf("idle frames must still scroll in, rows=%d", len(w.rows))
	}
}
