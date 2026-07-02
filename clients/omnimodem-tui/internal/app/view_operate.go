package app

import (
	"fmt"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

type transcriptLine struct {
	t   time.Time
	dir rune // '›' TX, '‹' RX
	txt string
}

// operateView is the per-channel operate screen. Its surface depends on the
// mode's shape: ragchew (transcript + compose + macros + waterfall) for chat
// modes, the auto-sequence ladder for sequencer modes (FT8/FT4/JT65/JT9), or a
// receive-only spot monitor for beacon modes (WSPR).
type operateView struct {
	m          *Model
	compose    string
	transcript []transcriptLine
	tx         txState
	rxWf       waterfall
	txWf       waterfall
	rxOpen     bool // the last transcript line is an in-progress received line
	draining   bool // a TX-waterfall scroll-off animation is in flight
	myCall     string
	myGrid     string
	theirCall  string
	rst        string
	seq        *ft8Seq
	beacon     bool    // receive-only beacon monitor (WSPR): no ladder, no compose
	modeLabel  string  // active mode label, for the surface header
	slotSecs   float64 // T/R slot length for sequencer/beacon modes
	qlog       qsoLog
}

func newOperateView(m *Model) *operateView {
	v := &operateView{
		m:      m,
		myCall: m.myCall,
		myGrid: m.myGrid,
		rst:    "599",
	}
	if cl := m.live[m.sel]; cl != nil {
		v.modeLabel = cl.mode
		if mi := modeByLabel(cl.mode); mi != nil {
			v.slotSecs = mi.slotSecs
			switch mi.shape {
			case "sequencer":
				v.seq = newFT8Seq(v.myCall, v.myGrid)
			case "beacon":
				v.beacon = true
			}
		}
	}
	// Size the TX watchdog to the mode's slot length now that it's known: windowed
	// modes wait for the daemon's slot-align count-off before keying, so a fixed
	// timeout would abort long-slot modes before they ever transmit.
	v.tx = txState{watchdog: txWatchdog(v.slotSecs)}
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
		if rf := msg.ev.GetRxFrame(); rf != nil && rf.GetChannel() == v.m.sel {
			v.appendRx(string(rf.GetData()))
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
			if v.beacon {
				return v, nil // beacon TX is scheduled, not keyed from here
			}
			if v.seq != nil {
				return v, v.ft8Send()
			}
			return v, v.sendCompose()
		case "f1", "f2", "f3", "f4", "f5":
			if v.beacon || v.seq != nil {
				return v, nil // no free-text macros on the ladder/beacon surfaces
			}
			v.compose = expandMacro(macroForKey(msg.String()), macroCtx{
				myCall: v.myCall, theirCall: v.theirCall, rst: v.rst,
			})
			return v, nil
		case "backspace":
			if n := len(v.compose); n > 0 {
				v.compose = v.compose[:n-1]
			}
		default:
			if v.seq == nil && !v.beacon && len(msg.Runes) > 0 {
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
	v.rxOpen = false // a TX line closes any in-progress received line
	v.tx.begin([]byte(msg))
	return acquireLeaseCmd(v.m.c, v.m.sel)
}

// appendRx folds streaming decoded text into the transcript. Modes like PSK31
// decode roughly a character at a time, so received text is accumulated onto a
// single in-progress received line and broken into a new line at each newline.
func (v *operateView) appendRx(s string) {
	for _, r := range s {
		if r == '\n' || r == '\r' {
			v.rxOpen = false
			continue
		}
		if !v.rxOpen {
			v.transcript = append(v.transcript, transcriptLine{t: time.Now(), dir: '‹', txt: ""})
			v.rxOpen = true
		}
		last := &v.transcript[len(v.transcript)-1]
		last.txt += string(r)
	}
}

func (v *operateView) sendCompose() tea.Cmd {
	line := strings.TrimSpace(v.compose)
	if line == "" || v.tx.active() {
		return nil
	}
	v.transcript = append(v.transcript, transcriptLine{t: time.Now(), dir: '›', txt: line})
	v.rxOpen = false // a TX line closes any in-progress received line
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
	// Every cell carries the black panel background (label padding, the gap, and
	// the waterfall rows) so no grey leaks through between the colored spans.
	head := ui.Title.Background(ui.ColorPanel).Width(col)
	bodyH := wfRows + 1 // waterfall rows + axis line
	// msgPane centers a message both ways in a pane's body area, on black.
	msgPane := func(msg string) string {
		text := lipgloss.NewStyle().Foreground(ui.ColorAccent).Background(ui.ColorPanel).
			Width(col).Align(lipgloss.Center).Render(msg)
		return lipgloss.Place(col, bodyH, lipgloss.Center, lipgloss.Center, text,
			lipgloss.WithWhitespaceBackground(ui.ColorPanel))
	}
	column := func(label, override string, wf *waterfall) string {
		if override != "" {
			return head.Render(label) + "\n" + msgPane(override)
		}
		return head.Render(label) + "\n" + wf.render(col, wfRows) + "\n" + wf.axis(col)
	}
	rxMsg := ""
	if v.tx.active() {
		rxMsg = "RX channel muted during TX" // the rig can't receive while keyed
	}
	txMsg := ""
	if len(v.txWf.rows) == 0 {
		txMsg = "waterfall idle" // nothing transmitted (or it has scrolled off)
	}
	gapBlock := lipgloss.NewStyle().Background(ui.ColorPanel).Width(gap).Height(wfRows + 2).Render("")
	b.WriteString(lipgloss.JoinHorizontal(
		lipgloss.Top,
		column("RX", rxMsg, &v.rxWf),
		gapBlock,
		column("TX", txMsg, &v.txWf),
	) + "\n\n")

	if v.seq != nil {
		b.WriteString(fmt.Sprintf("%s · slot %.1f/%gs · DX [%s %s]\n\n",
			strings.ToUpper(v.modeLabel), slotPosition(time.Now(), v.slotSecs), v.slotSecs,
			orDash(v.seq.dxCall), v.seq.dxGrid))
		b.WriteString("next: " + v.seq.current() + "\n")
		b.WriteString("cq:   " + v.seq.cq() + "\n\n")
		b.WriteString(fmt.Sprintf("logged QSOs: %d", len(v.qlog.entries)))
		return b.String()
	}
	if v.beacon {
		// Receive-only spot monitor: show decoded spots; no compose/ladder.
		for _, l := range v.transcript {
			b.WriteString(fmt.Sprintf("%s %c %s\n", l.t.Format("15:04"), l.dir, l.txt))
		}
		b.WriteString(fmt.Sprintf("%s beacon · slot %.0f/%gs · spots: %d",
			strings.ToUpper(v.modeLabel), slotPosition(time.Now(), v.slotSecs), v.slotSecs, len(v.transcript)))
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
	if v.beacon {
		return []ui.Hint{{Key: "esc", Action: "back"}}
	}
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
