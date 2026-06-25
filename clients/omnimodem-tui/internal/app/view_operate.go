package app

import (
	"fmt"
	"strings"
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

type transcriptLine struct {
	t   time.Time
	dir rune // '›' TX, '‹' RX
	txt string
}

// operateView is the per-channel operate screen: ragchew (transcript + compose +
// macros + waterfall) or, for FT8-shape modes, the auto-sequence ladder.
type operateView struct {
	m          *Model
	compose    string
	transcript []transcriptLine
	tx         txState
	rxWf       waterfall
	txWf       waterfall
	draining   bool // a TX-waterfall scroll-off animation is in flight
	myCall     string
	myGrid     string
	theirCall  string
	rst        string
	seq        *ft8Seq
	qlog       qsoLog
}

func newOperateView(m *Model) *operateView {
	v := &operateView{
		m:      m,
		myCall: m.myCall,
		myGrid: m.myGrid,
		rst:    "599",
		tx:     txState{watchdog: 30 * time.Second},
	}
	if cl := m.live[m.sel]; cl != nil {
		if mi := modeByLabel(cl.mode); mi != nil && mi.shape == "ft8" {
			v.seq = newFT8Seq(v.myCall, v.myGrid)
		}
	}
	return v
}

func (v *operateView) Update(msg tea.Msg) (View, tea.Cmd) {
	switch msg := msg.(type) {
	case eventMsg:
		// The window manager has already folded this into live state and will
		// re-issue waitForEvent; here we only react to operate-specific events.
		if sf := msg.ev.GetSpectrumFrame(); sf != nil {
			if sf.GetTransmit() {
				v.txWf.push(sf)
			} else {
				v.rxWf.push(sf)
			}
		}
		if tc := msg.ev.GetTransmitComplete(); tc != nil && v.tx.active() {
			v.tx.onComplete()
			return v, releaseLeaseCmd(v.m.c, v.m.sel)
		}
		return v, nil
	case spectrumCfgMsg:
		// The per-frame events carry the frequency axis; nothing to do here.
		return v, nil
	case leaseMsg:
		if msg.resp.GetGranted() {
			v.tx.onLeaseGranted()
			return v, transmitCmd(v.m.c, v.m.sel, v.tx.payload)
		}
		v.tx.halt()
		v.m.toast = ui.NewToast(fmt.Sprintf("TX lease held by ch%d", msg.resp.GetHeldBy()), ui.SeverityWarn)
		return v, nil
	case transmitMsg:
		v.tx.id = msg.id
		return v, nil
	case tickMsg:
		if v.tx.watchdogExpired(time.Time(msg)) {
			v.tx.halt()
			v.m.toast = ui.NewToast("TX watchdog: aborted", ui.SeverityError)
			return v, releaseLeaseCmd(v.m.c, v.m.sel)
		}
		// Once a transmission ends, scroll its waterfall off to black. The fast
		// drain animation runs only while there's something to clear.
		if !v.draining && !v.tx.active() && v.txWf.hasSignal() {
			v.draining = true
			return v, txDrainCmd()
		}
		return v, nil
	case txDrainMsg:
		if v.tx.active() || !v.txWf.hasSignal() {
			if !v.txWf.hasSignal() {
				v.txWf.rows = nil // fully scrolled off — leave the pane blank
			}
			v.draining = false
			return v, nil
		}
		v.txWf.pushBlank()
		return v, txDrainCmd()
	case tea.KeyMsg:
		switch msg.String() {
		case "esc":
			// Leave operate: halt any TX, stop the spectrum, then pop back.
			v.m.pop()
			if v.tx.active() {
				v.tx.halt()
				return v, tea.Batch(releaseLeaseCmd(v.m.c, v.m.sel), disableSpectrumCmd(v.m.c, v.m.sel))
			}
			return v, disableSpectrumCmd(v.m.c, v.m.sel)
		case "ctrl+x":
			// Halt TX in place, stay on the screen.
			if v.tx.active() {
				v.tx.halt()
				return v, releaseLeaseCmd(v.m.c, v.m.sel)
			}
			return v, nil
		case "enter":
			if v.seq != nil {
				return v, v.ft8Send()
			}
			return v, v.sendCompose()
		case "f1", "f2", "f3", "f4", "f5":
			v.compose = expandMacro(macroForKey(msg.String()), macroCtx{
				myCall: v.myCall, theirCall: v.theirCall, rst: v.rst,
			})
			return v, nil
		case "backspace":
			if n := len(v.compose); n > 0 {
				v.compose = v.compose[:n-1]
			}
		default:
			if v.seq == nil && len(msg.Runes) > 0 {
				v.compose += string(msg.Runes)
			}
		}
	}
	return v, nil
}

// ft8Send transmits the next ladder message; CQ does not advance, RR73 logs once.
func (v *operateView) ft8Send() tea.Cmd {
	if v.tx.active() {
		return nil
	}
	seq := v.seq
	var msg string
	if seq.dxCall == "" {
		msg = seq.cq()
	} else {
		msg = seq.current()
		if seq.step == ladderRR73Step {
			v.qlog.add(seq.dxCall, seq.dxGrid, v.rst)
		}
		seq.advance()
	}
	v.transcript = append(v.transcript, transcriptLine{t: time.Now(), dir: '›', txt: msg})
	v.tx.begin([]byte(msg))
	return acquireLeaseCmd(v.m.c, v.m.sel)
}

func (v *operateView) sendCompose() tea.Cmd {
	line := strings.TrimSpace(v.compose)
	if line == "" || v.tx.active() {
		return nil
	}
	v.transcript = append(v.transcript, transcriptLine{t: time.Now(), dir: '›', txt: line})
	v.tx.begin([]byte(line))
	v.compose = ""
	return acquireLeaseCmd(v.m.c, v.m.sel)
}

func (v *operateView) Render(w, h int) string {
	var b strings.Builder

	// Two waterfalls side by side, fixed at the top: RX (received) on the left,
	// TX (transmitted) on the right.
	wfRows := h / 3
	if wfRows < 3 {
		wfRows = 3
	}
	if wfRows > 8 {
		wfRows = 8
	}
	const gap = 2
	col := (w - gap) / 2
	if col < 8 {
		col = 8
	}
	column := func(label string, wf *waterfall) string {
		return ui.Title.Render(label) + "\n" + wf.render(col, wfRows) + "\n" + wf.axis(col)
	}
	b.WriteString(lipgloss.JoinHorizontal(
		lipgloss.Top,
		column("RX", &v.rxWf),
		strings.Repeat(" ", gap),
		column("TX", &v.txWf),
	) + "\n\n")

	if v.seq != nil {
		b.WriteString(fmt.Sprintf("FT8 · slot %.0f/15s · DX [%s %s]\n\n",
			slotPosition(time.Now()), orDash(v.seq.dxCall), v.seq.dxGrid))
		b.WriteString("next: " + v.seq.current() + "\n")
		b.WriteString("cq:   " + v.seq.cq() + "\n\n")
		b.WriteString(fmt.Sprintf("logged QSOs: %d", len(v.qlog.entries)))
		return b.String()
	}
	for _, l := range v.transcript {
		b.WriteString(fmt.Sprintf("%s %c %s\n", l.t.Format("15:04"), l.dir, l.txt))
	}
	b.WriteString("› " + v.compose)
	if v.tx.active() {
		b.WriteString("   " + ui.Accent.Render("[TX]"))
	}
	return b.String()
}

func (v *operateView) Title() string {
	cl := v.m.live[v.m.sel]
	mode := "—"
	if cl != nil {
		mode = orNone(cl.mode)
	}
	return fmt.Sprintf("Operate ch%d · %s", v.m.sel, mode)
}

func (v *operateView) Hints() []ui.Hint {
	if v.seq != nil {
		return []ui.Hint{
			{Key: "enter", Action: "send next"}, {Key: "ctrl+x", Action: "halt"}, {Key: "esc", Action: "back"},
		}
	}
	return []ui.Hint{
		{Key: "enter", Action: "send"}, {Key: "f1-f5", Action: "macros"},
		{Key: "ctrl+x", Action: "halt"}, {Key: "esc", Action: "back"},
	}
}
