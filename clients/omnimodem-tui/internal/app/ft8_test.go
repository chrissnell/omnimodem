package app

import (
	"testing"
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
)

func TestFT8LadderAdvance(t *testing.T) {
	seq := newFT8Seq("NW5W", "EM10")
	seq.target("W1AW", "FN31")
	if got := seq.current(); got != "W1AW NW5W EM10" {
		t.Fatalf("Tx1 = %q", got)
	}
	seq.advance()
	if got := seq.current(); got != "W1AW NW5W -10" {
		t.Fatalf("Tx2 = %q", got)
	}
}

func TestSlotPosition(t *testing.T) {
	at := time.Date(2026, 1, 1, 0, 0, 7, 0, time.UTC)
	if p := slotPosition(at, 15); p < 6.9 || p > 7.1 {
		t.Fatalf("slot pos at :07 in a 15 s slot = %v, want ~7", p)
	}
	// FT4's 7.5 s slot: :07 is 7.0 into the second 7.5 s window (0..7.5).
	if p := slotPosition(at, 7.5); p < 6.9 || p > 7.1 {
		t.Fatalf("slot pos at :07 in a 7.5 s slot = %v, want ~7", p)
	}
	// A 60 s slot (JT65/JT9) is anchored to the minute: :07 → 7.
	if p := slotPosition(at, 60); p < 6.9 || p > 7.1 {
		t.Fatalf("slot pos at :07 in a 60 s slot = %v, want ~7", p)
	}
	if p := slotPosition(at, 0); p != 0 {
		t.Fatalf("degenerate period must be 0, got %v", p)
	}
}

func TestQSOLogAppend(t *testing.T) {
	var lg qsoLog
	lg.add("W1AW", "FN31", "-10")
	if len(lg.entries) != 1 || lg.entries[0].call != "W1AW" {
		t.Fatalf("log entry wrong: %+v", lg.entries)
	}
}

// Regression (code review): calling CQ must not advance the ladder, and a QSO
// must be logged exactly once at RR73 — now via the operate view.
func TestOperateFT8SendCQDoesNotAdvanceAndLogsOnce(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "ft8"}
	m.sel = 0
	v := newOperateView(m)
	if v.seq == nil {
		t.Fatal("ft8 mode should attach a sequencer")
	}
	v.ft8Send()
	v.tx.onComplete()
	v.ft8Send()
	if v.seq.step != 0 {
		t.Fatalf("calling CQ must not advance the ladder, step=%d", v.seq.step)
	}
	v.seq.target("W1AW", "FN31")
	for i := 0; i < 5; i++ {
		v.ft8Send()
		v.tx.onComplete()
	}
	if len(v.qlog.entries) != 1 {
		t.Fatalf("QSO should log exactly once, got %d", len(v.qlog.entries))
	}
}
