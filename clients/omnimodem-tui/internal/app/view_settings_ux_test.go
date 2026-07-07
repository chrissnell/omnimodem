package app

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// The Settings row shows a count and an edit cue, not the individual values —
// a lone value (e.g. the center freq) next to the row is confusing and useless
// once a mode has more than one setting.
func TestSettingsRowShowsCountNotValue(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.modeIdx = modeIdxByLabel("psk31")
	v.rebuildSettings()
	out := v.Render(100, 30)
	if !strings.Contains(out, "1 setting") || !strings.Contains(out, "edit") {
		t.Fatalf("Settings row must show a count + edit cue:\n%s", out)
	}
	if strings.Contains(out, "1000") {
		t.Fatalf("Settings row must not surface the raw setting value:\n%s", out)
	}
}

// The mode-settings modal header is just "<mode> settings"; the hotkeys live on
// their own line along the bottom, not crammed into the title (where they wrapped).
func TestSettingsModalHeaderAndHotkeys(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.modeIdx = modeIdxByLabel("psk31")
	v.rebuildSettings()
	v.focus = fSettings
	v.Update(tea.KeyMsg{Type: tea.KeyEnter}) // open editor
	out := v.Render(100, 30)
	if !strings.Contains(out, "psk31 settings") {
		t.Fatalf("modal header must read 'psk31 settings':\n%s", out)
	}
	// The hotkeys must NOT be appended to the title (the old wrapping layout).
	if strings.Contains(out, "settings  ‹↑") || strings.Contains(out, "settings  ↑") {
		t.Fatalf("hotkeys must not be crammed into the title:\n%s", out)
	}
	// They must appear on their own footer line inside the dialog.
	if !strings.Contains(out, "space toggle") || !strings.Contains(out, "esc done") {
		t.Fatalf("hotkeys must be shown along the bottom of the dialog:\n%s", out)
	}
}

// An edited mode setting must survive closing and reopening the Configure screen:
// the value is cached on the Model when the save confirms and re-seeds the form.
func TestSettingsPersistAcrossReopen(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	m.live[0] = &chanLive{name: "vfo-a", mode: "psk31", deviceID: "usb:1:2:"}
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.rxID = "usb:1:2:"
	v.saved = v.sig()

	// Edit center 1000 -> 1200 directly in the form (focus is on the sole field).
	for i := 0; i < 4; i++ {
		v.settings.Update(tea.KeyMsg{Type: tea.KeyBackspace})
	}
	for _, r := range "1200" {
		v.settings.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
	}
	if v.settings.Value("center") != "1200" {
		t.Fatalf("precondition: form should hold 1200, got %q", v.settings.Value("center"))
	}
	drainCmd(v, v.maybePersist())

	// The save reached the daemon with the edited center...
	if n := len(f.ChannelCalls); n == 0 || f.ChannelCalls[n-1].GetModeParams().GetPsk().GetCenterHz() != 1200 {
		t.Fatalf("edited center must reach ConfigureChannel, calls=%+v", f.ChannelCalls)
	}
	// ...and was cached on the Model.
	if sp := m.modeParams[0]; sp.label != "psk31" || sp.vals["center"] != 1200 {
		t.Fatalf("saved settings must be cached on the Model, got %+v", m.modeParams[0])
	}

	// Reopen Configure: the form must show 1200, not the 1000 default.
	v2 := newConfigView(m)
	if got := v2.settings.Value("center"); got != "1200" {
		t.Fatalf("reopened form must show the saved 1200, got %q", got)
	}
}
