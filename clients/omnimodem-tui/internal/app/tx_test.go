package app

import (
	"testing"
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
)

// The TX watchdog must outlast the daemon's slot-align count-off plus burst, or a
// windowed mode is aborted before it ever keys. JT65/JT9 have a 60 s slot: the
// daemon waits up to a full slot for the boundary, then transmits a ~47 s burst,
// so the old fixed 30 s watchdog fired mid count-off and nothing ever went out.
func TestTxWatchdogCoversSlotCountoff(t *testing.T) {
	// wspr is a 120 s-slot beacon; its enter-keyed beacon TX now runs through the
	// operate view, so its watchdog must cover the 2-slot worst case like the rest.
	for _, tc := range []struct {
		name string
		slot float64
	}{{"ft8", 15}, {"ft4", 7.5}, {"jt65", 60}, {"jt9", 60}, {"wspr", 120}} {
		wd := txWatchdog(tc.slot)
		// Worst case from lease grant to burst completion is one slot of count-off
		// plus a burst that nearly fills a slot, i.e. ~2 slots. Pin the exact value
		// (2 slots + margin) so the safety cushion can't silently shrink.
		worst := time.Duration(2 * tc.slot * float64(time.Second))
		if wd != worst+txWatchdogMargin {
			t.Errorf("%s: watchdog = %v, want %v (2 slots + margin)", tc.name, wd, worst+txWatchdogMargin)
		}
		if wd <= worst {
			t.Errorf("%s: watchdog %v must exceed worst-case airtime %v", tc.name, wd, worst)
		}
	}
	// Streaming (chat) modes key immediately and keep the bare margin.
	if got := txWatchdog(0); got != txWatchdogMargin {
		t.Errorf("streaming watchdog = %v, want %v", got, txWatchdogMargin)
	}
}

// End-to-end guard: a JT65 operate view's watchdog must not expire while the
// daemon is still counting off to the 60 s slot boundary (i.e. before it keys).
func TestJt65WatchdogSurvivesCountoff(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "jt65"}
	m.sel = 0
	v := newOperateView(m)

	v.tx.begin([]byte("CQ K1ABC FN42"))
	v.tx.onLeaseGranted() // clock starts here; daemon now counts off to the slot

	// Even a full 60 s slot of count-off must not trip the watchdog.
	at := v.tx.startedAt.Add(60 * time.Second)
	if v.tx.watchdogExpired(at) {
		t.Fatal("watchdog aborted JT65 during the 60 s slot count-off")
	}
}
