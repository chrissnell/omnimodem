package app

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
)

// Guard against daemon↔TUI mode drift: every mode the daemon can run must be
// selectable in the operate screen. If the daemon gains a mode, add it here and
// to modes.go in the same change.
func TestAllDaemonModesAreExposed(t *testing.T) {
	want := []string{
		"psk31", "psk63", "psk125", "psk250", "psk500", "psk1000",
		"rtty", "cw", "afsk1200", "olivia", "ft8", "ft4", "jt65", "jt9", "fst4", "wspr",
	}
	for _, label := range want {
		if modeByLabel(label) == nil {
			t.Errorf("mode %q is not offered in the TUI modes list", label)
		}
	}
}

// Olivia carries typed params (tones + bandwidth); the oneof must round-trip the
// operator's values, not silently fall through to the bare-label default.
func TestOliviaModeParams(t *testing.T) {
	mp := modeParamsFor("olivia", map[string]float64{"tones": 16, "bw": 500})
	if mp == nil {
		t.Fatal("olivia must produce typed ModeParams")
	}
	o := mp.GetOlivia()
	if o == nil {
		t.Fatalf("expected OliviaParams, got %T", mp.GetParams())
	}
	if o.GetTones() != 16 || o.GetBandwidthHz() != 500 {
		t.Fatalf("olivia params = tones %d / bw %d, want 16 / 500", o.GetTones(), o.GetBandwidthHz())
	}
	// Defaults when the operator supplied nothing.
	d := modeParamsFor("olivia", nil).GetOlivia()
	if d.GetTones() != 32 || d.GetBandwidthHz() != 1000 {
		t.Fatalf("olivia defaults = %d / %d, want 32 / 1000", d.GetTones(), d.GetBandwidthHz())
	}
}

// The PSK family carries its submode label and center; the oneof must round-trip
// the operator's values rather than fall through to the bare-label default.
func TestPskModeParams(t *testing.T) {
	mp := modeParamsFor("psk250", map[string]float64{"center": 1200})
	if mp == nil {
		t.Fatal("psk250 must produce typed ModeParams")
	}
	p := mp.GetPsk()
	if p == nil {
		t.Fatalf("expected PskParams, got %T", mp.GetParams())
	}
	if p.GetSubmode() != "psk250" || p.GetCenterHz() != 1200 {
		t.Fatalf("psk params = %q / %v, want psk250 / 1200", p.GetSubmode(), p.GetCenterHz())
	}
	// Defaults: psk31 centres at 1000 Hz, the higher rates at 1500 Hz.
	if d := modeParamsFor("psk31", nil).GetPsk(); d.GetCenterHz() != 1000 {
		t.Fatalf("psk31 default center = %v, want 1000", d.GetCenterHz())
	}
	if d := modeParamsFor("psk125", nil).GetPsk(); d.GetCenterHz() != 1500 {
		t.Fatalf("psk125 default center = %v, want 1500", d.GetCenterHz())
	}
}

// The sequencer shape (FT8/FT4/JT65/JT9) attaches the QSO ladder and carries the
// mode's own slot length — the header must not hardcode FT8/15 s.
func TestSequencerModesAttachLadderWithOwnSlot(t *testing.T) {
	for _, tc := range []struct {
		mode string
		slot float64
	}{{"ft8", 15}, {"ft4", 7.5}, {"jt65", 60}, {"jt9", 60}} {
		m := New(&client.Fake{}, "x")
		m.live[0] = &chanLive{mode: tc.mode}
		m.sel = 0
		v := newOperateView(m)
		if v.seq == nil {
			t.Errorf("%s should attach a sequencer", tc.mode)
			continue
		}
		if v.beacon {
			t.Errorf("%s must not be a beacon", tc.mode)
		}
		if v.slotSecs != tc.slot {
			t.Errorf("%s slot = %v, want %v", tc.mode, v.slotSecs, tc.slot)
		}
		if got := v.Render(80, 20); !strings.Contains(got, strings.ToUpper(tc.mode)+" · slot") {
			t.Errorf("%s header should name the mode, got %q", tc.mode, firstLine(got))
		}
	}
}

// WSPR is a beacon: receive-only monitor, no QSO ladder, and keystrokes/enter
// must never compose or key a transmission from the operate screen.
func TestWSPRIsBeaconAndInert(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "wspr"}
	m.sel = 0
	v := newOperateView(m)
	if !v.beacon || v.seq != nil {
		t.Fatalf("wspr should be a beacon (beacon=%v, seq=%v)", v.beacon, v.seq != nil)
	}
	// Typing must not accumulate a compose buffer.
	v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("CQ")})
	if v.compose != "" {
		t.Fatalf("beacon must ignore typing, compose=%q", v.compose)
	}
	// Enter must not begin a transmission.
	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyEnter}); cmd != nil {
		t.Fatal("beacon enter must not transmit")
	}
	if v.tx.active() {
		t.Fatal("beacon must not key TX from the operate screen")
	}
}

func firstLine(s string) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		return s[:i]
	}
	return s
}
