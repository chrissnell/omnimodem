package app

import (
	"strings"
	"testing"
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

// The panes show centered status messages: "waterfall idle" on an empty TX pane,
// and "RX channel muted during TX" on the RX pane while transmitting.
func TestOperatePaneMessages(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "psk31"}
	m.sel = 0
	v := newOperateView(m)
	if !strings.Contains(v.Render(80, 16), "waterfall idle") {
		t.Fatal("idle TX pane should show 'waterfall idle'")
	}
	v.tx.begin([]byte("CQ"))
	if !strings.Contains(v.Render(80, 16), "RX channel muted during TX") {
		t.Fatal("RX pane should show the muted message during TX")
	}
}

// When TX ends, the TX waterfall must scroll off to black, not pause on its last
// line: an idle tick starts the drain and drain ticks clear the pane.
func TestTXWaterfallDrainsWhenIdle(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "psk31"}
	m.sel = 0
	v := newOperateView(m)
	sig := make([]byte, 64)
	for i := range sig {
		sig[i] = 200
	}
	for k := 0; k < 5; k++ {
		v.txWf.push(&pb.SpectrumFrame{Bins: sig, FreqStepHz: 1, Transmit: true})
	}
	if _, cmd := v.Update(tickMsg(time.Now())); cmd == nil {
		t.Fatal("an idle tick with TX content should start the scroll-off drain")
	}
	for i := 0; i < 60 && v.txWf.hasSignal(); i++ {
		v.Update(txDrainMsg{})
	}
	if v.txWf.hasSignal() {
		t.Fatal("TX waterfall should drain to black when not transmitting")
	}
}

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

// Received text (decoded by the daemon and delivered as RxFrame events) must
// appear in the transcript; streaming modes accumulate onto one line.
func TestOperateShowsReceivedText(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "psk31"}
	m.sel = 0
	v := newOperateView(m)
	for _, c := range "CQ TEST" {
		v.Update(eventMsg{&pb.Event{Kind: &pb.Event_RxFrame{RxFrame: &pb.RxFrame{Channel: 0, Data: []byte(string(c))}}}})
	}
	if len(v.transcript) != 1 || v.transcript[0].txt != "CQ TEST" || v.transcript[0].dir != '‹' {
		t.Fatalf("expected one received line 'CQ TEST', got %+v", v.transcript)
	}
}
