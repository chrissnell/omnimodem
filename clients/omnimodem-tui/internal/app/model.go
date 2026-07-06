package app

import (
	"context"
	"fmt"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/config"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

// chanLive is the per-channel live state, fed by the event stream.
type chanLive struct {
	name         string
	mode         string
	deviceID     string // RX (capture) device
	txDeviceID   string // TX (playback) device; "" == same as RX
	pttDeviceID  string // PTT device; "" when deviceless or unset
	pttMethod    pb.PttMethod
	pttTxDelayMs uint32 // per-channel PTT keying lead-in
	pttTxTailMs  uint32 // per-channel PTT keying tail/hold
	running      bool
	rxDbfs       float32
	txDbfs       float32
	pttKeyed     bool
	clockSync    bool
	clockOff     float64
	rsidTx       bool   // prepend the mode's RSID burst before each TX
	rsidRx       bool   // run the RSID detector over received audio
	lastRsid     string // most recently identified RSID (tag @ freq), "" if none
}

// Model is the root window manager: it owns the client, the event stream, shared
// live state, terminal size, a stack of Views, and a transient toast. It renders
// chrome (header/footer/toast) around the active view.
type Model struct {
	c         client.ModemClient
	addr      string
	version   string
	width     int
	height    int
	live      map[uint32]*chanLive
	sel       uint32
	events    <-chan *pb.Event
	cancel    context.CancelFunc
	connected bool
	stack     []View
	toast     *ui.Toast
	// Operator station identity, used by FT8 sequencing and macros. Set on the
	// Configure screen and persisted to the client config file (the daemon has
	// no station-identity field). savedCall/savedGrid track the last values
	// written so persistIdentity only touches disk on a real change.
	myCall    string
	myGrid    string
	savedCall string
	savedGrid string
	// modeParams caches the mode settings last saved from the Configure screen,
	// per channel. The daemon persists them but doesn't report them back in the
	// snapshot (ChannelInfo carries only the mode label — see GRA-281), so without
	// this cache reopening Configure would show mode defaults instead of the values
	// just saved. Keyed by channel; only trusted when the cached label matches the
	// channel's current mode.
	modeParams map[uint32]savedModeParams
}

// savedModeParams is the last-persisted settings for one channel's mode.
type savedModeParams struct {
	label string
	vals  map[string]float64
}

func New(c client.ModemClient, addr string) *Model {
	id := config.Load()
	return &Model{
		c: c, addr: addr, version: "dev", live: map[uint32]*chanLive{},
		myCall: id.Call, myGrid: id.Grid,
		savedCall: id.Call, savedGrid: id.Grid,
		modeParams: map[uint32]savedModeParams{},
	}
}

// persistIdentity writes the operator call/grid to the client config file when
// they've changed since the last save. Best-effort: a write failure surfaces as
// a toast but never blocks the UI, and the values stay live for the session.
func (m *Model) persistIdentity() {
	if m.myCall == m.savedCall && m.myGrid == m.savedGrid {
		return
	}
	if err := config.Save(config.Identity{Call: m.myCall, Grid: m.myGrid}); err != nil {
		m.toast = ui.NewToast("could not save station identity: "+err.Error(), ui.SeverityWarn)
		return
	}
	m.savedCall, m.savedGrid = m.myCall, m.myGrid
}

func (m *Model) Init() tea.Cmd { return connectCmd(m.c) }

// applyEvent folds one event into live state. LOSSY events overwrite (latest
// wins); the snapshot rebuilds the channel map.
func (m *Model) applyEvent(ev *pb.Event) {
	ensure := func(ch uint32) *chanLive {
		cl := m.live[ch]
		if cl == nil {
			cl = &chanLive{}
			m.live[ch] = cl
		}
		return cl
	}
	switch k := ev.Kind.(type) {
	case *pb.Event_Snapshot:
		m.live = map[uint32]*chanLive{}
		for _, ci := range k.Snapshot.GetChannels() {
			m.live[ci.GetChannel()] = &chanLive{
				name: ci.GetName(), mode: ci.GetMode(),
				deviceID: ci.GetDeviceId(), running: ci.GetRunning(),
				txDeviceID: ci.GetTxDeviceId(), pttDeviceID: ci.GetPttDeviceId(),
				pttMethod:    ci.GetPttMethod(),
				pttTxDelayMs: ci.GetPttTxDelayMs(),
				pttTxTailMs:  ci.GetPttTxTailMs(),
				rsidTx:       ci.GetRsidTx(),
				rsidRx:       ci.GetRsidRx(),
			}
		}
	case *pb.Event_AudioLevel:
		ensure(k.AudioLevel.GetChannel()).rxDbfs = k.AudioLevel.GetDbfs()
	case *pb.Event_PttState:
		ensure(k.PttState.GetChannel()).pttKeyed = k.PttState.GetKeyed()
	case *pb.Event_ClockOffset:
		for _, cl := range m.live {
			cl.clockSync = k.ClockOffset.GetSynchronized()
			cl.clockOff = k.ClockOffset.GetOffsetS()
		}
	case *pb.Event_ChannelConfigured:
		ensure(k.ChannelConfigured.GetChannel())
	case *pb.Event_RsidDetected:
		d := k.RsidDetected
		label := d.GetTag()
		if d.GetMode() != "" {
			label = d.GetMode()
		}
		summary := fmt.Sprintf("%s @ %.0f Hz", label, d.GetFreqHz())
		ensure(d.GetChannel()).lastRsid = summary
		m.toast = ui.NewToast("RSID: "+summary, ui.SeverityInfo)
	}
}

func (m *Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		return m, nil
	case tea.KeyMsg:
		switch msg.String() {
		case "ctrl+c":
			if m.cancel != nil {
				m.cancel()
			}
			return m, tea.Quit
		}
		// "esc" is not special-cased here: each view owns its own back/cancel
		// behavior and calls m.pop() itself (so a view with an open inner modal
		// can swallow esc to close it instead of leaving the screen).
		return m.routeToView(msg)
	case connectedMsg:
		m.connected = true
		m.events = msg.events
		m.cancel = msg.cancel
		m.stack = []View{newChannelsView(m)}
		return m, tea.Batch(snapshotCmd(m.c), waitForEvent(m.events), tickCmd())
	case eventMsg:
		m.applyEvent(msg.ev)
		_, cmd := m.routeToView(msg) // let the active view react too
		return m, tea.Batch(cmd, waitForEvent(m.events))
	case eventClosedMsg:
		m.connected = false
		m.toast = ui.NewToast("event stream closed", ui.SeverityError)
		return m, nil
	case snapshotMsg:
		m.applyEvent(&pb.Event{Kind: &pb.Event_Snapshot{Snapshot: msg.state}})
		return m, nil
	case rpcErrMsg:
		m.toast = ui.NewToast(msg.err.Error(), ui.SeverityError)
		return m, nil
	case tickMsg:
		if m.toast != nil && m.toast.Expired() {
			m.toast = nil
		}
		_, cmd := m.routeToView(msg)
		return m, tea.Batch(cmd, tickCmd())
	}
	return m.routeToView(msg)
}

func (m *Model) routeToView(msg tea.Msg) (tea.Model, tea.Cmd) {
	idx := len(m.stack) - 1
	if idx < 0 {
		return m, nil
	}
	v := m.stack[idx]
	nv, cmd := v.Update(msg)
	// Write the updated view back ONLY if Update didn't change the stack itself.
	// If Update pushed (top grew) or popped (top shrank), the new top is already
	// correct; writing to len-1 here would clobber the pushed view or reinstall a
	// popped one. (Short-circuit also keeps this panic-safe after a pop.)
	if idx == len(m.stack)-1 && m.stack[idx] == v {
		m.stack[idx] = nv
	}
	return m, cmd
}

func (m *Model) View() string {
	if !m.connected || len(m.stack) == 0 {
		return "Connecting to " + m.addr + " …"
	}
	v := m.top()
	header := ui.Header(m.connected, m.addr, m.version, m.width)
	footer := ui.Footer(v.Hints(), m.width)
	// The toast is drawn as an extra line below the footer, so it eats into the
	// body's height budget — otherwise a full-height view (e.g. the picture
	// picker) plus a live toast renders taller than the terminal and scrolls the
	// top off. Reserve its rows (the line plus the "\n" separator) up front.
	var toastLine string
	toastH := 0
	if m.toast != nil {
		toastLine = m.toast.Line()
		toastH = lipgloss.Height(toastLine) + 1
	}
	bodyH := m.height - lipgloss.Height(header) - lipgloss.Height(footer) - toastH
	if bodyH < 3 {
		bodyH = 3
	}
	body := ui.Frame(v.Title(), v.Render(m.width-4, bodyH-2), true, m.width, bodyH)
	out := lipgloss.JoinVertical(lipgloss.Left, header, body, footer)
	if toastH > 0 {
		out += "\n" + toastLine
	}
	return out
}
