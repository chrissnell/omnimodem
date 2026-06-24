package app

import (
	"testing"
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

func TestTxLifecycle(t *testing.T) {
	tx := &txState{}
	if tx.phase != txIdle {
		t.Fatal("starts idle")
	}
	tx.begin([]byte("CQ"))
	if tx.phase != txAcquiring {
		t.Fatal("begin → acquiring")
	}
	tx.onLeaseGranted()
	if tx.phase != txTransmitting {
		t.Fatal("lease → transmitting")
	}
	tx.onComplete()
	if tx.phase != txIdle {
		t.Fatal("complete → idle")
	}
}

func TestTxWatchdogTrips(t *testing.T) {
	tx := &txState{watchdog: 10 * time.Second}
	tx.begin([]byte("CQ"))
	tx.onLeaseGranted()
	tx.startedAt = time.Now().Add(-11 * time.Second)
	if !tx.watchdogExpired(time.Now()) {
		t.Fatal("watchdog should expire after ceiling")
	}
}

func TestMacroExpand(t *testing.T) {
	ctx := macroCtx{myCall: "NW5W", theirCall: "W1AW", rst: "599"}
	if got := expandMacro("{mycall} de {call} ur {rst}", ctx); got != "NW5W de W1AW ur 599" {
		t.Fatalf("expand = %q", got)
	}
}

func TestWaterfallLine(t *testing.T) {
	wf := &waterfall{}
	wf.push(&pb.SpectrumFrame{Bins: []byte{0, 64, 128, 255}})
	runes := []rune(wf.line(4))
	if runes[0] != ' ' {
		t.Fatalf("bin 0 should be blank, got %q", string(runes[0]))
	}
	if runes[3] != '█' {
		t.Fatalf("bin 255 should be full block, got %q", string(runes[3]))
	}
}

func TestOperateSendTransmits(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.live[0] = &chanLive{mode: "psk31"}
	m.sel = 0
	m.enterOperate()
	m.op.compose = "CQ CQ de NW5W"
	// Enter → begin TX (acquire lease)
	if _, cmd := m.updateOperate(tea.KeyMsg{Type: tea.KeyEnter}); cmd != nil {
		cmd()
	}
	if m.op.tx.phase != txAcquiring {
		t.Fatalf("enter should start TX, phase=%v", m.op.tx.phase)
	}
	// lease granted → transmit
	if _, cmd := m.updateOperate(leaseMsg{&pb.TxLeaseResponse{Granted: true}}); cmd != nil {
		cmd()
	}
	if len(f.TransmitCalls) != 1 || string(f.TransmitCalls[0].GetPayload()) != "CQ CQ de NW5W" {
		t.Fatalf("transmit payload wrong: %+v", f.TransmitCalls)
	}
	if !contains(m.viewOperate(), "CQ CQ de NW5W") {
		t.Fatalf("transcript should show the sent line")
	}
}
