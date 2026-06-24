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
