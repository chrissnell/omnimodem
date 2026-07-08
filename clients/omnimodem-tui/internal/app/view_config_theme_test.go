package app

import (
	"regexp"
	"strings"
	"testing"

	"github.com/charmbracelet/lipgloss"
	"github.com/muesli/termenv"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// A mid-line SGR reset (ESC[0m) followed by any visible character means that
// character has no background set, so it renders on the terminal's own (often
// grey) background — the "dark grey box" artifact. Every run inside the black
// panel must carry the panel background, so no such hole may appear.
var greyHoleRe = regexp.MustCompile("\x1b\\[0m[^\x1b\n]")

// The Configure screen must never leave a background hole in any of its states —
// form, focused text field, settings editor, or an open picker.
func TestConfigNoGreyBackgroundHoles(t *testing.T) {
	prev := lipgloss.ColorProfile()
	lipgloss.SetColorProfile(termenv.TrueColor) // force real SGR codes in tests
	defer lipgloss.SetColorProfile(prev)

	m := New(&client.Fake{}, "/tmp/omnimodem.sock")
	m.connected = true
	m.version = "dev"
	m.width, m.height = 92, 24
	m.sel = 0
	m.live[0] = &chanLive{
		name: "vfo-a", mode: "psk125",
		deviceID:  "virtual:LG UltraFine Display Audio",
		pttMethod: pb.PttMethod_PTT_METHOD_VOX, rsidTx: true, rsidRx: true,
	}
	m.myCall, m.myGrid = "NW5W", "DN40CL"
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{
		devItem("virtual:LG UltraFine Display Audio", "LG UltraFine Display Audio", false, true),
		devItem("bh", "BlackHole 2ch", true, true),
	})
	m.push(v)

	check := func(state string) {
		out := m.View()
		for i, ln := range strings.Split(out, "\n") {
			if greyHoleRe.MatchString(ln) {
				t.Fatalf("%s: background hole (styled run then bare text) on line %d:\n%s",
					state, i, strings.ReplaceAll(ln, "\x1b", "^["))
			}
		}
	}

	v.focus = fFamily
	check("form")
	v.focus = fName // a focused text field shows its cursor
	check("name-focused")
	v.focus = fSettings
	v.editing = true
	check("settings-editor")
	v.editing = false
	v.focus = fRx
	v.openPicker(pickDevice)
	check("device-picker")
	v.closePicker()
	v.focus = fFamily
	v.openPicker(pickFamily)
	check("family-picker")
}
