package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	tea "github.com/charmbracelet/bubbletea"
)

// Regression: a view's Update that pushes a new view must not be clobbered by
// the window manager writing the old view back over the new top.

func TestChannelsCOpensConfigView(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.connected = true
	m.live[0] = &chanLive{name: "vfo-a", mode: "psk31"}
	m.push(newChannelsView(m))
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("c")})
	if _, ok := m.top().(*configView); !ok {
		t.Fatalf("'c' must open the Configure view; top=%T, stack=%d", m.top(), len(m.stack))
	}
}

func TestChannelsOOpensOperateView(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.connected = true
	m.live[0] = &chanLive{name: "vfo-a", mode: "psk31"}
	m.push(newChannelsView(m))
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("o")})
	if _, ok := m.top().(*operateView); !ok {
		t.Fatalf("'o' must open the Operate view; top=%T", m.top())
	}
}

// And a config bind that pops (pttBoundMsg) must not panic or leave the popped
// view installed.
func TestConfigPopOnBindComplete(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.connected = true
	m.push(newChannelsView(m))
	m.sel = 0
	m.live[0] = &chanLive{}
	m.push(newConfigView(m))
	m.Update(pttBoundMsg{}) // config view pops itself
	if _, ok := m.top().(*channelsView); !ok {
		t.Fatalf("after bind-complete pop, top should be Channels; got %T (stack=%d)", m.top(), len(m.stack))
	}
}
