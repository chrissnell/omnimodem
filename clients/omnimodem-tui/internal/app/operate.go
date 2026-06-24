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
	theirCall  string
	rst        string
}

func (m *Model) enterOperate() {
	m.screen = screenOperate
	m.op = &operateState{
		myCall: "NW5W",
		rst:    "599",
		tx:     txState{watchdog: 30 * time.Second},
	}
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
