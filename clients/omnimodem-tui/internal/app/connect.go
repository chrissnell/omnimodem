package app

import (
	"context"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

type connectedMsg struct {
	events <-chan *pb.Event
	cancel context.CancelFunc
}

// connectCmd opens the event stream (the act of subscribing also proves the
// daemon is reachable; the first event is the snapshot). The returned cancel
// func tears the stream goroutine down on quit.
func connectCmd(c client.ModemClient) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := context.WithCancel(context.Background())
		ch := startEventStream(ctx, c)
		return connectedMsg{events: ch, cancel: cancel}
	}
}
