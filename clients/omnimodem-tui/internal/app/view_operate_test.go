package app

import (
	"strings"
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

func TestOperateSendTransmits(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.live[0] = &chanLive{mode: "psk31"}
	m.sel = 0
	v := newOperateView(m)
	v.compose = "CQ CQ de NW5W"
	// Enter → begin TX (acquire lease)
	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyEnter}); cmd != nil {
		cmd()
	}
	if v.tx.phase != txAcquiring {
		t.Fatalf("enter should start TX, phase=%v", v.tx.phase)
	}
	// lease granted → transmit
	if _, cmd := v.Update(leaseMsg{&pb.TxLeaseResponse{Granted: true}}); cmd != nil {
		cmd()
	}
	if len(f.TransmitCalls) != 1 || string(f.TransmitCalls[0].GetPayload()) != "CQ CQ de NW5W" {
		t.Fatalf("transmit payload wrong: %+v", f.TransmitCalls)
	}
	if !strings.Contains(v.Render(60, 10), "CQ CQ de NW5W") {
		t.Fatal("transcript should show the sent line")
	}
}
