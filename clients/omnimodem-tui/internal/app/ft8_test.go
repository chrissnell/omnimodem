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
	if p := slotPosition(at); p < 6.9 || p > 7.1 {
		t.Fatalf("slot pos at :07 = %v, want ~7", p)
	}
}

func TestQSOLogAppend(t *testing.T) {
	var lg qsoLog
	lg.add("W1AW", "FN31", "-10")
	if len(lg.entries) != 1 || lg.entries[0].call != "W1AW" {
		t.Fatalf("log entry wrong: %+v", lg.entries)
	}
}

// Regression for the code-review finding: calling CQ must not advance the
// ladder, and a QSO must be logged exactly once (at RR73), not on every send.
func TestFT8SendCQDoesNotAdvanceAndLogsOnce(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "ft8"}
	m.sel = 0
	m.enterOperate()
	if m.op.seq == nil {
		t.Fatal("ft8 mode should attach a sequencer")
	}

	// No DX worked: repeated CQ must keep step at 0.
	m.ft8Send()
	m.op.tx.onComplete() // clear active so the next send proceeds
	m.ft8Send()
	if m.op.seq.step != 0 {
		t.Fatalf("calling CQ must not advance the ladder, step=%d", m.op.seq.step)
	}

	// Work a DX and walk to RR73; the QSO logs exactly once.
	m.op.seq.target("W1AW", "FN31")
	for i := 0; i < 5; i++ {
		m.ft8Send()
		m.op.tx.onComplete()
	}
	if len(m.op.qlog.entries) != 1 {
		t.Fatalf("QSO should log exactly once, got %d", len(m.op.qlog.entries))
	}
}
