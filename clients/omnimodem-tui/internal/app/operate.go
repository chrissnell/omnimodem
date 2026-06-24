package app

import (
	"fmt"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
)

type transcriptLine struct {
	t   time.Time
	dir rune // '›' TX, '‹' RX
	txt string
}

// operateState holds the operate-screen working state (ragchew surface; the FT8
// sequencer + QSO log attach in Phase 4).
type operateState struct {
	compose    string
	transcript []transcriptLine
	tx         txState
	wf         waterfall
	myCall     string
	myGrid     string
	theirCall  string
	rst        string
	seq        *ft8Seq // non-nil ⇒ FT8 structured surface
	qlog       qsoLog
}

func (m *Model) enterOperate() {
	m.screen = screenOperate
	op := &operateState{
		myCall: "NW5W",
		myGrid: "EM10",
		rst:    "599",
		tx:     txState{watchdog: 30 * time.Second},
	}
	// FT8 (and other structured modes) get the auto-sequence surface.
	if cl := m.live[m.sel]; cl != nil {
		if mi := modeByLabel(cl.mode); mi != nil && mi.shape == "ft8" {
			op.seq = newFT8Seq(op.myCall, op.myGrid)
		}
	}
	m.op = op
}

// ft8Send transmits the next message and starts the TX FSM. With no DX worked
// yet it (re)sends CQ without advancing — CQ is not a ladder step. With a DX
// target it sends the current ladder message, logs the QSO exactly once as it
// sends RR73, then advances.
func (m *Model) ft8Send() tea.Cmd {
	if m.op.tx.active() {
		return nil
	}
	seq := m.op.seq
	var msg string
	if seq.dxCall == "" {
		msg = seq.cq() // calling CQ: not a ladder step, do not advance
	} else {
		msg = seq.current()
		if seq.step == ladderRR73Step { // RR73 → QSO complete; log once
			m.op.qlog.add(seq.dxCall, seq.dxGrid, m.op.rst)
		}
		seq.advance()
	}
	m.op.transcript = append(m.op.transcript, transcriptLine{t: time.Now(), dir: '›', txt: msg})
	m.op.tx.begin([]byte(msg))
	return acquireLeaseCmd(m.c, m.sel)
}

func (m *Model) updateOperate(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case eventMsg:
		if sf := msg.ev.GetSpectrumFrame(); sf != nil {
			m.op.wf.push(sf)
		}
		if tc := msg.ev.GetTransmitComplete(); tc != nil && m.op.tx.active() {
			m.op.tx.onComplete()
			m.applyEvent(msg.ev)
			return m, tea.Batch(releaseLeaseCmd(m.c, m.sel), waitForEvent(m.events))
		}
		m.applyEvent(msg.ev)
		return m, waitForEvent(m.events)
	case spectrumCfgMsg:
		// Seed the waterfall axis from the daemon's clamped params, before the
		// first SpectrumFrame arrives.
		m.op.wf.enabled = true
		m.op.wf.freqStart = msg.resp.GetFreqStartHz()
		m.op.wf.freqStep = msg.resp.GetFreqStepHz()
		return m, nil
	case leaseMsg:
		if msg.resp.GetGranted() {
			m.op.tx.onLeaseGranted()
			return m, transmitCmd(m.c, m.sel, m.op.tx.payload)
		}
		m.op.tx.halt()
		m.err = fmt.Sprintf("TX lease held by ch%d", msg.resp.GetHeldBy())
		return m, nil
	case transmitMsg:
		m.op.tx.id = msg.id
		return m, nil
	case tickMsg:
		if m.op.tx.watchdogExpired(time.Time(msg)) {
			m.op.tx.halt()
			m.err = "TX watchdog: aborted"
			return m, tea.Batch(releaseLeaseCmd(m.c, m.sel), tickCmd())
		}
		return m, tickCmd()
	case tea.KeyMsg:
		switch msg.String() {
		case "esc":
			if m.op.tx.active() {
				m.op.tx.halt()
				return m, releaseLeaseCmd(m.c, m.sel)
			}
			m.screen = screenDashboard
			return m, disableSpectrumCmd(m.c, m.sel)
		case "enter":
			if m.op.seq != nil {
				return m, m.ft8Send()
			}
			return m, m.sendCompose()
		case "f1", "f2", "f3", "f4", "f5":
			m.op.compose = expandMacro(macroForKey(msg.String()), macroCtx{
				myCall: m.op.myCall, theirCall: m.op.theirCall, rst: m.op.rst,
			})
			return m, nil
		case "backspace":
			if n := len(m.op.compose); n > 0 {
				m.op.compose = m.op.compose[:n-1]
			}
		default:
			if len(msg.Runes) > 0 {
				m.op.compose += string(msg.Runes)
			}
		}
	}
	return m, nil
}

// sendCompose appends the line to the transcript and starts the TX FSM.
func (m *Model) sendCompose() tea.Cmd {
	line := strings.TrimSpace(m.op.compose)
	if line == "" || m.op.tx.active() {
		return nil
	}
	m.op.transcript = append(m.op.transcript, transcriptLine{t: time.Now(), dir: '›', txt: line})
	m.op.tx.begin([]byte(line))
	m.op.compose = ""
	return acquireLeaseCmd(m.c, m.sel)
}

func (m *Model) viewOperate() string {
	op := m.op
	var b strings.Builder
	if op.seq != nil {
		b.WriteString(fmt.Sprintf("FT8 · slot %.0f/15s · DX [%s %s]\n\n", slotPosition(time.Now()), orNone(op.seq.dxCall), op.seq.dxGrid))
		b.WriteString("QSO sequence (next): " + op.seq.current() + "\n")
		b.WriteString(op.seq.cq() + "\n")
		b.WriteString("\n" + op.wf.line(40) + "\n")
		b.WriteString(fmt.Sprintf("\n[↵] send next   logged QSOs: %d   [Esc] HALT TX", len(op.qlog.entries)))
		return b.String()
	}
	b.WriteString("Activity            │ Transcript\n")
	for _, l := range op.transcript {
		b.WriteString(fmt.Sprintf("                    │ %s %c %s\n", l.t.Format("15:04"), l.dir, l.txt))
	}
	b.WriteString("                    │ " + op.wf.line(40) + "\n")
	b.WriteString("\n› " + op.compose)
	if op.tx.active() {
		b.WriteString("    [TX ACTIVE]")
	}
	b.WriteString("\n" + macroBar())
	return b.String()
}
