package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

// Toggling the RSID TX switch (space) auto-applies a ConfigureChannel carrying
// the new flag, so the operator's choice reaches the daemon.
func TestConfigRsidToggleAutoApplies(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.saved = v.sig() // baseline as if the current config were already saved
	v.focus = fRsidTx

	_, cmd := v.Update(tea.KeyMsg{Type: tea.KeySpace})
	drainCmd(v, cmd)

	if !v.rsidTx {
		t.Fatal("space on the RSID TX field must toggle it on")
	}
	if len(f.ChannelCalls) == 0 {
		t.Fatal("toggling RSID must auto-apply a ConfigureChannel")
	}
	if last := f.ChannelCalls[len(f.ChannelCalls)-1]; !last.GetRsidTx() {
		t.Fatal("ConfigureChannel must carry rsid_tx=true")
	}
}

// A RsidDetected event records the identification on the channel and raises a
// toast so the operator sees the mode + offset.
func TestApplyRsidDetectedSetsStateAndToast(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.applyEvent(&pb.Event{Kind: &pb.Event_RsidDetected{RsidDetected: &pb.RsidDetected{
		Channel: 2, Tag: "MFSK16", Mode: "mfsk16", FreqHz: 1500,
	}}})
	cl := m.live[2]
	if cl == nil || cl.lastRsid == "" {
		t.Fatal("RsidDetected must record the identification on the channel")
	}
	if m.toast == nil {
		t.Fatal("RsidDetected must raise a toast")
	}
}
