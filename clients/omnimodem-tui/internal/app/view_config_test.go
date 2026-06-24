package app

import (
	"strings"
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

func devItem(id, label string, capt, play bool) *pb.DeviceInfo {
	return &pb.DeviceInfo{DeviceId: id, Label: label, HasCapture: capt, HasPlayback: play}
}

// Regression: the name field must accept every character, including 'a' and
// space, while focused (form-action keys must not steal them).
func TestConfigNameAcceptsLettersAndSpace(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.name.SetValue("")
	for _, r := range "data a" {
		v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
	}
	if v.name.Value() != "data a" {
		t.Fatalf("name field dropped characters: %q", v.name.Value())
	}
}

// The device picker is a modal: hidden until a device field is opened with
// enter, shown while picking, and gone again once a device is chosen.
func TestConfigDevicePickerModalOpensAndCloses(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{
		devItem("usb:rx", "Mic", true, false),
		devItem("usb:tx", "Speaker", false, true),
	})
	v.focus = fTx

	// Closed: the form shows but the device list does not.
	if out := v.Render(80, 20); strings.Contains(out, "Speaker") {
		t.Fatalf("picker must be hidden until opened:\n%s", out)
	}

	// Enter opens the picker over the focused (TX) field.
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if !v.picking {
		t.Fatal("enter on a device field must open the picker modal")
	}
	if out := v.Render(80, 20); !strings.Contains(out, "TX device") || !strings.Contains(out, "Speaker") {
		t.Fatalf("open picker must render the device list:\n%s", out)
	}

	// Enter again chooses the highlighted device and closes the modal.
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if v.picking {
		t.Fatal("choosing a device must close the picker modal")
	}
	if v.txID != "usb:tx" {
		t.Fatalf("chosen device must be recorded, got %q", v.txID)
	}
	if out := v.Render(80, 20); strings.Contains(out, "Speaker") {
		t.Fatalf("picker must be gone after choosing:\n%s", out)
	}
}

// Esc inside the open picker cancels the pick (closes the modal) without
// leaving the Configure screen.
func TestConfigDevicePickerEscCancels(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:tx", "Speaker", false, true)})
	v.focus = fTx
	v.picking = true
	if _, _ = v.Update(tea.KeyMsg{Type: tea.KeyEsc}); v.picking {
		t.Fatal("esc must close the picker modal")
	}
	if v.txID != "" {
		t.Fatal("esc must not record a device")
	}
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

// A channel that binds RX-only (tx_rate == 0) must warn the operator that
// transmit will be silent, rather than failing quietly.
func TestConfigWarnsWhenBoundRxOnly(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.Update(audioCfgMsg{resp: &pb.ConfigureAudioResponse{ActualSampleRate: 48000, ActualTxSampleRate: 0}})
	if m.toast == nil || !strings.Contains(m.toast.Line(), "RX-only") {
		t.Fatalf("RX-only bind must surface a transmit-silent warning, toast=%v", m.toast)
	}

	// A real TX rate must not warn.
	m.toast = nil
	v.Update(audioCfgMsg{resp: &pb.ConfigureAudioResponse{ActualSampleRate: 48000, ActualTxSampleRate: 48000}})
	if m.toast != nil {
		t.Fatalf("a bound TX device must not warn, got %q", m.toast.Line())
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
