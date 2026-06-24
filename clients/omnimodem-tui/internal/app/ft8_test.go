package app

import (
	"testing"
	"time"
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
