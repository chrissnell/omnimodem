package app

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
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

// parseModeLabel splits the daemon's canonical parametric mode string into a
// base label (matches the selector) and numeric params (seed the inputs).
func TestParseModeLabel(t *testing.T) {
	label, vals := parseModeLabel("olivia:tones=16,bw=500")
	if label != "olivia" || vals["tones"] != 16 || vals["bw"] != 500 {
		t.Fatalf("olivia parse = %q %v", label, vals)
	}
	// Bare label → no params.
	if l, v := parseModeLabel("ft8"); l != "ft8" || v != nil {
		t.Fatalf("bare parse = %q %v", l, v)
	}
	// Non-numeric values (rtty's reverse=false) are skipped, numeric ones kept.
	_, r := parseModeLabel("rtty:baud=45.45,shift=170,center=2210,reverse=false")
	if r["baud"] != 45.45 || r["shift"] != 170 || r["center"] != 2210 {
		t.Fatalf("rtty numeric params = %v", r)
	}
	if _, ok := r["reverse"]; ok {
		t.Fatalf("non-numeric reverse must be skipped, got %v", r)
	}
}

// Reopening Configure on a parametric channel must select the right mode (not
// silently fall back to the first) AND seed the param inputs with the saved
// values — the daemon reports the mode as "olivia:tones=16,bw=500".
func TestConfigPreloadsParametricMode(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	m.live[0] = &chanLive{name: "olivia-net", mode: "olivia:tones=16,bw=500", deviceID: "alsa:Mic"}
	v := newConfigView(m)
	if v.modeLabel() != "olivia" {
		t.Fatalf("parametric mode must select olivia, got %q", v.modeLabel())
	}
	if len(v.params) != 2 {
		t.Fatalf("olivia should expose 2 params, got %d", len(v.params))
	}
	got := map[string]string{}
	for _, p := range v.params {
		got[p.key] = p.input.Value()
	}
	if got["tones"] != "16" || got["bw"] != "500" {
		t.Fatalf("param inputs must preload saved values, got %v", got)
	}
}

// Editing a param and applying must send the operator's value in ModeParams,
// not the mode default.
func TestConfigEditedParamReachesConfigureChannel(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	m.live[0] = &chanLive{name: "olivia-net", mode: "olivia:tones=32,bw=1000"}
	v := newConfigView(m)
	v.rxID = "usb:rx" // satisfy canApply
	// Edit the first param (tones) 32 -> 8.
	v.params[0].input.SetValue("8")
	v.apply()()
	if len(f.ChannelCalls) != 1 {
		t.Fatalf("apply must call ConfigureChannel once, got %d", len(f.ChannelCalls))
	}
	o := f.ChannelCalls[0].GetModeParams().GetOlivia()
	if o == nil {
		t.Fatalf("olivia params must be sent, got %+v", f.ChannelCalls[0].GetModeParams())
	}
	if o.GetTones() != 8 || o.GetBandwidthHz() != 1000 {
		t.Fatalf("edited params must be sent: tones=%d bw=%d, want 8/1000", o.GetTones(), o.GetBandwidthHz())
	}
}

// Down-arrow navigation must step through the mode's param fields between Mode
// and the audio fields, and cycling the mode rebuilds its param set.
func TestConfigParamNavigationAndModeCycle(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	m.live[0] = &chanLive{name: "cw-net", mode: "cw:wpm=20,tone=700"}
	v := newConfigView(m)
	if v.modeLabel() != "cw" || len(v.params) != 2 {
		t.Fatalf("expected cw with 2 params, got %q/%d", v.modeLabel(), len(v.params))
	}
	// Name -> Call -> Grid -> Mode -> Param0 -> Param1 -> RX.
	for i := 0; i < 3; i++ {
		v.Update(tea.KeyMsg{Type: tea.KeyDown})
	}
	if v.focus != fMode {
		t.Fatalf("4th field should be Mode, got %d", v.focus)
	}
	v.Update(tea.KeyMsg{Type: tea.KeyDown})
	if v.focus != fParam || v.paramIdx != 0 {
		t.Fatalf("after Mode should be param 0, got focus=%d idx=%d", v.focus, v.paramIdx)
	}
	v.Update(tea.KeyMsg{Type: tea.KeyDown})
	if v.focus != fParam || v.paramIdx != 1 {
		t.Fatalf("should advance to param 1, got focus=%d idx=%d", v.focus, v.paramIdx)
	}
	v.Update(tea.KeyMsg{Type: tea.KeyDown})
	if v.focus != fRx {
		t.Fatalf("after the last param should be RX, got %d", v.focus)
	}
	// Cycle the mode at fMode: params rebuild for the new mode.
	v.focus = fMode
	v.Update(tea.KeyMsg{Type: tea.KeyRight})
	// ft4 (next after cw in the modes slice order is not guaranteed; just assert
	// the param set matches whatever mode is now selected).
	if len(v.params) != len(modes[v.modeIdx].params) {
		t.Fatalf("cycling mode must rebuild params for %q", v.modeLabel())
	}
}
