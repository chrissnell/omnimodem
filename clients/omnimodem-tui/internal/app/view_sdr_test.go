package app

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

func keyRunes(s string) tea.KeyMsg { return tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(s)} }

// run drives a view with one message and, if it returns a command, executes it
// (commands are where the RPC calls live) so the Fake records the request.
func run(t *testing.T, v View, msg tea.Msg) tea.Msg {
	t.Helper()
	_, cmd := v.Update(msg)
	if cmd == nil {
		return nil
	}
	return cmd()
}

func sdrTestView(f *client.Fake) *sdrView {
	m := New(f, "x")
	m.live[0] = &chanLive{
		deviceID: "rtltcp:127.0.0.1:1234", haveSdr: true,
		sdrFreqHz: 144_390_000, sdrCenterHz: 144_390_000,
		sdrGainAuto: true, sdrGainDb: 0, sdrDemod: pb.DemodMode_DEMOD_NBFM, sdrSquelchDb: -30,
	}
	m.sel = 0
	m.stack = []View{newChannelsView(m)}
	v := newSdrView(m)
	m.push(v)
	v.caps = &pb.GetSdrCapsResponse{
		FreqMinHz: 24_000_000, FreqMaxHz: 1_766_000_000,
		GainsDb: []float32{0, 9, 21, 30, 49},
	}
	return v
}

// Right/left arrows tune by the current step, clamped to the tuner range.
func TestSdrStepTune(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	run(t, v, tea.KeyMsg{Type: tea.KeyRight}) // +1 kHz (default step)
	if len(f.TuneCalls) != 1 || f.TuneCalls[0].GetFreqHz() != 144_391_000 {
		t.Fatalf("right should tune +1 kHz, got %+v", f.TuneCalls)
	}
	run(t, v, tea.KeyMsg{Type: tea.KeyLeft}) // -1 kHz
	if f.TuneCalls[1].GetFreqHz() != 144_389_000 {
		t.Fatalf("left should tune -1 kHz, got %v", f.TuneCalls[1].GetFreqHz())
	}
}

// Cycling the step size with `s` changes the tuning delta.
func TestSdrStepSizeCycle(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	run(t, v, keyRunes("s")) // 1k -> 5k
	run(t, v, tea.KeyMsg{Type: tea.KeyRight})
	if f.TuneCalls[0].GetFreqHz() != 144_395_000 {
		t.Fatalf("after one step-cycle, right should tune +5 kHz, got %v", f.TuneCalls[0].GetFreqHz())
	}
	run(t, v, keyRunes("s")) // 5k -> 12.5k
	run(t, v, tea.KeyMsg{Type: tea.KeyRight})
	if f.TuneCalls[1].GetFreqHz() != 144_402_500 {
		t.Fatalf("12.5 kHz step expected, got %v", f.TuneCalls[1].GetFreqHz())
	}
}

// Tuning past the tuner's advertised range clamps to the edge.
func TestSdrTuneClampsToRange(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	v.m.live[0].sdrFreqHz = v.caps.GetFreqMaxHz() // sitting at the top edge
	run(t, v, tea.KeyMsg{Type: tea.KeyRight})
	if f.TuneCalls[0].GetFreqHz() != v.caps.GetFreqMaxHz() {
		t.Fatalf("tune should clamp at freq_max, got %v", f.TuneCalls[0].GetFreqHz())
	}
}

// Direct entry parses a MHz value and tunes to it.
func TestSdrDirectEntry(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	run(t, v, keyRunes("f")) // open entry
	if !v.entering {
		t.Fatal("f should open direct entry")
	}
	run(t, v, keyRunes("146.52"))
	run(t, v, tea.KeyMsg{Type: tea.KeyEnter})
	if v.entering {
		t.Fatal("enter should close direct entry")
	}
	if len(f.TuneCalls) != 1 || f.TuneCalls[0].GetFreqHz() != 146_520_000 {
		t.Fatalf("direct entry should tune 146.52 MHz, got %+v", f.TuneCalls)
	}
}

// A malformed entry warns and does not tune.
func TestSdrDirectEntryRejectsGarbage(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	run(t, v, keyRunes("f"))
	run(t, v, keyRunes("145.5")) // only digits/'.' are accepted; letters are dropped
	run(t, v, keyRunes("xyz"))
	if v.entryBuf != "145.5" {
		t.Fatalf("entry should ignore non-numeric runes, got %q", v.entryBuf)
	}
	run(t, v, tea.KeyMsg{Type: tea.KeyEnter})
	if len(f.TuneCalls) != 1 || f.TuneCalls[0].GetFreqHz() != 145_500_000 {
		t.Fatalf("expected 145.5 MHz tune, got %+v", f.TuneCalls)
	}
}

// `g` toggles AGC off to a manual table value; `[`/`]` step through the table.
func TestSdrGainControls(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	run(t, v, keyRunes("g")) // auto -> manual (snaps to nearest table entry, 0 dB)
	if len(f.SdrGainCalls) != 1 || f.SdrGainCalls[0].GetAuto() || f.SdrGainCalls[0].GetGainDb() != 0 {
		t.Fatalf("g should switch to manual 0 dB, got %+v", f.SdrGainCalls)
	}
	run(t, v, keyRunes("]")) // step up one table entry -> 9 dB
	if f.SdrGainCalls[1].GetGainDb() != 9 {
		t.Fatalf("] should step to 9 dB, got %v", f.SdrGainCalls[1].GetGainDb())
	}
	run(t, v, keyRunes("[")) // step down -> 0 dB
	if f.SdrGainCalls[2].GetGainDb() != 0 {
		t.Fatalf("[ should step back to 0 dB, got %v", f.SdrGainCalls[2].GetGainDb())
	}
	// Re-enable AGC.
	v.m.live[0].sdrGainAuto = false
	run(t, v, keyRunes("g"))
	if !f.SdrGainCalls[3].GetAuto() {
		t.Fatalf("g should re-enable auto, got %+v", f.SdrGainCalls[3])
	}
}

// `m` cycles the demod mode; `,`/`.` adjust squelch — both via ConfigureSdr.
func TestSdrDemodAndSquelch(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	run(t, v, keyRunes("m")) // NBFM -> AM
	if len(f.SdrConfigCalls) != 1 || f.SdrConfigCalls[0].GetDemodMode() != pb.DemodMode_DEMOD_AM {
		t.Fatalf("m should cycle to AM, got %+v", f.SdrConfigCalls)
	}
	run(t, v, keyRunes(".")) // squelch up: -30 (adopted) -> -29
	if f.SdrConfigCalls[1].GetSquelchDb() != -29 {
		t.Fatalf(". should raise squelch to -29, got %v", f.SdrConfigCalls[1].GetSquelchDb())
	}
	run(t, v, keyRunes(",")) // squelch down -> -30
	if f.SdrConfigCalls[2].GetSquelchDb() != -30 {
		t.Fatalf(", should lower squelch to -30, got %v", f.SdrConfigCalls[2].GetSquelchDb())
	}
	run(t, v, keyRunes("\\")) // toggle squelch off (sentinel)
	if f.SdrConfigCalls[3].GetSquelchDb() > squelchDisabled {
		t.Fatalf("\\ should disable squelch, got %v", f.SdrConfigCalls[3].GetSquelchDb())
	}
}

// ppm steps via ConfigureSdr and shows in the control bar.
func TestSdrPpm(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	run(t, v, keyRunes("+"))
	run(t, v, keyRunes("+"))
	if f.SdrConfigCalls[1].GetPpm() != 2 {
		t.Fatalf("two + should set ppm=2, got %v", f.SdrConfigCalls[1].GetPpm())
	}
	run(t, v, keyRunes("-"))
	if f.SdrConfigCalls[2].GetPpm() != 1 {
		t.Fatalf("- should drop ppm to 1, got %v", f.SdrConfigCalls[2].GetPpm())
	}
}

// Selecting a Phase-B demod mode (UNIMPLEMENTED daemon-side) surfaces an error
// and doesn't stick: the ConfigureSdr failure yields an rpcErrMsg that the Model
// turns into a toast, and the picker label (read from chanLive) stays on NBFM.
func TestSdrDemodUnimplemented(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	f.Err = status.Error(codes.Unimplemented, "demod mode AM lands in Phase B")
	msg := run(t, v, keyRunes("m")) // cycle NBFM -> AM, daemon rejects
	if _, ok := msg.(rpcErrMsg); !ok {
		t.Fatalf("UNIMPLEMENTED demod should yield rpcErrMsg, got %T", msg)
	}
	if got := demodLabel(v.demod(v.live())); got != "NBFM" {
		t.Fatalf("rejected demod must not stick; label = %s", got)
	}
	if v.m.Update(msg); v.m.toast == nil {
		t.Fatal("rpcErrMsg should raise a toast")
	}
}

// The cursor column maps an absolute RF frequency to the display column of its
// bin, and returns -1 for frequencies outside the shown span.
func TestWaterfallCursorColumn(t *testing.T) {
	var w waterfall
	// 100 bins, bin[0]=100.0 MHz, 1 kHz/bin → span [100.000, 100.100) MHz.
	w.push(&pb.SpectrumFrame{
		Bins:        make([]byte, 100),
		FreqStartHz: 100_000_000,
		FreqStepHz:  1000,
	})
	// A width == bins mapping is 1:1: bin 50 -> column 50.
	if got := w.cursorColumn(100, 100_050_000); got != 50 {
		t.Fatalf("mid-band frequency should map to column 50, got %d", got)
	}
	// Half the width folds two bins per column: bin 50 -> column 25.
	if got := w.cursorColumn(50, 100_050_000); got != 25 {
		t.Fatalf("half-width mapping should give column 25, got %d", got)
	}
	if got := w.cursorColumn(100, 200_000_000); got != -1 {
		t.Fatalf("out-of-span frequency should map to -1, got %d", got)
	}
}

// The cursor overlay actually paints a marker glyph on the rendered line.
func TestWaterfallCursorRenders(t *testing.T) {
	var w waterfall
	w.push(&pb.SpectrumFrame{Bins: make([]byte, 40), FreqStartHz: 100_000_000, FreqStepHz: 1000})
	line := spectrumLineCursor(w.rows[0], 40, 20)
	if !strings.Contains(line, "│") {
		t.Fatal("cursor line should contain the marker glyph")
	}
	plain := spectrumLineCursor(w.rows[0], 40, -1)
	if strings.Contains(plain, "│") {
		t.Fatal("no marker should be drawn when cursorCol < 0")
	}
}

// SdrState events fold into per-channel live state so the readout stays in sync.
func TestSdrStateFolds(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.applyEvent(&pb.Event{Kind: &pb.Event_SdrState{SdrState: &pb.SdrState{
		Channel: 2, CenterHz: 144_400_000, OffsetHz: -10_000, FreqHz: 144_390_000,
		GainAuto: false, GainDb: 21, DemodMode: pb.DemodMode_DEMOD_NBFM, SquelchDb: -25,
	}}})
	cl := m.live[2]
	if cl == nil || !cl.haveSdr {
		t.Fatal("SdrState should create+mark the channel")
	}
	if cl.sdrFreqHz != 144_390_000 || cl.sdrGainDb != 21 || cl.sdrSquelchDb != -25 {
		t.Fatalf("SdrState folded wrong: %+v", cl)
	}
}

// SDR-bound channels open the tuning view; soundcard channels open operate.
func TestChannelRoutingByDevice(t *testing.T) {
	// rtl_tcp -> sdrView
	f := &client.Fake{}
	m := New(f, "x")
	m.live[0] = &chanLive{deviceID: "rtltcp:127.0.0.1:1234", mode: "afsk1200"}
	m.sel = 0
	m.stack = []View{newChannelsView(m)}
	run(t, m.top(), tea.KeyMsg{Type: tea.KeyEnter})
	if _, ok := m.top().(*sdrView); !ok {
		t.Fatalf("rtl_tcp channel should open sdrView, got %T", m.top())
	}

	// soundcard -> operateView
	m2 := New(&client.Fake{}, "x")
	m2.live[0] = &chanLive{deviceID: "hw:1,0", mode: "psk31"}
	m2.sel = 0
	m2.stack = []View{newChannelsView(m2)}
	run(t, m2.top(), tea.KeyMsg{Type: tea.KeyEnter})
	if _, ok := m2.top().(*operateView); !ok {
		t.Fatalf("soundcard channel should open operateView, got %T", m2.top())
	}
}

// Render shows the RF readout and control bar, and folds a live spectrum frame
// into the waterfall with a demod cursor — without panicking at small sizes.
func TestSdrRender(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	v.Update(eventMsg{&pb.Event{Kind: &pb.Event_SpectrumFrame{SpectrumFrame: &pb.SpectrumFrame{
		Channel: 0, Bins: make([]byte, sdrBinCount), FreqStartHz: 144_290_000, FreqStepHz: 1000,
	}}}})
	out := v.Render(80, 20)
	for _, want := range []string{"144.390000 MHz", "step", "gain", "demod", "sql", "sig"} {
		if !strings.Contains(out, want) {
			t.Fatalf("render missing %q:\n%s", want, out)
		}
	}
	// Small terminal shouldn't panic.
	_ = v.Render(20, 6)
}

// Leaving the SDR view stops the RF spectrum producer.
func TestSdrEscDisablesSpectrum(t *testing.T) {
	f := &client.Fake{}
	v := sdrTestView(f)
	run(t, v, tea.KeyMsg{Type: tea.KeyEsc})
	if len(f.SpectrumCalls) == 0 || f.SpectrumCalls[len(f.SpectrumCalls)-1].GetEnable() {
		t.Fatalf("esc should disable the spectrum, calls=%+v", f.SpectrumCalls)
	}
}
