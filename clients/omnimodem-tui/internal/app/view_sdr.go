package app

import (
	"fmt"
	"strconv"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

// squelchDisabled is the ConfigureSdr sentinel that turns squelch off (any
// threshold at or below this passes all audio). Matches the proto contract.
const squelchDisabled = -200.0

// defaultDemodFreqHz seeds the tuning base before any SdrState/caps arrive so
// arrow tuning has a sane starting point — the 2 m APRS calling frequency.
const defaultDemodFreqHz = 144_390_000.0

// sdrBinCount is the RF waterfall's display width in bins. Wider than the
// operate view's 64 so a wideband captured band reads with useful resolution.
const sdrBinCount = 128

// sdrView is the RTL-SDR tuning screen: a large RF readout, the wideband RF
// waterfall with a demod-channel cursor, and a control bar for step tuning,
// gain, ppm, demod mode, and squelch. The daemon is authoritative for the tune/
// gain/demod/squelch state (folded into chanLive from SdrState events); this
// view holds only its own editing state and reads chanLive each render.
type sdrView struct {
	m       *Model
	rxWf    waterfall
	caps    *pb.GetSdrCapsResponse
	stepIdx int // index into sdrSteps
	gainIdx int // cursor into caps.gains for manual stepping; -1 unknown
	// ppm is the operator's desired frequency correction. It is NOT carried in
	// SdrState (the daemon boots it at 0 and, in Phase A, only a ConfigureSdr call
	// changes it — end-to-end ppm wiring is Phase C), so unlike squelch/gain it
	// cannot be adopted from the daemon. This view is therefore the sole author of
	// ppm for a single operator; with multiple concurrent clients ppm can't be
	// kept in sync until SdrState carries it. See the review note on GRA-300.
	ppm     int32
	squelch float32
	adopted bool // adopted the daemon's squelch/gain into editing state yet

	// Direct frequency entry: `f` opens it; digits + '.' build a MHz string;
	// enter applies, esc cancels the edit (not the screen).
	entering bool
	entryBuf string
}

func newSdrView(m *Model) *sdrView {
	return &sdrView{m: m, gainIdx: -1, squelch: -30}
}

// live returns the selected channel's live state (may be nil very briefly before
// the first snapshot).
func (v *sdrView) live() *chanLive { return v.m.live[v.m.sel] }

// sync seats the editing state on the daemon's reported values the first time we
// have them, so stepping gain/squelch starts from the real current value rather
// than a stale local default.
func (v *sdrView) sync(cl *chanLive) {
	if cl == nil || v.adopted || !cl.haveSdr {
		return
	}
	v.squelch = cl.sdrSquelchDb
	if v.caps != nil {
		v.gainIdx = nearestGainIdx(v.caps.GetGainsDb(), cl.sdrGainDb)
	}
	v.adopted = true
}

// baseFreq is the frequency arrow tuning works from: the daemon's effective
// demod freq when known, else the tuner's mid-band, else the APRS default.
func (v *sdrView) baseFreq(cl *chanLive) float64 {
	if cl != nil && cl.haveSdr && cl.sdrFreqHz > 0 {
		return cl.sdrFreqHz
	}
	if v.caps != nil {
		lo, hi := v.caps.GetFreqMinHz(), v.caps.GetFreqMaxHz()
		if lo > 0 && hi > lo {
			return lo + (hi-lo)/2
		}
	}
	return defaultDemodFreqHz
}

func (v *sdrView) Update(msg tea.Msg) (View, tea.Cmd) {
	cl := v.live()
	v.sync(cl)
	switch msg := msg.(type) {
	case eventMsg:
		if sf := msg.ev.GetSpectrumFrame(); sf != nil && !sf.GetTransmit() && sf.GetChannel() == v.m.sel {
			v.rxWf.push(sf)
		}
		return v, nil
	case spectrumCfgMsg:
		return v, nil
	case sdrCapsMsg:
		v.caps = msg.resp
		if cl != nil && cl.haveSdr {
			v.gainIdx = nearestGainIdx(v.caps.GetGainsDb(), cl.sdrGainDb)
		}
		return v, nil
	case sdrTuneMsg, sdrGainMsg:
		// The daemon echoes the applied value and follows with an SdrState event
		// that folds into chanLive; nothing to store here.
		return v, nil
	case sdrConfigMsg:
		return v, nil
	case tea.KeyMsg:
		if v.entering {
			return v, v.handleEntry(msg)
		}
		return v, v.handleKey(msg)
	}
	return v, nil
}

// handleEntry drives the direct-frequency editor. Enter applies the buffer as
// MHz; esc cancels; digits and one decimal point build the value.
func (v *sdrView) handleEntry(msg tea.KeyMsg) tea.Cmd {
	switch msg.String() {
	case "esc":
		v.entering, v.entryBuf = false, ""
		return nil
	case "enter":
		v.entering = false
		buf := v.entryBuf
		v.entryBuf = ""
		mhz, err := strconv.ParseFloat(strings.TrimSpace(buf), 64)
		if err != nil || mhz <= 0 {
			v.m.toast = ui.NewToast("Invalid frequency: "+buf, ui.SeverityWarn)
			return nil
		}
		freq := clampFreq(mhz*1e6, v.freqMin(), v.freqMax())
		return setSdrTuneCmd(v.m.c, v.m.sel, freq)
	case "backspace":
		if len(v.entryBuf) > 0 {
			v.entryBuf = v.entryBuf[:len(v.entryBuf)-1]
		}
		return nil
	default:
		for _, r := range msg.Runes {
			if (r >= '0' && r <= '9') || (r == '.' && !strings.Contains(v.entryBuf, ".")) {
				v.entryBuf += string(r)
			}
		}
		return nil
	}
}

// handleKey drives the tuning/control keys when not in direct entry.
func (v *sdrView) handleKey(msg tea.KeyMsg) tea.Cmd {
	cl := v.live()
	switch msg.String() {
	case "esc":
		v.m.pop()
		return disableSpectrumCmd(v.m.c, v.m.sel)
	case "left":
		return v.tune(cl, -1)
	case "right":
		return v.tune(cl, +1)
	case "s":
		v.stepIdx = (v.stepIdx + 1) % len(sdrSteps)
		return nil
	case "f":
		v.entering, v.entryBuf = true, ""
		return nil
	case "g":
		// Toggle AGC. Leaving auto snaps to the current manual table value.
		if cl != nil && cl.sdrGainAuto {
			return setSdrGainCmd(v.m.c, v.m.sel, false, v.manualGain())
		}
		return setSdrGainCmd(v.m.c, v.m.sel, true, 0)
	case "[":
		return v.stepGain(-1)
	case "]":
		return v.stepGain(+1)
	case "m":
		next := nextDemod(v.demod(cl))
		return configureSdrCmd(v.m.c, v.m.sel, next, v.squelch, v.ppm)
	case ",":
		v.squelch -= 1
		return configureSdrCmd(v.m.c, v.m.sel, v.demod(cl), v.squelch, v.ppm)
	case ".":
		v.squelch += 1
		return configureSdrCmd(v.m.c, v.m.sel, v.demod(cl), v.squelch, v.ppm)
	case "\\":
		// Toggle squelch off/on (sentinel <-> last threshold).
		if v.squelch <= squelchDisabled {
			v.squelch = -30
		} else {
			v.squelch = squelchDisabled
		}
		return configureSdrCmd(v.m.c, v.m.sel, v.demod(cl), v.squelch, v.ppm)
	case "-":
		v.ppm--
		return configureSdrCmd(v.m.c, v.m.sel, v.demod(cl), v.squelch, v.ppm)
	case "+", "=":
		v.ppm++
		return configureSdrCmd(v.m.c, v.m.sel, v.demod(cl), v.squelch, v.ppm)
	}
	return nil
}

// tune moves the demod frequency by ±(current step), clamped to the tuner range.
func (v *sdrView) tune(cl *chanLive, dir int) tea.Cmd {
	target := clampFreq(v.baseFreq(cl)+float64(dir)*sdrSteps[v.stepIdx], v.freqMin(), v.freqMax())
	return setSdrTuneCmd(v.m.c, v.m.sel, target)
}

// stepGain moves the manual-gain cursor through the tuner's table and applies it.
func (v *sdrView) stepGain(dir int) tea.Cmd {
	gains := v.caps.GetGainsDb()
	if len(gains) == 0 {
		v.m.toast = ui.NewToast("No gain table yet — try again once the tuner reports caps", ui.SeverityWarn)
		return nil
	}
	if v.gainIdx < 0 {
		v.gainIdx = len(gains) / 2
	}
	v.gainIdx = cycleIdx(v.gainIdx, dir, len(gains))
	return setSdrGainCmd(v.m.c, v.m.sel, false, gains[v.gainIdx])
}

// manualGain is the currently-seated manual gain value (table[gainIdx]), or 0
// when no gain table is known.
func (v *sdrView) manualGain() float32 {
	gains := v.caps.GetGainsDb()
	if len(gains) == 0 {
		return 0
	}
	if v.gainIdx < 0 || v.gainIdx >= len(gains) {
		return gains[len(gains)/2]
	}
	return gains[v.gainIdx]
}

func (v *sdrView) demod(cl *chanLive) pb.DemodMode {
	if cl != nil {
		return cl.sdrDemod
	}
	return pb.DemodMode_DEMOD_NBFM
}

func (v *sdrView) freqMin() float64 { return v.caps.GetFreqMinHz() }
func (v *sdrView) freqMax() float64 { return v.caps.GetFreqMaxHz() }

func (v *sdrView) Render(w, h int) string {
	cl := v.live()
	v.sync(cl)
	var b strings.Builder

	// Large RF readout.
	freq := v.baseFreq(cl)
	readout := fmt.Sprintf("%.6f MHz", freq/1e6)
	if !(cl != nil && cl.haveSdr) {
		readout += "  (default)"
	}
	b.WriteString(ui.Title.Render("RF  ") + ui.Accent.Bold(true).Render(readout))
	if cl != nil && cl.haveSdr {
		b.WriteString(ui.Dim.Render(fmt.Sprintf("   center %.4f MHz · offset %+.1f kHz",
			cl.sdrCenterHz/1e6, cl.sdrOffsetHz/1000)))
	}
	b.WriteString("\n\n")

	// RF waterfall with the demod-channel cursor overlay.
	wfRows := h / 3
	if wfRows < 3 {
		wfRows = 3
	}
	if wfRows > 10 {
		wfRows = 10
	}
	cursor := v.rxWf.cursorColumn(w, freq)
	b.WriteString(v.rxWf.renderCursor(w, wfRows, cursor) + "\n")
	b.WriteString(v.rxWf.axis(w) + "\n\n")

	// Control bar.
	b.WriteString(v.controlBar(cl, w))

	// Direct-entry prompt overrides the hint line while active.
	if v.entering {
		b.WriteString("\n\n" + ui.Title.Render("Tune to (MHz): ") + ui.Accent.Render(v.entryBuf+"_"))
	}
	return b.String()
}

// controlBar renders the gain/step/demod/ppm/squelch line and the signal meter.
func (v *sdrView) controlBar(cl *chanLive, w int) string {
	gain := "auto"
	if cl != nil && cl.haveSdr && !cl.sdrGainAuto {
		gain = fmt.Sprintf("%.1f dB", cl.sdrGainDb)
	} else if cl == nil || !cl.haveSdr {
		gain = fmt.Sprintf("%.1f dB", v.manualGain())
	}

	sql := fmt.Sprintf("%.0f dBFS", v.squelch)
	if v.squelch <= squelchDisabled {
		sql = "off"
	}

	var b strings.Builder
	kv := func(k, val string) string { return ui.Title.Render(k+" ") + ui.Body.Render(val) }
	b.WriteString(strings.Join([]string{
		kv("step", stepLabel(sdrSteps[v.stepIdx])),
		kv("gain", gain),
		kv("demod", demodLabel(v.demod(cl))),
		kv("ppm", fmt.Sprintf("%d", v.ppm)),
		kv("sql", sql+" "+v.squelchState(cl)),
	}, ui.Dim.Render("  ·  ")))
	b.WriteString("\n")
	b.WriteString(ui.Title.Render("sig  ") + v.signalMeter(cl, minInt(w-6, 40)))
	return b.String()
}

// squelchState shows whether the channel is currently passing audio, using the
// live RX level against the threshold as an approximation of the daemon's gate.
func (v *sdrView) squelchState(cl *chanLive) string {
	if v.squelch <= squelchDisabled {
		return ui.Dim.Render("(open)")
	}
	if cl != nil && cl.rxDbfs >= v.squelch {
		return ui.Accent.Render("▣ open")
	}
	return ui.Dim.Render("▢ closed")
}

// signalMeter draws a horizontal bar for the channel's RX level (dBFS), mapped
// over a -80..0 dBFS span.
func (v *sdrView) signalMeter(cl *chanLive, width int) string {
	if width < 4 {
		width = 4
	}
	db := float32(-80)
	if cl != nil {
		db = cl.rxDbfs
	}
	frac := (db + 80) / 80
	if frac < 0 {
		frac = 0
	}
	if frac > 1 {
		frac = 1
	}
	fill := int(frac * float32(width))
	bar := strings.Repeat("█", fill) + strings.Repeat("░", width-fill)
	return ui.Accent.Render(bar) + ui.Dim.Render(fmt.Sprintf(" %.0f dBFS", db))
}

func minInt(a, b int) int {
	if a < b {
		return a
	}
	return b
}

func (v *sdrView) Title() string {
	dev := "—"
	if cl := v.live(); cl != nil {
		dev = orDash(cl.deviceID)
	}
	return fmt.Sprintf("SDR Tune CH%d · %s", v.m.sel, dev)
}

func (v *sdrView) Hints() []ui.Hint {
	if v.entering {
		return []ui.Hint{
			{Key: "0-9/.", Action: "enter MHz"}, {Key: "enter", Action: "tune"}, {Key: "esc", Action: "cancel"},
		}
	}
	return []ui.Hint{
		{Key: "←/→", Action: "tune"}, {Key: "s", Action: "step"}, {Key: "f", Action: "freq"},
		{Key: "g/[/]", Action: "gain"}, {Key: "m", Action: "demod"},
		{Key: ",/.", Action: "squelch"}, {Key: "\\", Action: "sql off"}, {Key: "-/+", Action: "ppm"},
		{Key: "esc", Action: "back"},
	}
}
