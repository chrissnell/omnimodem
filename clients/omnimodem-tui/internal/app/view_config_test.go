package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func devItem(id, label string, capt, play bool) *pb.DeviceInfo {
	return &pb.DeviceInfo{DeviceId: id, Label: label, HasCapture: capt, HasPlayback: play}
}

func TestConfigApplyGatedWithoutRxDevice(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	if v.canApply() {
		t.Fatal("apply must be gated until an RX device is chosen")
	}
}

// The reported bug: ConfigureAudio went out with an empty device_id. With a
// selected RX device, it must carry that id.
func TestConfigSelectedRxReachesConfigureAudio(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:" // as if selected from the list
	if !v.canApply() {
		t.Fatal("apply should be allowed once RX device is set")
	}
	v.apply()()        // ConfigureChannel → channelBoundMsg
	v.afterChannel()() // ConfigureAudio
	if len(f.AudioCalls) != 1 || f.AudioCalls[0].GetDeviceId() != "usb:1:2:" {
		t.Fatalf("ConfigureAudio must carry the selected device_id, got %+v", f.AudioCalls)
	}
}

func TestConfigBindChainsThroughPtt(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	v := newConfigView(m)
	v.rxID = "usb:1:2:"
	// channel → audio
	if _, cmd := v.Update(channelBoundMsg{}); cmd != nil {
		cmd()
	}
	if len(f.AudioCalls) != 1 {
		t.Fatalf("channelBound should trigger ConfigureAudio, got %d", len(f.AudioCalls))
	}
	// audio → ptt
	if _, cmd := v.Update(audioCfgMsg{resp: &pb.ConfigureAudioResponse{}}); cmd != nil {
		cmd()
	}
	if len(f.PttCalls) != 1 {
		t.Fatalf("audioCfg should trigger ConfigurePtt, got %d", len(f.PttCalls))
	}
}
