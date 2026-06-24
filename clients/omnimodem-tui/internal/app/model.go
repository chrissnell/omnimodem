package app

import (
	"context"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

// chanLive is the per-channel live state, fed by the event stream.
type chanLive struct {
	name      string
	mode      string
	deviceID  string
	running   bool
	rxDbfs    float32
	txDbfs    float32
	pttKeyed  bool
	clockSync bool
	clockOff  float64
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
}

func New(c client.ModemClient, addr string) *Model {
	return &Model{c: c, addr: addr, version: "dev", live: map[uint32]*chanLive{}}
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
		case "esc":
			// Let the active view tear down (e.g. operate halts TX / stops the
			// spectrum), then pop. Only "esc" is global "back" — "q" is left for
			// views (text fields need it; Channels maps it to quit).
			if len(m.stack) > 1 {
				v := m.top()
				nv, cmd := v.Update(msg)
				m.stack[len(m.stack)-1] = nv
				m.pop()
				return m, cmd
			}
		}
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
	if v := m.top(); v != nil {
		nv, cmd := v.Update(msg)
		m.stack[len(m.stack)-1] = nv
		return m, cmd
	}
	return m, nil
}

func (m *Model) View() string {
	if !m.connected || len(m.stack) == 0 {
		return "Connecting to " + m.addr + " …"
	}
	v := m.top()
	header := ui.Header(m.connected, m.addr, m.version, m.width)
	footer := ui.Footer(v.Hints(), m.width)
	bodyH := m.height - lipgloss.Height(header) - lipgloss.Height(footer)
	if bodyH < 3 {
		bodyH = 3
	}
	body := ui.Frame(v.Title(), v.Render(m.width-4, bodyH-2), true, m.width, bodyH)
	out := lipgloss.JoinVertical(lipgloss.Left, header, body, footer)
	if m.toast != nil {
		out += "\n" + m.toast.Line()
	}
	return out
}
