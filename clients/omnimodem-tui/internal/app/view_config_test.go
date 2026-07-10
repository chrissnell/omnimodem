package app

import (
	"errors"
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

// drainCmd runs a command and feeds every resulting message back into the view,
// unwrapping tea.Batch, until the chain goes quiet. Lets a test drive the whole
// channel→audio→ptt auto-apply pipeline (and its coalescing re-check) to rest.
func drainCmd(v *configView, cmd tea.Cmd) {
	if cmd == nil {
		return
	}
	switch msg := cmd().(type) {
	case tea.BatchMsg:
		for _, c := range msg {
			drainCmd(v, c)
		}
	default:
		_, next := v.Update(msg)
		drainCmd(v, next)
	}
}

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

// Focus/Tab order must follow the on-screen top-to-bottom layout: the AUDIO
// block ends with PTT Method -> TX Delay -> TX Tail, then the RSID section.
// Guards against the enum drifting out of render order (a zig-zagging cursor).
func TestConfigFocusOrderMatchesLayout(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.focus = fMethod
	for _, want := range []cfgFocus{fTxDelay, fTxTail, fRsidTx, fRsidRx} {
		v.Update(tea.KeyMsg{Type: tea.KeyDown})
		if v.focus != want {
			t.Fatalf("focus order broke: got %d, want %d", v.focus, want)
		}
	}
	if fLast != fRsidRx {
		t.Fatalf("fLast should be the last rendered field (fRsidRx), got %d", fLast)
	}
}

// The Grid field exists and edits the operator's station locator (uppercased).
func TestConfigGridFieldSetsStationGrid(t *testing.T) {
	// Editing identity can trigger persistIdentity; keep the write off the real
	// user config dir so the test stays hermetic even if it grows a blur/esc.
	t.Setenv("OMNIMODEM_TUI_CONFIG", filepath.Join(t.TempDir(), "config.json"))
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
	if !v.pickerOpen() {
		t.Fatal("enter on a device field must open the picker modal")
	}
	if out := v.Render(80, 20); !strings.Contains(out, "TX device") || !strings.Contains(out, "Speaker") {
		t.Fatalf("open picker must render the device list:\n%s", out)
	}

	// Enter again chooses the highlighted device and closes the modal.
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if v.pickerOpen() {
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
	v.picker = pickDevice
	if _, _ = v.Update(tea.KeyMsg{Type: tea.KeyEsc}); v.pickerOpen() {
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
		pttTxDelayMs: 275, pttTxTailMs: 35,
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
	if v.txDelay.Value() != "275" || v.txTail.Value() != "35" {
		t.Fatalf("ptt timing not preloaded: delay=%q tail=%q", v.txDelay.Value(), v.txTail.Value())
	}
}

// The per-channel TX delay / TX tail entered in the form must reach the
// ConfigurePtt RPC so the daemon persists and applies them.
func TestConfigPttTimingReachesConfigurePtt(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.txDelay.SetValue("420")
	v.txTail.SetValue("15")
	v.persistAll()()
	if len(f.PttCalls) != 1 {
		t.Fatalf("want one ConfigurePtt, got %d", len(f.PttCalls))
	}
	if got := f.PttCalls[0]; got.GetTxDelayMs() != 420 || got.GetTxTailMs() != 15 {
		t.Fatalf("ptt timing must reach the RPC: delay=%d tail=%d", got.GetTxDelayMs(), got.GetTxTailMs())
	}
}

// A fresh channel (no saved state) opens with the sensible default timing so an
// operator who never touches the fields still gets a working lead-in.
func TestConfigDefaultPttTiming(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	v := newConfigView(m)
	if v.txDelay.Value() != "300" || v.txTail.Value() != "50" {
		t.Fatalf("default timing wrong: delay=%q tail=%q", v.txDelay.Value(), v.txTail.Value())
	}
}

// A fresh channel's default name must be all-caps and not collide with an
// existing channel: with a legacy lowercase "vfo-a" already present, the next
// default is "VFO-B".
func TestConfigDefaultNameAvoidsCollision(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{name: "vfo-a"}
	m.sel = 1 // adding a new channel
	v := newConfigView(m)
	if v.name.Value() != "VFO-B" {
		t.Fatalf("default name must skip the taken A slot and be all-caps; got %q", v.name.Value())
	}
}

// With no channels yet, the first default is VFO-A (all-caps).
func TestConfigDefaultNameFirstIsVfoA(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	v := newConfigView(m)
	if v.name.Value() != "VFO-A" {
		t.Fatalf("first channel default must be VFO-A; got %q", v.name.Value())
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
// selected RX device, the one-shot save must carry that id.
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
	v.persistAll()() // ConfigureChannel → ConfigureAudio → ConfigurePtt, one shot
	if len(f.AudioCalls) != 1 || f.AudioCalls[0].GetDeviceId() != "usb:1:2:" {
		t.Fatalf("ConfigureAudio must carry the selected device_id, got %+v", f.AudioCalls)
	}
}

// persistAll runs the full save in one command: one ConfigureChannel, one
// ConfigureAudio, one ConfigurePtt — so leaving mid-save can't drop a stage.
func TestConfigPersistAllIssuesAllThreeRPCs(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:rx", "Mic", true, false), devItem("usb:tx", "Spk", false, true)})
	v.rxID, v.txID, v.pttID = "usb:rx", "usb:tx", "usb:tx"
	if _, ok := v.persistAll()().(saveDoneMsg); !ok {
		t.Fatal("persistAll must complete with a saveDoneMsg")
	}
	if len(f.ChannelCalls) != 1 || len(f.AudioCalls) != 1 || len(f.PttCalls) != 1 {
		t.Fatalf("want one of each RPC, got ch=%d audio=%d ptt=%d",
			len(f.ChannelCalls), len(f.AudioCalls), len(f.PttCalls))
	}
	if f.AudioCalls[0].GetTxDeviceId() != "usb:tx" || f.PttCalls[0].GetDeviceId() != "usb:tx" {
		t.Fatalf("TX/PTT device choices must reach their RPCs: audio=%+v ptt=%+v", f.AudioCalls[0], f.PttCalls[0])
	}
}

// A channel that binds RX-only (tx_rate == 0) must warn the operator that
// transmit will be silent, rather than failing quietly.
func TestConfigWarnsWhenBoundRxOnly(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.Update(saveDoneMsg{warnRxOnly: true})
	if m.toast == nil || !strings.Contains(m.toast.Line(), "RX-only") {
		t.Fatalf("RX-only bind must surface a transmit-silent warning, toast=%v", m.toast)
	}

	// A bound TX device must not warn.
	m.toast = nil
	v.Update(saveDoneMsg{warnRxOnly: false})
	if m.toast != nil {
		t.Fatalf("a bound TX device must not warn, got %q", m.toast.Line())
	}
}

// When TX mirrors RX (one device used for both — e.g. a single BlackHole on
// macOS), the daemon reports TX empty. The form must still show the effective
// device with a "(same as RX)" note, not a bare "(same as RX)" that reads as if
// the TX choice was lost.
func TestConfigTxShowsEffectiveDeviceWhenSameAsRx(t *testing.T) {
	// Distinct TX device: shown outright (wide budget so nothing clips).
	if got := txDeviceValue("virtual:BlackHole 16ch", "virtual:BlackHole 2ch", 40); !strings.Contains(got, "BlackHole 16ch") {
		t.Fatalf("distinct TX must show its own device, got %q", got)
	}
	// TX mirrors RX (empty tx): show the RX device AND the note.
	got := txDeviceValue("", "virtual:BlackHole 2ch", 40)
	if !strings.Contains(got, "BlackHole 2ch") {
		t.Fatalf("TX mirroring RX must show the RX device, got %q", got)
	}
	if !strings.Contains(got, "same as RX") {
		t.Fatalf("TX mirroring RX must still note (same as RX), got %q", got)
	}
	// No RX yet: bare note is fine.
	if got := txDeviceValue("", "", 40); strings.Contains(got, "✓") {
		t.Fatalf("with no RX, TX must not claim a device, got %q", got)
	}
}

// Full reopen render: a channel configured with one device for RX (TX reported
// empty by the daemon) must render that device in the TX row, not blank.
func TestConfigReopenRendersTxDeviceForSingleCard(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	m.live[0] = &chanLive{
		name: "vfo-a", mode: "psk31",
		deviceID: "virtual:BlackHole 2ch", txDeviceID: "", // TX mirrors RX
	}
	v := newConfigView(m)
	out := v.Render(100, 40)
	// The TX row must carry the device, so the operator sees TX is set.
	if !strings.Contains(out, "BlackHole 2ch") {
		t.Fatalf("reopen must show the RX device in the TX row:\n%s", out)
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
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})           // open picker
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

// A failed auto-apply must not advance the "saved" baseline: the operator sees
// the error toast, and the next change (even to the same value) retries the save
// rather than the form silently believing it persisted.
func TestConfigFailedApplyRetriesOnNextChange(t *testing.T) {
	f := &client.Fake{Err: errors.New("daemon down")}
	m := New(f, "x")
	m.connected = true
	m.push(newChannelsView(m))
	m.sel = 0
	m.live[0] = &chanLive{}
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.saved = v.sig()
	v.focus = fMode
	m.push(v)

	// Change mode → starts a pipeline; its first RPC fails.
	_, cmd := m.Update(tea.KeyMsg{Type: tea.KeyRight})
	if cmd == nil {
		t.Fatal("mode change should start a save")
	}
	m.Update(cmd()) // rpcErrMsg → routed to view → clears the in-flight guard
	if v.applying {
		t.Fatal("a failed apply must clear the in-flight guard")
	}
	if got := len(f.ChannelCalls); got != 1 {
		t.Fatalf("want 1 ConfigureChannel attempt, got %d", got)
	}

	// Recover and change again: saved was never advanced, so this must retry.
	f.Err = nil
	_, cmd = m.Update(tea.KeyMsg{Type: tea.KeyRight})
	if cmd == nil {
		t.Fatal("a change after failure must retry the save")
	}
	cmd()
	if got := len(f.ChannelCalls); got != 2 {
		t.Fatalf("retry must issue another ConfigureChannel, got %d", got)
	}
}

// Auto-apply serializes: a change made while a pipeline is in flight must not
// launch a second concurrent pipeline (that could race two ConfigureChannel
// RPCs out of order). The in-flight pipeline finishes, then a single coalesced
// follow-up persists the latest state.
func TestConfigSerializesConcurrentApplies(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.saved = v.sig()
	v.focus = fMode

	// First change starts a pipeline.
	_, cmd1 := v.Update(tea.KeyMsg{Type: tea.KeyRight})
	if cmd1 == nil || !v.applying {
		t.Fatal("first change should start an in-flight pipeline")
	}

	// Second change while in flight must NOT start another pipeline...
	if _, cmd2 := v.Update(tea.KeyMsg{Type: tea.KeyRight}); cmd2 != nil {
		t.Fatal("no second pipeline may start while one is in flight")
	}
	latestMode := v.modeLabel()

	// ...only the first pipeline's ConfigureChannel has gone out so far.
	drainCmd(v, cmd1)
	if len(f.ChannelCalls) != 2 {
		t.Fatalf("want exactly 2 serialized ConfigureChannel calls (in-flight + coalesced), got %d", len(f.ChannelCalls))
	}
	// The coalesced follow-up persisted the latest mode, and the baseline caught up.
	if last := f.ChannelCalls[len(f.ChannelCalls)-1].GetMode(); last != latestMode {
		t.Fatalf("coalesced save must carry the latest mode %q, got %q", latestMode, last)
	}
	if v.applying || v.saved != v.sig() {
		t.Fatalf("after draining, no pipeline should be in flight and saved should match current state")
	}
}

// drive runs a command through the MODEL (so routeToView + pop behave as at
// runtime), following every resulting message and its follow-ups to rest.
func drive(m *Model, cmd tea.Cmd) {
	for cmd != nil {
		msg := cmd()
		if batch, ok := msg.(tea.BatchMsg); ok {
			for _, c := range batch {
				drive(m, c)
			}
			return
		}
		_, cmd = m.Update(msg)
	}
}

// The reported bug: choose RX, TX, and PTT in quick succession then leave with
// <esc>, and only RX persisted. The old save was split across view-routed
// messages, so popping the view mid-save dropped ConfigureAudio/ConfigurePtt;
// devices picked while a save was in flight were also never flushed. esc must
// now hold the view open until the whole save drains, persisting all three.
func TestConfigEscPersistsAllChosenDevices(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.connected = true
	m.push(newChannelsView(m))
	m.sel = 0
	m.live[0] = &chanLive{}
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{
		devItem("usb:rx", "Mic", true, false),
		devItem("usb:tx", "Spk", false, true),
		devItem("usb:ptt", "Rig", true, true),
	})
	m.push(v)

	// Pick RX — starts a save. Hold its command undrained (still "in flight").
	v.rxID = "usb:rx"
	rxSave := v.maybePersist()
	if rxSave == nil || !v.applying {
		t.Fatal("choosing RX should start a save")
	}
	// Pick TX and PTT while that save is in flight: no second pipeline may start;
	// the choices stay pending.
	v.txID = "usb:tx"
	if v.maybePersist() != nil {
		t.Fatal("TX pick while a save is in flight must not start a second pipeline")
	}
	v.pttID = "usb:ptt"
	if v.maybePersist() != nil {
		t.Fatal("PTT pick while a save is in flight must not start a second pipeline")
	}

	// esc now: the save is still in flight with TX/PTT pending. The view must NOT
	// pop yet and must not launch a racing save.
	if _, escCmd := m.Update(tea.KeyMsg{Type: tea.KeyEsc}); escCmd != nil {
		t.Fatal("esc must not act while a save is in flight; the completion handler drives it")
	}
	if _, ok := m.top().(*configView); !ok {
		t.Fatalf("view must stay open until the in-flight save drains; top=%T", m.top())
	}

	// Let the in-flight RX save complete; its coalesced follow-up persists the
	// pending TX and PTT, then the view pops.
	drive(m, rxSave)

	if _, ok := m.top().(*channelsView); !ok {
		t.Fatalf("view must pop once fully saved; top=%T", m.top())
	}
	if n := len(f.AudioCalls); n == 0 || f.AudioCalls[n-1].GetTxDeviceId() != "usb:tx" {
		t.Fatalf("TX device must be persisted; audio calls=%+v", f.AudioCalls)
	}
	if n := len(f.PttCalls); n == 0 || f.PttCalls[n-1].GetDeviceId() != "usb:ptt" {
		t.Fatalf("PTT device must be persisted; ptt calls=%+v", f.PttCalls)
	}
	// The close path must refresh live state (GetState) so that reopening
	// Configure preloads the devices just saved — m.live is only repopulated by
	// a snapshot, not by the deviceless ChannelConfigured event.
	if f.StateCalls == 0 {
		t.Fatal("esc-close must refresh live state so a reopen reflects the save")
	}
}

// The mode selector cascades: Family is chosen first, then Mode within it.
// Cycling the family must land on that family's first submode and rebuild the
// settings; cycling the mode must stay inside the current family.
func TestConfigFamilyCascadesToMode(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	v := newConfigView(m)

	// Start on a known multi-member family so the cascade is observable.
	v.familyIdx = familyIdxOfMode(modeIdxByLabel("dominoex4"))
	v.modeIdx = modeIdxByLabel("dominoex4")
	famName := v.familyName()

	// Cycling Mode moves within the family, never out of it.
	v.focus = fMode
	v.cycle(+1)
	if v.familyName() != famName {
		t.Fatalf("cycling mode left the family: %q -> %q", famName, v.familyName())
	}
	if familyName(v.modeLabel()) != famName {
		t.Fatalf("mode %q is not in family %q", v.modeLabel(), famName)
	}

	// Cycling Family lands on the new family's first submode.
	v.focus = fFamily
	v.cycle(+1)
	fam := families[v.familyIdx]
	if v.modeIdx != fam.modes[0] {
		t.Fatalf("changing family must select its first mode; got %q want %q",
			v.modeLabel(), modes[fam.modes[0]].label)
	}
}

// Preload must point the Family selector at the family owning the saved mode, so
// reopening Configure shows the right family/mode pair (not family 0).
func TestConfigPreloadSelectsMatchingFamily(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	m.live[0] = &chanLive{name: "eme", mode: "jt65", deviceID: "usb:rx"}
	v := newConfigView(m)
	if v.modeLabel() != "jt65" {
		t.Fatalf("mode not preloaded: %q", v.modeLabel())
	}
	if v.familyName() != "JT65" {
		t.Fatalf("family must match preloaded mode; got %q", v.familyName())
	}
}

// The Render output must present both the Family and Mode rows of the cascade.
func TestConfigRendersFamilyAndModeRows(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	m.live[0] = &chanLive{name: "vfo-a", mode: "dominoex8", deviceID: "usb:rx"}
	v := newConfigView(m)
	out := v.Render(100, 40)
	if !strings.Contains(out, "Family") {
		t.Fatalf("Render must show a Family row:\n%s", out)
	}
	if !strings.Contains(out, "DominoEX") {
		t.Fatalf("Render must show the selected family name:\n%s", out)
	}
	if !strings.Contains(out, "DominoEX 8") {
		t.Fatalf("Render must show the selected mode:\n%s", out)
	}
}

// Cycling the Family with an RX device chosen must auto-apply, persisting the
// new (first-of-family) mode through ConfigureChannel.
func TestConfigAutoAppliesOnFamilyChange(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.saved = v.sig()
	v.focus = fFamily

	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyRight}); cmd != nil {
		cmd() // ConfigureChannel
	}
	if len(f.ChannelCalls) != 1 {
		t.Fatalf("family change must auto-apply ConfigureChannel, got %d", len(f.ChannelCalls))
	}
}

// modeStringFor: the mode-string tail is only appended for the modes with no
// typed ModeParams message (FST4/JS8/MSK144); everything else stays a bare label.
func TestModeStringForTailModes(t *testing.T) {
	cases := map[string]struct {
		vals map[string]string
		want string
	}{
		"fst4":   {map[string]string{"tr": "300"}, "fst4:tr=300"},
		"js8":    {map[string]string{"sub": "fast"}, "js8:sub=fast"},
		"msk144": {map[string]string{"freq": "1200"}, "msk144:freq=1200"},
	}
	for label, c := range cases {
		if got := modeStringFor(label, c.vals); got != c.want {
			t.Fatalf("modeStringFor(%q) = %q, want %q", label, got, c.want)
		}
	}
	// A bare label with no tail params (default via empty vals) still round-trips.
	if got := modeStringFor("fst4", nil); got != "fst4:tr=15" {
		t.Fatalf("fst4 default tail = %q, want fst4:tr=15", got)
	}
	// Modes that use typed ModeParams keep the bare label.
	for _, label := range []string{"psk31", "ft8", "cw", "fsq"} {
		if got := modeStringFor(label, map[string]string{"center": "1500"}); got != label {
			t.Fatalf("modeStringFor(%q) must stay bare, got %q", label, got)
		}
	}
}

// modeStringParam extracts a numeric tail key, falling back to the default.
func TestModeStringParam(t *testing.T) {
	if got := modeStringParam("fst4:tr=300", "tr", 15); got != 300 {
		t.Fatalf("tr parse = %v, want 300", got)
	}
	if got := modeStringParam("fst4", "tr", 15); got != 15 {
		t.Fatalf("missing tail must return default, got %v", got)
	}
	if got := modeStringParam("msk144:freq=1200,x=1", "freq", 1500); got != 1200 {
		t.Fatalf("freq parse = %v, want 1200", got)
	}
}

// Each daemon-tunable mode must expose an editable field for its extra param:
// FSQ 'directed', JS8 speed, FST4 T/R, MSK144 center.
func TestModeFieldsCoverDaemonParams(t *testing.T) {
	hasKey := func(label, key string) bool {
		for _, f := range modeFields(label) {
			if f.Key == key {
				return true
			}
		}
		return false
	}
	for _, tc := range []struct{ label, key string }{
		{"fsq", "directed"}, {"fsq", "center"},
		{"js8", "sub"}, {"fst4", "tr"}, {"msk144", "freq"},
	} {
		if !hasKey(tc.label, tc.key) {
			t.Fatalf("mode %q must expose a %q settings field", tc.label, tc.key)
		}
	}
}

// The FSQ directed toggle must reach ConfigureChannel as a typed FsqParams flag.
func TestConfigFsqDirectedReachesRPC(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	// Seed the session cache so the settings form opens with directed on.
	m.modeParams[0] = savedModeParams{label: "fsq", vals: map[string]float64{"center": 1500, "directed": 1}}
	m.live[0] = &chanLive{name: "fsq-net", mode: "fsq", deviceID: "usb:rx"}
	v := newConfigView(m)
	if v.modeLabel() != "fsq" {
		t.Fatalf("expected fsq preloaded, got %q", v.modeLabel())
	}
	v.persistAll()()
	if len(f.ChannelCalls) != 1 {
		t.Fatalf("want one ConfigureChannel, got %d", len(f.ChannelCalls))
	}
	fp := f.ChannelCalls[0].GetModeParams().GetFsq()
	if fp == nil || !fp.GetDirected() {
		t.Fatalf("FSQ directed flag must reach the RPC, got %+v", f.ChannelCalls[0].GetModeParams())
	}
}

// MSK144's audio center must reach ConfigureChannel as a mode-string tail (it has
// no typed ModeParams message).
func TestConfigMsk144CenterReachesModeString(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	m.modeParams[0] = savedModeParams{label: "msk144", vals: map[string]float64{"freq": 1200}}
	m.live[0] = &chanLive{name: "ms", mode: "msk144", deviceID: "usb:rx"}
	v := newConfigView(m)
	v.persistAll()()
	if len(f.ChannelCalls) != 1 || f.ChannelCalls[0].GetMode() != "msk144:freq=1200" {
		t.Fatalf("MSK144 center must ride the mode string, got mode=%q", f.ChannelCalls[0].GetMode())
	}
}

// FST4's T/R period must reach ConfigureChannel via the mode string AND drive the
// operate view's slot clock, so the TX watchdog matches the chosen sequence.
func TestFst4TrReachesModeStringAndSlotClock(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	m.modeParams[0] = savedModeParams{label: "fst4", vals: map[string]float64{"tr": 300}}
	m.live[0] = &chanLive{name: "lf", mode: "fst4", deviceID: "usb:rx"}
	v := newConfigView(m)
	v.persistAll()()
	if f.ChannelCalls[0].GetMode() != "fst4:tr=300" {
		t.Fatalf("FST4 T/R must ride the mode string, got %q", f.ChannelCalls[0].GetMode())
	}
	// The operate view, opened on the persisted mode string, must adopt the period.
	m.live[0].mode = "fst4:tr=300"
	ov := newOperateView(m)
	if ov.slotSecs != 300 {
		t.Fatalf("operate slot clock must reflect the FST4 T/R period, got %v", ov.slotSecs)
	}
}

// The device picker filters by the focused field's capability (RX shows only
// capture devices), navigates with the cursor, narrows with '/', and enter
// chooses the highlighted (possibly filtered) device.
func TestConfigDevicePickerNavFilterCapability(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{
		devItem("usb:mic", "USB Mic", true, false),
		devItem("bh:2", "BlackHole 2ch", true, true),
		devItem("spk:only", "Speakers", false, true), // playback-only
		devItem("hw:cap", "Line In", true, false),
	})
	v.focus = fRx

	// RX picker must exclude the playback-only device.
	for _, d := range v.capabilityDevices() {
		if d.id == "spk:only" {
			t.Fatalf("RX picker must exclude playback-only devices")
		}
	}
	if len(v.capabilityDevices()) != 3 {
		t.Fatalf("RX picker should show 3 capture devices, got %d", len(v.capabilityDevices()))
	}

	// Open and navigate.
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if !v.pickerOpen() || v.picker != pickDevice {
		t.Fatal("enter must open the device picker")
	}
	v.Update(tea.KeyMsg{Type: tea.KeyDown})
	if v.pickIdx != 1 {
		t.Fatalf("down must advance the cursor, got %d", v.pickIdx)
	}

	// Filter to "line" → only Line In remains (id column carries the device id).
	v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("/")})
	if !v.filtering {
		t.Fatal("'/' must start filtering")
	}
	for _, r := range "line" {
		v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
	}
	if got := v.pickerRows(); len(got) != 1 || got[0].cells[1] != "hw:cap" {
		t.Fatalf("filter must narrow to Line In, got %+v", got)
	}

	// Enter applies the filter (leaves typing), enter again chooses.
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if v.filtering {
		t.Fatal("enter must leave filter typing")
	}
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if v.rxID != "hw:cap" {
		t.Fatalf("enter must choose the highlighted filtered device, got %q", v.rxID)
	}
	if v.pickerOpen() {
		t.Fatal("choosing must close the picker")
	}
}

// The redesigned form composes titled cards; each section's header is present.
func TestConfigRendersSectionCards(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	v := newConfigView(m)
	out := v.Render(100, 30)
	for _, title := range []string{"STATION", "MODE", "AUDIO", "RSID"} {
		if !strings.Contains(out, title) {
			t.Fatalf("config screen must show the %q card:\n%s", title, out)
		}
	}
	if !strings.Contains(out, "╭") {
		t.Fatalf("config screen must use rounded card borders:\n%s", out)
	}
}

// Enter on the Family field opens the family picker (homed on the current
// family); choosing one switches family, homes the mode on the family's first
// submode, and auto-applies.
func TestConfigFamilyPickerModal(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.saved = v.sig()
	v.focus = fFamily

	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if v.picker != pickFamily {
		t.Fatal("enter on Family must open the family picker")
	}
	if v.pickIdx != v.familyIdx {
		t.Fatalf("family picker must home on the current family, idx=%d family=%d", v.pickIdx, v.familyIdx)
	}

	// Filter to Throb and choose it.
	v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("/")})
	for _, r := range "throb" {
		v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
	}
	v.Update(tea.KeyMsg{Type: tea.KeyEnter}) // leave filter typing
	if rows := v.pickerRows(); len(rows) != 1 || rows[0].cells[0] != "Throb" {
		t.Fatalf("filter should narrow to Throb, got %+v", rows)
	}
	_, cmd := v.Update(tea.KeyMsg{Type: tea.KeyEnter}) // choose
	if v.pickerOpen() {
		t.Fatal("choosing must close the picker")
	}
	if v.familyName() != "Throb" {
		t.Fatalf("family must switch to Throb, got %q", v.familyName())
	}
	if v.modeIdx != families[v.familyIdx].modes[0] {
		t.Fatal("mode must home on the first submode of the chosen family")
	}
	if cmd != nil {
		cmd()
	}
	if len(f.ChannelCalls) == 0 {
		t.Fatal("choosing a family must auto-apply the mode change")
	}
}

// Enter on the Mode field opens the mode picker scoped to the current family;
// choosing a submode selects it.
func TestConfigModePickerModal(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	v := newConfigView(m)
	v.modeIdx = modeIdxByLabel("scottie1")
	v.familyIdx = familyIdxOfMode(v.modeIdx)
	v.focus = fMode

	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if v.picker != pickMode {
		t.Fatal("enter on Mode must open the mode picker")
	}
	// Every row must belong to the current (SSTV) family.
	if v.familyName() != "SSTV" {
		t.Fatalf("mode picker should be scoped to SSTV, got %q", v.familyName())
	}
	v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("/")})
	for _, r := range "martin2" {
		v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
	}
	v.Update(tea.KeyMsg{Type: tea.KeyEnter}) // leave filter
	v.Update(tea.KeyMsg{Type: tea.KeyEnter}) // choose
	if v.modeLabel() != "martin2" {
		t.Fatalf("mode must switch to martin2, got %q", v.modeLabel())
	}
	if v.pickerOpen() {
		t.Fatal("choosing must close the mode picker")
	}
}

// Regression: the Name/Call/Grid inputs must fit the STATION card so the value
// never wraps (the reported "vfo-a" → "vfo-"/"a" break). Checked across widths.
func TestConfigStationFieldsDoNotWrap(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	m.live[0] = &chanLive{name: "vfo-a"}
	m.myCall, m.myGrid = "NW5W", "DN40CL"
	v := newConfigView(m)
	for _, lw := range []int{32, 39, 48} {
		card := ui.Card("STATION", v.stationBody(lw), true, lw)
		lines := strings.Split(card, "\n")
		// top border + title + rule + 3 field rows + bottom border = 7 lines. A
		// wrap would add an extra line.
		if len(lines) != 7 {
			t.Fatalf("width %d: STATION card must be 7 lines (no wrap), got %d:\n%s", lw, len(lines), card)
		}
		for _, ln := range lines {
			if lipgloss.Width(ln) != lw {
				t.Fatalf("width %d: line width %d != %d (overflow):\n%s", lw, lipgloss.Width(ln), lw, card)
			}
		}
		if !strings.Contains(card, "vfo-a") {
			t.Fatalf("width %d: name value must render intact:\n%s", lw, card)
		}
	}
}

// Regression: the Settings row leads with the edit button so it lines up under
// the Family/Mode values, rather than being pushed far right by the count.
func TestConfigSettingsEditLeadsValue(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.sel = 0
	m.live[0] = &chanLive{name: "vfo-a", mode: "psk125"}
	v := newConfigView(m)
	if s := v.settingsSummary(30); !strings.HasPrefix(s, "✎ edit") {
		t.Fatalf("settings value must lead with the edit button, got %q", s)
	}
	// The MODE card must not wrap either, at any width.
	for _, lw := range []int{32, 39, 48} {
		card := ui.Card("MODE", v.modeBody(lw), true, lw)
		if n := len(strings.Split(card, "\n")); n != 7 {
			t.Fatalf("width %d: MODE card must be 7 lines (no wrap), got %d:\n%s", lw, n, card)
		}
	}
}
