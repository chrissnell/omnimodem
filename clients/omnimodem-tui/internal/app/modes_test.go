package app

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// Guard against daemon↔TUI mode drift: every mode the daemon can run must be
// selectable in the operate screen. If the daemon gains a mode, add it here and
// to modes.go in the same change.
func TestAllDaemonModesAreExposed(t *testing.T) {
	want := []string{
		"psk31", "psk63", "psk125", "psk250", "psk500", "psk1000",
		"qpsk31", "qpsk63", "qpsk125", "qpsk250", "qpsk500",
		"psk63f", "psk125r", "psk250r", "psk500r", "psk1000r",
		"psk63rc4", "psk63rc5", "psk63rc10", "psk63rc20", "psk63rc32",
		"psk125rc4", "psk125rc5", "psk125rc10", "psk125rc12", "psk125rc16",
		"psk250rc2", "psk250rc3", "psk250rc5", "psk250rc6", "psk250rc7", "psk500rc2", "psk500rc3", "psk500rc4",
		"psk125c12", "psk250c6", "psk500c2", "psk500c4", "psk1000c2",
		"dominoexmicro", "dominoex4", "dominoex5", "dominoex8", "dominoex11",
		"dominoex16", "dominoex22", "dominoex44", "dominoex88",
		"feldhell", "slowhell", "hellx5", "hellx9", "hell80",
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
	// QPSK routes through the same PskParams oneof, carrying its own submode.
	if q := modeParamsFor("qpsk250", nil).GetPsk(); q == nil || q.GetSubmode() != "qpsk250" {
		t.Fatalf("qpsk250 must carry PskParams with submode qpsk250")
	}
}

// The DominoEX family carries its submode label and center through the dedicated
// DominoParams oneof, not the PSK arm.
func TestDominoExModeParams(t *testing.T) {
	mp := modeParamsFor("dominoex16", map[string]float64{"center": 1200})
	if mp == nil {
		t.Fatal("dominoex16 must produce typed ModeParams")
	}
	d := mp.GetDominoex()
	if d == nil {
		t.Fatalf("expected DominoParams, got %T", mp.GetParams())
	}
	if d.GetSubmode() != "dominoex16" || d.GetCenterHz() != 1200 {
		t.Fatalf("domino params = %q / %v, want dominoex16 / 1200", d.GetSubmode(), d.GetCenterHz())
	}
	if def := modeParamsFor("dominoex4", nil).GetDominoex(); def.GetCenterHz() != 1500 {
		t.Fatalf("dominoex4 default center = %v, want 1500", def.GetCenterHz())
	}
}

// The Feld Hell family carries its submode label and center through the dedicated
// HellParams oneof, and every submode uses the facsimile "image" shape.
func TestHellModeParams(t *testing.T) {
	mp := modeParamsFor("feldhell", map[string]float64{"center": 1200})
	if mp == nil {
		t.Fatal("feldhell must produce typed ModeParams")
	}
	h := mp.GetHell()
	if h == nil {
		t.Fatalf("expected HellParams, got %T", mp.GetParams())
	}
	if h.GetSubmode() != "feldhell" || h.GetCenterHz() != 1200 {
		t.Fatalf("hell params = %q / %v, want feldhell / 1200", h.GetSubmode(), h.GetCenterHz())
	}
	for _, label := range []string{"feldhell", "slowhell", "hellx5", "hellx9", "hell80"} {
		mi := modeByLabel(label)
		if mi == nil || mi.shape != "image" {
			t.Fatalf("%s must use the image shape, got %v", label, mi)
		}
		if def := modeParamsFor(label, nil).GetHell(); def == nil || def.GetCenterHz() != 1500 {
			t.Fatalf("%s default center = %v, want 1500", label, def.GetCenterHz())
		}
	}
}

// The image shape (Hell) attaches a raster surface — not a chat transcript — and
// folds received Image frames into a scrolling raster it renders.
func TestHellImageShapeRendersRaster(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "feldhell"}
	m.sel = 0
	v := newOperateView(m)
	if v.raster == nil {
		t.Fatal("feldhell should attach a raster surface")
	}
	if v.seq != nil || v.beacon {
		t.Fatal("feldhell must not be a sequencer/beacon")
	}
	// A received Image frame (14-tall column with an all-on column) folds in and
	// renders a block glyph.
	on := make([]byte, 14)
	for i := range on {
		on[i] = 255
	}
	v.raster.push(&pb.Image{Width: 14, Gray: on})
	if got := v.raster.render(80); !strings.Contains(got, "#") {
		t.Fatalf("raster should render on-pixels as blocks, got %q", got)
	}
	if got := v.Render(80, 24); !strings.Contains(got, "FELDHELL") {
		t.Fatalf("raster header should name the mode, got %q", firstLine(got))
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

// WSPR is a beacon: receive-only monitor with no QSO ladder or free-text
// compose. Typing is ignored, but enter keys a single "CALL GRID DBM" beacon
// (with a configured call/grid).
func TestWSPRIsBeacon(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.myCall, m.myGrid = "K1ABC", "FN42"
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
	// Enter keys the beacon.
	if _, cmd := v.Update(tea.KeyMsg{Type: tea.KeyEnter}); cmd == nil {
		t.Fatal("beacon enter should key a WSPR beacon")
	}
	if v.tx.phase != txAcquiring {
		t.Fatalf("beacon enter should start TX, phase=%v", v.tx.phase)
	}
}

func firstLine(s string) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		return s[:i]
	}
	return s
}
