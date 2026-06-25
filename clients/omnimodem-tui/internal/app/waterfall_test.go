package app

import "testing"

// The colormap must span its full range: silence at the low end, peak at the top.
func TestWaterfallColorIdx(t *testing.T) {
	if got := wfColorIdx(0); got != 0 {
		t.Fatalf("silence must map to the lowest color, got %d", got)
	}
	if got := wfColorIdx(255); got != len(wfStyles)-1 {
		t.Fatalf("max intensity must map to the highest color (%d), got %d", len(wfStyles)-1, got)
	}
}
