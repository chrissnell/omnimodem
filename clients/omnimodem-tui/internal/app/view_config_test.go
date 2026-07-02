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

// Up/Down arrows must traverse the form fields (not just Tab/Shift-Tab).
func TestConfigArrowsTraverseFields(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	if v.focus != fName {
		t.Fatalf("initial focus should be Name, got %d", v.focus)
	}
	v.Update(tea.KeyMsg{Type: tea.KeyDown})
	if v.focus != fCall {
		t.Fatalf("down should advance to Call, got %d", v.focus)
	}
	v.Update(tea.KeyMsg{Type: tea.KeyUp})
	if v.focus != fName {
		t.Fatalf("up should return to Name, got %d", v.focus)
	}
}

// The Grid field exists and edits the operator's station locator (uppercased).
func TestConfigGridFieldSetsStationGrid(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.Update(tea.KeyMsg{Type: tea.KeyDown}) // -> Call
	v.Update(tea.KeyMsg{Type: tea.KeyDown}) // -> Grid
	if v.focus != fGrid {
		t.Fatalf("expected Grid focus, got %d", v.focus)
	}
	v.grid.SetValue("")
	for _, r := range "em10" {
		v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
	}
	if m.myGrid != "EM10" {
		t.Fatalf("grid must sync to the model uppercased, got %q", m.myGrid)
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

// Reopening Configure must preload the channel's persisted config (surfaced via
// the snapshot) instead of showing blank defaults — the "config doesn't persist"
// report was really the form not reflecting what was saved.
func TestConfigPreloadsPersistedConfig(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	m.live[0] = &chanLive{
		name: "20m-rtty", mode: "rtty",
		deviceID: "alsa:Mic", txDeviceID: "alsa:Speakers",
		pttDeviceID: "serial:rig", pttMethod: pb.PttMethod_PTT_METHOD_SERIAL_RTS,
	}
	v := newConfigView(m)
	if v.name.Value() != "20m-rtty" {
		t.Fatalf("name not preloaded: %q", v.name.Value())
	}
	if v.modeLabel() != "rtty" {
		t.Fatalf("mode not preloaded: %q", v.modeLabel())
	}
	if v.rxID != "alsa:Mic" || v.txID != "alsa:Speakers" || v.pttID != "serial:rig" {
		t.Fatalf("devices not preloaded: rx=%q tx=%q ptt=%q", v.rxID, v.txID, v.pttID)
	}
	if v.method() != pb.PttMethod_PTT_METHOD_SERIAL_RTS {
		t.Fatalf("ptt method not preloaded: %v", v.method())
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

// Auto-apply: cycling the mode with an RX device chosen must persist the change
// (a full channel→audio→ptt rebind), so a mode switch takes effect immediately.
func TestConfigAutoAppliesOnModeChange(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.saved = v.sig() // baseline as if this rx were already saved
	v.focus = fMode

	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyRight}); cmd != nil {
		cmd() // ConfigureChannel
	}
	if len(f.ChannelCalls) != 1 {
		t.Fatalf("mode change must auto-apply ConfigureChannel, got %d calls", len(f.ChannelCalls))
	}
}

// Auto-apply must not fire on navigation alone (no field changed) — that would
// needlessly rebind the daemon's workers on every keypress.
func TestConfigNoApplyOnPlainNavigation(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.saved = v.sig()

	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyDown}); cmd != nil {
		cmd()
	}
	if len(f.ChannelCalls) != 0 {
		t.Fatalf("navigation must not auto-apply, got %d ConfigureChannel calls", len(f.ChannelCalls))
	}
}

// Auto-apply is gated on an RX device: with none chosen, changing a field saves
// nothing (audio can't bind without a capture device).
func TestConfigNoApplyWithoutRxDevice(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.focus = fMode
	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyRight}); cmd != nil {
		cmd()
	}
	if len(f.ChannelCalls) != 0 {
		t.Fatalf("no RX device: field change must not persist, got %d calls", len(f.ChannelCalls))
	}
}

// Choosing a PTT device auto-applies, and the choice reaches ConfigurePtt —
// the "PTT device isn't saved" report.
func TestConfigPttDeviceAutoAppliesAndPersists(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.saved = v.sig()

	// Open the PTT picker and choose the (only) device.
	v.focus = fPtt
	v.Update(tea.KeyMsg{Type: tea.KeyEnter}) // open picker
	_, cmd := v.Update(tea.KeyMsg{Type: tea.KeyEnter}) // choose → auto-apply
	if v.pttID != "usb:1:2:" {
		t.Fatalf("ptt device must be recorded, got %q", v.pttID)
	}
	// Drive the returned pipeline to completion: channel → audio → ptt.
	for cmd != nil {
		msg := cmd()
		_, cmd = v.Update(msg)
	}
	if len(f.PttCalls) != 1 || f.PttCalls[0].GetDeviceId() != "usb:1:2:" {
		t.Fatalf("ConfigurePtt must carry the chosen device, got %+v", f.PttCalls)
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
