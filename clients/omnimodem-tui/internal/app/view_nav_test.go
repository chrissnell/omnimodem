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

// 'n' adds a channel: it must open Configure targeting the lowest free id,
// leaving existing channels untouched.
func TestChannelsNAddsNewChannel(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.connected = true
	m.live[0] = &chanLive{name: "vfo-a", mode: "psk31"}
	m.push(newChannelsView(m))
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("n")})
	if _, ok := m.top().(*configView); !ok {
		t.Fatalf("'n' must open the Configure view; top=%T", m.top())
	}
	if m.sel != 1 {
		t.Fatalf("'n' must target the lowest free channel (1); sel=%d", m.sel)
	}
}

// With no channels yet, 'n' targets ch0.
func TestChannelsNFromEmptyTargetsCh0(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.connected = true
	m.push(newChannelsView(m))
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("n")})
	if _, ok := m.top().(*configView); !ok {
		t.Fatalf("'n' must open the Configure view; top=%T", m.top())
	}
	if m.sel != 0 {
		t.Fatalf("'n' on an empty list must target ch0; sel=%d", m.sel)
	}
}

// 'n' fills gaps: with ch0 and ch2 present it must target ch1, not ch3.
func TestChannelsNFillsGap(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.connected = true
	m.live[0] = &chanLive{name: "vfo-a"}
	m.live[2] = &chanLive{name: "vfo-c"}
	m.push(newChannelsView(m))
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("n")})
	if m.sel != 1 {
		t.Fatalf("'n' must fill the lowest gap (1); sel=%d", m.sel)
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

// Auto-apply persists in place: a completed save (saveDoneMsg) must keep the
// Configure form open so the operator can keep editing, not pop back — unless
// they asked to leave (that path is covered by TestConfigEscPersistsAllChosenDevices).
func TestConfigStaysOpenOnSaveComplete(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.connected = true
	m.push(newChannelsView(m))
	m.sel = 0
	m.live[0] = &chanLive{}
	m.push(newConfigView(m))
	m.Update(saveDoneMsg{})
	if _, ok := m.top().(*configView); !ok {
		t.Fatalf("save-complete must keep Configure open; got %T (stack=%d)", m.top(), len(m.stack))
	}
}
