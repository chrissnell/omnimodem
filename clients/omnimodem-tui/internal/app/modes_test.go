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
		"thormicro", "thor4", "thor5", "thor8", "thor11", "thor16", "thor22",
		"thor25x4", "thor50x1", "thor50x2", "thor100",
		"feldhell", "slowhell", "hellx5", "hellx9", "hell80",
		"scottie1", "scottie2", "scottiedx", "martin1", "martin2", "sc2-180", "sc2-120", "sc2-60",
		"robot72", "robot36", "robot24", "bw8", "bw12", "p3", "p5", "p7",
		"pd50", "pd90", "pd120", "pd160", "pd180", "pd240", "pd290",
		"mp73", "mp115", "mp140", "mp175",
		"mr73", "mr90", "mr115", "mr140", "mr175", "ml180", "ml240", "ml280", "ml320",
		"mp73-n", "mp110-n", "mp140-n", "mc110-n", "mc140-n", "mc180-n", "avt90",
		"mfsk4", "mfsk8", "mfsk11", "mfsk16", "mfsk22", "mfsk31",
		"mfsk32", "mfsk64", "mfsk128", "mfsk64l", "mfsk128l",
		"contestia4_125", "contestia4_250", "contestia4_500", "contestia4_1000", "contestia4_2000",
		"contestia8_125", "contestia8_250", "contestia8_500", "contestia8_1000", "contestia8_2000",
		"contestia16_250", "contestia16_500", "contestia16_1000", "contestia16_2000",
		"contestia32_1000", "contestia32_2000",
		"contestia64_500", "contestia64_1000", "contestia64_2000",
		"mt63_500s", "mt63_500l", "mt63_1000s", "mt63_1000l", "mt63_2000s", "mt63_2000l",
		"navtex", "sitorb", "wefax576", "wefax288",
		"rtty", "cw", "afsk1200", "olivia", "ft8", "ft4", "jt65", "jt9", "fst4", "msk144", "wspr", "js8",
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

// The THOR family carries its submode label and center through the dedicated
// ThorParams oneof, and every submode uses the ragchew "chat" shape.
func TestThorModeParams(t *testing.T) {
	mp := modeParamsFor("thor16", map[string]float64{"center": 1200})
	if mp == nil {
		t.Fatal("thor16 must produce typed ModeParams")
	}
	th := mp.GetThor()
	if th == nil {
		t.Fatalf("expected ThorParams, got %T", mp.GetParams())
	}
	if th.GetSubmode() != "thor16" || th.GetCenterHz() != 1200 {
		t.Fatalf("thor params = %q / %v, want thor16 / 1200", th.GetSubmode(), th.GetCenterHz())
	}
	for _, label := range []string{"thormicro", "thor4", "thor16", "thor25x4", "thor100"} {
		mi := modeByLabel(label)
		if mi == nil || mi.shape != "chat" {
			t.Fatalf("%s must use the chat shape, got %v", label, mi)
		}
		if def := modeParamsFor(label, nil).GetThor(); def == nil || def.GetCenterHz() != 1500 {
			t.Fatalf("%s default center = %v, want 1500", label, def.GetCenterHz())
		}
	}
}

// The Throb family carries its submode label and center through the dedicated
// ThrobParams oneof, and every submode uses the ragchew "chat" shape.
func TestThrobModeParams(t *testing.T) {
	mp := modeParamsFor("throb2", map[string]float64{"center": 1200})
	if mp == nil {
		t.Fatal("throb2 must produce typed ModeParams")
	}
	th := mp.GetThrob()
	if th == nil {
		t.Fatalf("expected ThrobParams, got %T", mp.GetParams())
	}
	if th.GetSubmode() != "throb2" || th.GetCenterHz() != 1200 {
		t.Fatalf("throb params = %q / %v, want throb2 / 1200", th.GetSubmode(), th.GetCenterHz())
	}
	for _, label := range []string{"throb1", "throb2", "throb4", "throbx1", "throbx2", "throbx4"} {
		mi := modeByLabel(label)
		if mi == nil || mi.shape != "chat" {
			t.Fatalf("%s must use the chat shape, got %v", label, mi)
		}
		if def := modeParamsFor(label, nil).GetThrob(); def == nil || def.GetCenterHz() != 1500 {
			t.Fatalf("%s default center = %v, want 1500", label, def.GetCenterHz())
		}
	}
}

// The IFKP family carries its speed label and center through the dedicated
// IfkpParams oneof, and every speed uses the ragchew "chat" shape.
func TestIfkpModeParams(t *testing.T) {
	mp := modeParamsFor("ifkp-slow", map[string]float64{"center": 1200})
	if mp == nil {
		t.Fatal("ifkp-slow must produce typed ModeParams")
	}
	p := mp.GetIfkp()
	if p == nil {
		t.Fatalf("expected IfkpParams, got %T", mp.GetParams())
	}
	if p.GetSpeed() != "ifkp-slow" || p.GetCenterHz() != 1200 {
		t.Fatalf("ifkp params = %q / %v, want ifkp-slow / 1200", p.GetSpeed(), p.GetCenterHz())
	}
	for _, label := range []string{"ifkp", "ifkp-slow", "ifkp-fast"} {
		mi := modeByLabel(label)
		if mi == nil || mi.shape != "chat" {
			t.Fatalf("%s must use the chat shape, got %v", label, mi)
		}
		if def := modeParamsFor(label, nil).GetIfkp(); def == nil || def.GetCenterHz() != 1500 {
			t.Fatalf("%s default center = %v, want 1500", label, def.GetCenterHz())
		}
	}
}

// The FSQ family carries its speed label, center, and directed flag through the
// dedicated FsqParams oneof; every speed uses the "chat" shape. The mycall is
// injected from the station identity at persist time, so it is empty here.
func TestFsqModeParams(t *testing.T) {
	mp := modeParamsFor("fsq", map[string]float64{"center": 1200, "directed": 1})
	if mp == nil {
		t.Fatal("fsq must produce typed ModeParams")
	}
	p := mp.GetFsq()
	if p == nil {
		t.Fatalf("expected FsqParams, got %T", mp.GetParams())
	}
	if p.GetSpeed() != "fsq" || p.GetCenterHz() != 1200 || !p.GetDirected() {
		t.Fatalf("fsq params = %q / %v / %v, want fsq / 1200 / true",
			p.GetSpeed(), p.GetCenterHz(), p.GetDirected())
	}
	for _, label := range []string{"fsq", "fsq-1.5", "fsq-2", "fsq-4.5", "fsq-6"} {
		mi := modeByLabel(label)
		if mi == nil || mi.shape != "chat" {
			t.Fatalf("%s must use the chat shape, got %v", label, mi)
		}
		def := modeParamsFor(label, nil).GetFsq()
		if def == nil || def.GetCenterHz() != 1500 || def.GetDirected() {
			t.Fatalf("%s default = %v, want center 1500 / directed false", label, def)
		}
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

// The MFSK family carries its submode label and center through the dedicated
// MfskParams oneof, not the PSK/DominoEX arm.
func TestMfskModeParams(t *testing.T) {
	mp := modeParamsFor("mfsk16", map[string]float64{"center": 1200})
	if mp == nil {
		t.Fatal("mfsk16 must produce typed ModeParams")
	}
	m := mp.GetMfsk()
	if m == nil {
		t.Fatalf("expected MfskParams, got %T", mp.GetParams())
	}
	if m.GetSubmode() != "mfsk16" || m.GetCenterHz() != 1200 {
		t.Fatalf("mfsk params = %q / %v, want mfsk16 / 1200", m.GetSubmode(), m.GetCenterHz())
	}
	if def := modeParamsFor("mfsk64l", nil).GetMfsk(); def == nil || def.GetCenterHz() != 1500 {
		t.Fatalf("mfsk64l default center = %v, want 1500", def.GetCenterHz())
	}
}

// The MT63 family carries its submode label and center through the dedicated
// Mt63Params oneof, not the MFSK/DominoEX arm.
func TestMt63ModeParams(t *testing.T) {
	mp := modeParamsFor("mt63_1000l", map[string]float64{"center": 1200})
	if mp == nil {
		t.Fatal("mt63_1000l must produce typed ModeParams")
	}
	m := mp.GetMt63()
	if m == nil {
		t.Fatalf("expected Mt63Params, got %T", mp.GetParams())
	}
	if m.GetSubmode() != "mt63_1000l" || m.GetCenterHz() != 1200 {
		t.Fatalf("mt63 params = %q / %v, want mt63_1000l / 1200", m.GetSubmode(), m.GetCenterHz())
	}
	if def := modeParamsFor("mt63_2000s", nil).GetMt63(); def == nil || def.GetCenterHz() != 1500 {
		t.Fatalf("mt63_2000s default center = %v, want 1500", def.GetCenterHz())
	}
}

// NAVTEX / SITOR-B carry their submode label + center through the dedicated
// NavtexParams oneof; both use the "chat" text surface.
func TestNavtexModeParams(t *testing.T) {
	mp := modeParamsFor("sitorb", map[string]float64{"center": 1500})
	if mp == nil {
		t.Fatal("sitorb must produce typed ModeParams")
	}
	n := mp.GetNavtex()
	if n == nil {
		t.Fatalf("expected NavtexParams, got %T", mp.GetParams())
	}
	if n.GetSubmode() != "sitorb" || n.GetCenterHz() != 1500 {
		t.Fatalf("navtex params = %q / %v, want sitorb / 1500", n.GetSubmode(), n.GetCenterHz())
	}
	for _, label := range []string{"navtex", "sitorb"} {
		if mi := modeByLabel(label); mi == nil || mi.shape != "chat" {
			t.Fatalf("%s must use the chat shape, got %v", label, mi)
		}
		if def := modeParamsFor(label, nil).GetNavtex(); def == nil || def.GetCenterHz() != 1000 {
			t.Fatalf("%s default center = %v, want 1000", label, def.GetCenterHz())
		}
	}
}

// WEFAX carries its submode label + carrier through the dedicated WefaxParams
// oneof, and both submodes use the facsimile "image" shape.
func TestWefaxModeParams(t *testing.T) {
	mp := modeParamsFor("wefax576", map[string]float64{"center": 1800})
	if mp == nil {
		t.Fatal("wefax576 must produce typed ModeParams")
	}
	w := mp.GetWefax()
	if w == nil {
		t.Fatalf("expected WefaxParams, got %T", mp.GetParams())
	}
	if w.GetSubmode() != "wefax576" || w.GetCenterHz() != 1800 {
		t.Fatalf("wefax params = %q / %v, want wefax576 / 1800", w.GetSubmode(), w.GetCenterHz())
	}
	for _, label := range []string{"wefax576", "wefax288"} {
		if mi := modeByLabel(label); mi == nil || mi.shape != "image" {
			t.Fatalf("%s must use the image shape, got %v", label, mi)
		}
		if def := modeParamsFor(label, nil).GetWefax(); def == nil || def.GetCenterHz() != 1900 {
			t.Fatalf("%s default center = %v, want 1900", label, def.GetCenterHz())
		}
	}
}

// The Contestia grid carries tones + bandwidth through the dedicated
// ContestiaParams oneof; defaults come from the submode's grid coordinates.
func TestContestiaModeParams(t *testing.T) {
	d := modeParamsFor("contestia8_500", nil).GetContestia()
	if d == nil {
		t.Fatal("contestia8_500 must produce typed ContestiaParams")
	}
	if d.GetTones() != 8 || d.GetBandwidthHz() != 500 {
		t.Fatalf("contestia8_500 defaults = %d / %d, want 8 / 500", d.GetTones(), d.GetBandwidthHz())
	}
	// An operator override round-trips.
	o := modeParamsFor("contestia32_1000", map[string]float64{"tones": 32, "bw": 2000}).GetContestia()
	if o.GetTones() != 32 || o.GetBandwidthHz() != 2000 {
		t.Fatalf("contestia override = %d / %d, want 32 / 2000", o.GetTones(), o.GetBandwidthHz())
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
	v.raster.push(&pb.Image{Width: 14, Pixels: on})
	if got := v.raster.render(80); !strings.Contains(got, "#") {
		t.Fatalf("raster should render on-pixels as blocks, got %q", got)
	}
	if got := v.Render(80, 24); !strings.Contains(got, "FELDHELL") {
		t.Fatalf("raster header should name the mode, got %q", firstLine(got))
	}
}

// A colour (channels=3) Image frame from the picture sub-protocols folds into
// the same raster surface, reduced to luminance for the monochrome terminal.
func TestRasterFoldsColorImageAsLuma(t *testing.T) {
	var r rasterBuf
	// One 2-pixel column: white and black RGB pixels.
	r.push(&pb.Image{Width: 2, Channels: 3, Pixels: []byte{255, 255, 255, 0, 0, 0}})
	if r.width != 2 || len(r.cols) != 1 {
		t.Fatalf("expected one 2-tall column, got width=%d cols=%d", r.width, len(r.cols))
	}
	if r.cols[0][0] != 255 || r.cols[0][1] != 0 {
		t.Fatalf("RGB should reduce to luma {255,0}, got %v", r.cols[0])
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

// MSK144 is a streaming meteor-scatter QSO mode: it attaches the sequencer
// ladder (like FT8) but has no fixed T/R slot (slotSecs 0), since the demod runs
// continuously on short bursts rather than a windowed grid.
func TestMSK144IsStreamingSequencer(t *testing.T) {
	mi := modeByLabel("msk144")
	if mi == nil {
		t.Fatal("msk144 not offered in the modes list")
	}
	if mi.shape != "sequencer" {
		t.Fatalf("msk144 shape = %q, want sequencer", mi.shape)
	}
	if mi.slotSecs != 0 {
		t.Fatalf("msk144 slotSecs = %v, want 0 (streaming)", mi.slotSecs)
	}
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{mode: "msk144"}
	m.sel = 0
	v := newOperateView(m)
	if v.seq == nil {
		t.Fatal("msk144 should attach a sequencer ladder")
	}
	if v.beacon {
		t.Fatal("msk144 must not be a beacon")
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
