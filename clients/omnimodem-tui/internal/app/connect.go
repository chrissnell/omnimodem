package app

import (
	"context"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

type connectedMsg struct{ events <-chan *pb.Event }

// connectCmd opens the event stream (the act of subscribing also proves the
// daemon is reachable; the first event is the snapshot).
func connectCmd(c client.ModemClient) tea.Cmd {
	return func() tea.Msg {
		ch := startEventStream(context.Background(), c)
		return connectedMsg{events: ch}
	}
}
