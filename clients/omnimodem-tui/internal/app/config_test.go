package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func TestModeParamsForCW(t *testing.T) {
	mp := modeParamsFor("cw", map[string]float64{"wpm": 25, "tone": 600})
	cw := mp.GetCw()
	if cw == nil || cw.GetWpm() != 25 || cw.GetToneHz() != 600 {
		t.Fatalf("cw params = %+v, want wpm 25 tone 600", cw)
	}
	if modeParamsFor("ft8", nil) != nil {
		t.Fatalf("ft8 has no params → nil ModeParams")
	}
}

func TestConfigApplyBuildsChannelWithParams(t *testing.T) {
	f := &client.Fake{Devices: []*pb.DeviceInfo{{DeviceId: "usb:1:2:", HasCapture: true, HasPlayback: true}}}
	m := New(f, "x")
	m.sel = 0
	m.enterConfig()
	m.cfg.devices = f.Devices
	m.cfg.rxDev = "usb:1:2:"
	m.cfg.modeLabel = "cw"
	m.cfg.params = map[string]float64{"wpm": 25, "tone": 600}
	m.cfg.name = "vfo-a"

	m.applyConfig()() // run the channel step
	if len(f.ChannelCalls) != 1 {
		t.Fatalf("want 1 ConfigureChannel, got %d", len(f.ChannelCalls))
	}
	cc := f.ChannelCalls[0]
	if cc.GetMode() != "cw" || cc.GetModeParams().GetCw().GetWpm() != 25 {
		t.Fatalf("channel req wrong: %+v", cc)
	}
}

func TestConfigChainsAudioThenPtt(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	m.enterConfig()
	m.cfg.rxDev = "usb:1:2:"

	// channel ok → audio
	if _, cmd := m.updateConfig(rpcOKMsg{what: "channel"}); cmd != nil {
		cmd()
	}
	if len(f.AudioCalls) != 1 {
		t.Fatalf("channel-ok should trigger ConfigureAudio, got %d", len(f.AudioCalls))
	}
	// audio resp → ptt
	if _, cmd := m.updateConfig(audioCfgMsg{resp: &pb.ConfigureAudioResponse{}}); cmd != nil {
		cmd()
	}
	if len(f.PttCalls) != 1 {
		t.Fatalf("audio resp should trigger ConfigurePtt, got %d", len(f.PttCalls))
	}
}

func TestSetGainCmd(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 1
	m.setGainCmd(2.0, 1.0)()
	if len(f.GainCalls) != 1 || f.GainCalls[0].GetRxGain() != 2.0 || f.GainCalls[0].GetChannel() != 1 {
		t.Fatalf("gain call wrong: %+v", f.GainCalls)
	}
}
