package app

import (
	"errors"
	"path/filepath"
	"strings"
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
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
	// Distinct TX device: shown outright.
	if got := txDeviceLabel("virtual:BlackHole 16ch", "virtual:BlackHole 2ch"); !strings.Contains(got, "BlackHole 16ch") {
		t.Fatalf("distinct TX must show its own device, got %q", got)
	}
	// TX mirrors RX (empty tx): show the RX device AND the note.
	got := txDeviceLabel("", "virtual:BlackHole 2ch")
	if !strings.Contains(got, "BlackHole 2ch") {
		t.Fatalf("TX mirroring RX must show the RX device, got %q", got)
	}
	if !strings.Contains(got, "same as RX") {
		t.Fatalf("TX mirroring RX must still note (same as RX), got %q", got)
	}
	// No RX yet: bare note is fine.
	if got := txDeviceLabel("", ""); strings.Contains(got, "✓") {
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
