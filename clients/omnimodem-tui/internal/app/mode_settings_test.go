package app

import (
	"strings"
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	tea "github.com/charmbracelet/bubbletea"
)

func fieldKeys(fields []ui.Field) map[string]ui.Field {
	m := make(map[string]ui.Field, len(fields))
	for _, f := range fields {
		m[f.Key] = f
	}
	return m
}

// Each mode family surfaces the settings its typed params carry: CW its speed
// and tone, RTTY its baud/shift plus advanced center/reverse, a PSK submode just
// its audio center, and the windowed modes nothing.
func TestModeFieldsPerFamily(t *testing.T) {
	cw := fieldKeys(modeFields("cw"))
	if _, ok := cw["wpm"]; !ok {
		t.Fatal("cw must expose a wpm setting")
	}
	if _, ok := cw["tone"]; !ok {
		t.Fatal("cw must expose a tone setting")
	}

	rtty := fieldKeys(modeFields("rtty"))
	if rtty["baud"].Kind != ui.FieldEnum || rtty["shift"].Kind != ui.FieldEnum {
		t.Fatal("rtty baud/shift must be enum pickers")
	}
	if !rtty["center"].Advanced || !rtty["reverse"].Advanced {
		t.Fatal("rtty center/reverse must be tucked into advanced settings")
	}
	if rtty["reverse"].Kind != ui.FieldToggle {
		t.Fatal("rtty reverse must be a toggle")
	}

	psk := modeFields("psk31")
	if len(psk) != 1 || psk[0].Key != "center" || psk[0].Default != "1000" {
		t.Fatalf("psk31 must expose one center field defaulting to 1000, got %+v", psk)
	}
	if got := modeFields("psk63"); got[0].Default != "1500" {
		t.Fatalf("psk63 center must default to 1500, got %q", got[0].Default)
	}

	if len(modeFields("ft8")) != 0 {
		t.Fatal("ft8 must expose no operator settings")
	}
	if len(modeFields("contestia8_500")) != 0 {
		t.Fatal("contestia submodes are fixed by label; they expose no settings")
	}
}

// The submode-family center default must match the daemon's per-mode default, so
// opening the editor and saving without touching Center doesn't overwrite the
// mode's real center (navtex/sitorb 1000 Hz, wefax 1900 Hz, not the generic 1500).
func TestCenterDefaultMatchesDaemon(t *testing.T) {
	for _, tc := range []struct {
		label string
		want  string
	}{
		{"navtex", "1000"},
		{"sitorb", "1000"},
		{"wefax576", "1900"},
		{"wefax288", "1900"},
		{"psk31", "1000"},
		{"mfsk16", "1500"},
	} {
		got := modeFields(tc.label)
		if len(got) != 1 || got[0].Key != "center" {
			t.Fatalf("%s must expose a single center field, got %+v", tc.label, got)
		}
		if got[0].Default != tc.want {
			t.Errorf("%s center default = %q, want %q", tc.label, got[0].Default, tc.want)
		}
	}
}

// Values edited in a mode's settings form must flow through into the typed
// ModeParams the daemon receives.
func TestModeValsReachModeParams(t *testing.T) {
	f := newModeSettingsForm("rtty", nil)
	// The form's first field is baud; walk to shift and bump it, then flip reverse.
	f.Update(tea.KeyMsg{Type: tea.KeyDown})  // -> shift
	f.Update(tea.KeyMsg{Type: tea.KeyRight}) // 170 -> 200
	vals := modeValsFrom(f)
	mp := modeParamsFor("rtty", vals)
	if mp.GetRtty().GetShiftHz() != 200 {
		t.Fatalf("edited shift must reach RttyParams, got %v", mp.GetRtty().GetShiftHz())
	}

	// A toggle round-trips as a boolean: afsk1200's Transmit off must clear Tx.
	af := newModeSettingsForm("afsk1200", nil)
	af.Update(tea.KeyMsg{Type: tea.KeySpace}) // Transmit on -> off
	if modeParamsFor("afsk1200", modeValsFrom(af)).GetAfsk1200().GetTx() {
		t.Fatal("turning Transmit off must clear Afsk1200Params.Tx")
	}
}

// The Settings row opens the editor with Enter, and only for modes that have
// something to tune.
func TestConfigSettingsModalOpens(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.modeIdx = modeIdxByLabel("cw")
	v.rebuildSettings()
	v.focus = fSettings

	if out := v.Render(80, 30); strings.Contains(out, "Speed") {
		t.Fatal("settings modal must be closed until opened")
	}
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if !v.editing {
		t.Fatal("enter on the Settings row must open the editor for a mode with settings")
	}
	if out := v.Render(80, 30); !strings.Contains(out, "Speed") {
		t.Fatalf("open editor must render the mode's fields:\n%s", out)
	}
	// Esc closes it without leaving the Configure screen.
	v.Update(tea.KeyMsg{Type: tea.KeyEsc})
	if v.editing {
		t.Fatal("esc must close the settings editor")
	}
}

// A mode with no settings (ft8) must not open an editor.
func TestConfigSettingsModalSkippedWhenNoSettings(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.modeIdx = modeIdxByLabel("ft8")
	v.rebuildSettings()
	v.focus = fSettings
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if v.editing {
		t.Fatal("a mode with no settings must not open the editor")
	}
	if out := v.Render(80, 30); !strings.Contains(out, "no settings") {
		t.Fatalf("the Settings row must read 'no settings' for a mode with none:\n%s", out)
	}
}

// Editing a mode setting auto-applies through the same channel-save pipeline as
// any other field, and the edited value reaches ConfigureChannel's ModeParams.
func TestConfigSettingsEditAutoApplies(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.modeIdx = modeIdxByLabel("rtty")
	v.rebuildSettings()
	v.saved = v.sig() // baseline as if the current config were already saved
	v.focus = fSettings

	// Open the editor and bump baud 45.45 -> 50 (the first field is an enum).
	v.Update(tea.KeyMsg{Type: tea.KeyEnter})
	_, cmd := v.Update(tea.KeyMsg{Type: tea.KeyRight})
	drainCmd(v, cmd)

	if len(f.ChannelCalls) == 0 {
		t.Fatal("editing a mode setting must auto-apply a ConfigureChannel")
	}
	last := f.ChannelCalls[len(f.ChannelCalls)-1]
	if got := last.GetModeParams().GetRtty().GetBaud(); got != 50 {
		t.Fatalf("edited baud must reach ModeParams, got %v", got)
	}
}

// Cycling the mode rebuilds the settings form so it always matches the selected
// mode (a psk63 center default, not the previous mode's fields).
func TestConfigModeCycleRebuildsSettings(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.modeIdx = modeIdxByLabel("cw")
	v.rebuildSettings()
	if v.settings.Value("wpm") != "20" {
		t.Fatalf("cw form should carry wpm, got %q", v.settings.Value("wpm"))
	}
	// Move to a PSK submode and confirm the form now carries center, not wpm.
	v.focus = fMode
	v.modeIdx = modeIdxByLabel("psk63")
	v.rebuildSettings()
	if v.settings.Value("center") != "1500" {
		t.Fatalf("after switching to psk63 the form must expose center=1500, got %q", v.settings.Value("center"))
	}
	if v.settings.Value("wpm") != "" {
		t.Fatal("the psk63 form must not retain the cw wpm field")
	}
}
