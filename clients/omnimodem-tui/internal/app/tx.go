package app

import (
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	tea "github.com/charmbracelet/bubbletea"
)

type txPhase int

const (
	txIdle txPhase = iota
	txAcquiring
	txTransmitting
)

// txState is the per-operate TX lifecycle. The daemon auto-keys PTT for the
// burst, so the client only sequences lease → Transmit → complete → release.
type txState struct {
	phase     txPhase
	payload   []byte
	id        uint64
	startedAt time.Time
	watchdog  time.Duration // 0 = disabled
}

func (t *txState) begin(payload []byte) {
	t.phase = txAcquiring
	t.payload = payload
}
func (t *txState) onLeaseGranted() {
	t.phase = txTransmitting
	t.startedAt = time.Now()
}
func (t *txState) onComplete()  { t.phase = txIdle; t.payload = nil }
func (t *txState) halt()        { t.phase = txIdle; t.payload = nil }
func (t *txState) active() bool { return t.phase != txIdle }
func (t *txState) watchdogExpired(now time.Time) bool {
	return t.watchdog > 0 && t.phase == txTransmitting && now.Sub(t.startedAt) > t.watchdog
}

// txWatchdogMargin covers RPC latency, the burst tail, and clock jitter on top of
// the daemon's slot-align wait. It is also the whole watchdog for streaming (chat)
// modes, which key immediately.
const txWatchdogMargin = 30 * time.Second

// txWatchdog sizes the client-side TX safety timeout for a mode's slot length.
// The clock starts at lease grant, but for windowed modes the daemon then counts
// off to the next slot boundary (up to one slot) before keying and transmits a
// burst that nearly fills a slot — so the worst case from grant to completion is
// ~2 slots. A fixed 30 s watchdog aborted the keyed windowed modes JT65/JT9 (60 s
// slot) mid count-off, so they never keyed. Streaming modes (slotSecs == 0) key at
// once and keep the bare margin. (WSPR's 120 s beacon is now keyed from the operate
// view too, so this sizes its watchdog to 2×120 s + margin.)
func txWatchdog(slotSecs float64) time.Duration {
	if slotSecs <= 0 {
		return txWatchdogMargin
	}
	return time.Duration(2*slotSecs*float64(time.Second)) + txWatchdogMargin
}

// commands that drive the FSM transitions:
func acquireLeaseCmd(c client.ModemClient, ch uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		r, err := c.AcquireTxLease(ctx, ch)
		if err != nil {
			return rpcErrMsg{err}
		}
		return leaseMsg{r}
	}
}
func transmitCmd(c client.ModemClient, ch uint32, payload []byte) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		id, err := c.Transmit(ctx, ch, payload)
		if err != nil {
			return rpcErrMsg{err}
		}
		return transmitMsg{id}
	}
}
func releaseLeaseCmd(c client.ModemClient, ch uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		_ = c.ReleaseTxLease(ctx, ch)
		return rpcOKMsg{what: "lease-released"}
	}
}
