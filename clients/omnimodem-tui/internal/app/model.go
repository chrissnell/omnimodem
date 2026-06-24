package app

import (
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

type screen int

const (
	screenConnect screen = iota
	screenDashboard
	screenConfig
	screenOperate
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

// Model is the Elm root. Screens read/write the shared fields; only Update mutates.
type Model struct {
	c         client.ModemClient
	addr      string
	screen    screen
	width     int
	height    int
	err       string
	live      map[uint32]*chanLive
	sel       uint32 // selected channel
	events    <-chan *pb.Event
	connected bool

	// sub-screen state, attached on entry to that screen.
	cfg *configState
	op  *operateState
}

func New(c client.ModemClient, addr string) *Model {
	return &Model{c: c, addr: addr, screen: screenConnect, live: map[uint32]*chanLive{}}
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
		if msg.String() == "ctrl+c" {
			return m, tea.Quit
		}
		return m.updateScreen(msg)
	case connectedMsg:
		m.connected = true
		m.screen = screenDashboard
		m.events = msg.events
		return m, tea.Batch(snapshotCmd(m.c), waitForEvent(m.events), tickCmd())
	case eventMsg:
		// Operate screen consumes some events (spectrum, tx-complete) before the
		// central fold; give it first refusal while on that screen.
		if m.screen == screenOperate {
			return m.updateOperate(msg)
		}
		m.applyEvent(msg.ev)
		return m, waitForEvent(m.events)
	case eventClosedMsg:
		m.connected = false
		m.err = "event stream closed"
		return m, nil
	case snapshotMsg:
		m.applyEvent(&pb.Event{Kind: &pb.Event_Snapshot{Snapshot: msg.state}})
		return m, nil
	case rpcErrMsg:
		m.err = msg.err.Error()
		return m, nil
	case tickMsg:
		return m.updateScreen(msg)
	}
	return m.updateScreen(msg)
}

// updateScreen dispatches to the active screen's handler.
func (m *Model) updateScreen(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch m.screen {
	case screenConfig:
		return m.updateConfig(msg)
	case screenOperate:
		return m.updateOperate(msg)
	case screenDashboard:
		return m.updateDashboard(msg)
	}
	return m, nil
}

func (m *Model) View() string {
	body := ""
	switch m.screen {
	case screenConnect:
		body = "Connecting to " + m.addr + " …"
	case screenDashboard:
		body = m.viewDashboard()
	case screenConfig:
		body = m.viewConfig()
	case screenOperate:
		body = m.viewOperate()
	}
	return body + "\n" + m.statusBar()
}
