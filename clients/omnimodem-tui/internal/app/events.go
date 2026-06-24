package app

import (
	"context"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

// startEventStream opens SubscribeEvents and pumps events into a buffered
// channel from a goroutine; the bridge between gRPC streaming and Bubble Tea's
// single-threaded Update loop. Returns the channel to feed waitForEvent.
func startEventStream(ctx context.Context, c client.ModemClient) <-chan *pb.Event {
	out := make(chan *pb.Event, 256)
	go func() {
		defer close(out)
		stream, err := c.Subscribe(ctx)
		if err != nil {
			return
		}
		for {
			ev, err := stream.Recv()
			if err != nil {
				return
			}
			select {
			case out <- ev:
			case <-ctx.Done():
				return
			}
		}
	}()
	return out
}

// waitForEvent blocks on the next event and wraps it as a tea.Msg. Re-issued
// from Update after each eventMsg so the stream keeps draining.
func waitForEvent(ch <-chan *pb.Event) tea.Cmd {
	return func() tea.Msg {
		ev, ok := <-ch
		if !ok {
			return eventClosedMsg{}
		}
		return eventMsg{ev}
	}
}
