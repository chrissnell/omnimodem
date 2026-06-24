package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

func TestSnapshotCmd(t *testing.T) {
	f := &client.Fake{}
	if _, ok := snapshotCmd(f)().(snapshotMsg); !ok {
		t.Fatalf("want snapshotMsg")
	}
}

func TestWaitForEvent(t *testing.T) {
	ch := make(chan *pb.Event, 1)
	ch <- &pb.Event{Kind: &pb.Event_PttState{PttState: &pb.PttState{Channel: 0, Keyed: true}}}
	if m, ok := waitForEvent(ch)().(eventMsg); !ok || m.ev.GetPttState() == nil {
		t.Fatalf("want eventMsg with PttState")
	}
	close(ch)
	if _, ok := waitForEvent(ch)().(eventClosedMsg); !ok {
		t.Fatalf("closed channel should yield eventClosedMsg")
	}
}

func TestApplyEventUpdatesLiveState(t *testing.T) {
	m := New(&client.Fake{}, "/run/omnimodem.sock")
	m.applyEvent(&pb.Event{Kind: &pb.Event_AudioLevel{AudioLevel: &pb.AudioLevel{Channel: 0, Dbfs: -18}}})
	m.applyEvent(&pb.Event{Kind: &pb.Event_AudioLevel{AudioLevel: &pb.AudioLevel{Channel: 0, Dbfs: -12}}})
	if got := m.live[0].rxDbfs; got != -12 {
		t.Fatalf("rxDbfs = %v, want -12 (latest wins)", got)
	}
	m.applyEvent(&pb.Event{Kind: &pb.Event_PttState{PttState: &pb.PttState{Channel: 0, Keyed: true}}})
	if !m.live[0].pttKeyed {
		t.Fatalf("pttKeyed should be true")
	}
}

func TestConnectedTransitionsToDashboard(t *testing.T) {
	m := New(&client.Fake{State: &pb.ModemState{}}, "/run/omnimodem.sock")
	next, _ := m.Update(connectedMsg{events: make(chan *pb.Event)})
	if next.(*Model).screen != screenDashboard {
		t.Fatalf("want dashboard")
	}
}

func TestDashboardListsChannelsAndRoutes(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.screen = screenDashboard
	m.live[0] = &chanLive{name: "vfo-a", mode: "psk31", rxDbfs: -20}
	if got := m.viewDashboard(); got == "" || !contains(got, "vfo-a") {
		t.Fatalf("dashboard should list vfo-a")
	}
	next, _ := m.updateDashboard(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("o")})
	if next.(*Model).screen != screenOperate {
		t.Fatalf("'o' should open Operate")
	}
}

func contains(s, sub string) bool {
	return len(s) >= len(sub) && (s == sub || indexOf(s, sub) >= 0)
}
func indexOf(s, sub string) int {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}
