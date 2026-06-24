package app

import (
	"strings"
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

func TestApplyEventLatestWins(t *testing.T) {
	m := New(&client.Fake{}, "/x.sock")
	m.applyEvent(&pb.Event{Kind: &pb.Event_AudioLevel{AudioLevel: &pb.AudioLevel{Channel: 0, Dbfs: -18}}})
	m.applyEvent(&pb.Event{Kind: &pb.Event_AudioLevel{AudioLevel: &pb.AudioLevel{Channel: 0, Dbfs: -12}}})
	if m.live[0].rxDbfs != -12 {
		t.Fatalf("rxDbfs = %v, want -12", m.live[0].rxDbfs)
	}
}

func TestRootRendersHeaderAndActiveView(t *testing.T) {
	m := New(&client.Fake{}, "/tmp/omnimodem/omnimodem.sock")
	m.connected = true
	m.live[0] = &chanLive{name: "vfo-a", mode: "psk31"}
	m.push(newChannelsView(m))
	m.width, m.height = 80, 24
	out := m.View()
	if !strings.Contains(out, "omnimodem") || !strings.Contains(out, "Channels") {
		t.Fatalf("root must render header + active view title:\n%s", out)
	}
}

func TestEscPopsView(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.connected = true
	m.push(newChannelsView(m))
	m.sel = 0
	m.live[0] = &chanLive{}
	m.push(newConfigView(m))
	if len(m.stack) != 2 {
		t.Fatalf("expected 2 views, got %d", len(m.stack))
	}
	m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	if len(m.stack) != 1 {
		t.Fatalf("esc should pop to 1 view, got %d", len(m.stack))
	}
}
