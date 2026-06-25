package app

import (
	"context"
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

func rpcCtx() (context.Context, context.CancelFunc) {
	return context.WithTimeout(context.Background(), 5*time.Second)
}

// --- typed messages returned by commands / the event bridge ---
type snapshotMsg struct{ state *pb.ModemState }
type devicesMsg struct{ devices []*pb.DeviceInfo }
type rpcOKMsg struct{ what string } // generic "mutating RPC succeeded"
type rpcErrMsg struct{ err error }  // any RPC failure
type channelBoundMsg struct{} // ConfigureChannel succeeded → chain audio
type pttBoundMsg struct{}     // ConfigurePtt succeeded → bind complete
type audioCfgMsg struct{ resp *pb.ConfigureAudioResponse }
type spectrumCfgMsg struct{ resp *pb.ConfigureSpectrumResponse }
type leaseMsg struct{ resp *pb.TxLeaseResponse }
type transmitMsg struct{ id uint64 }
type eventMsg struct{ ev *pb.Event }
type eventClosedMsg struct{ err error }
type tickMsg time.Time
type txDrainMsg struct{}

func snapshotCmd(c client.ModemClient) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		st, err := c.GetState(ctx)
		if err != nil {
			return rpcErrMsg{err}
		}
		return snapshotMsg{st}
	}
}

func devicesCmd(c client.ModemClient) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		d, err := c.ListDevices(ctx)
		if err != nil {
			return rpcErrMsg{err}
		}
		return devicesMsg{d}
	}
}

// tickCmd drives the FT8 slot clock and the TX watchdog at 4 Hz.
func tickCmd() tea.Cmd {
	return tea.Tick(250*time.Millisecond, func(t time.Time) tea.Msg { return tickMsg(t) })
}

// txDrainCmd paces the TX-waterfall scroll-off between transmissions.
func txDrainCmd() tea.Cmd {
	return tea.Tick(80*time.Millisecond, func(time.Time) tea.Msg { return txDrainMsg{} })
}
