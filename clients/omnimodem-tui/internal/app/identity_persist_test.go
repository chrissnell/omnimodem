package app

import (
	"path/filepath"
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/config"
	tea "github.com/charmbracelet/bubbletea"
)

// Editing Call/Grid on the Configure screen and blurring off the field persists
// them to the client config file, and a fresh Model reloads them -- i.e. they
// survive an application restart.
func TestIdentityPersistsAcrossRestart(t *testing.T) {
	t.Setenv("OMNIMODEM_TUI_CONFIG", filepath.Join(t.TempDir(), "config.json"))

	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	v.Update(tea.KeyMsg{Type: tea.KeyDown}) // Name -> Call
	v.call.SetValue("")
	for _, r := range "nw5w" {
		v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
	}
	v.Update(tea.KeyMsg{Type: tea.KeyDown}) // Call -> Grid: blur commits the call
	v.grid.SetValue("")
	for _, r := range "em10" {
		v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
	}
	v.Update(tea.KeyMsg{Type: tea.KeyUp}) // Grid -> Call: blur commits the grid

	if got := config.Load(); got.Call != "NW5W" || got.Grid != "EM10" {
		t.Fatalf("persisted identity = %+v, want NW5W/EM10", got)
	}

	// A brand-new Model (simulating a restart) must preload the saved identity
	// rather than the N0CALL/AA00 defaults.
	restarted := New(&client.Fake{}, "x")
	if restarted.myCall != "NW5W" || restarted.myGrid != "EM10" {
		t.Fatalf("after restart myCall/myGrid = %q/%q, want NW5W/EM10",
			restarted.myCall, restarted.myGrid)
	}
}

// A fresh install with no config file starts on the defaults.
func TestIdentityDefaultsWhenNoConfig(t *testing.T) {
	t.Setenv("OMNIMODEM_TUI_CONFIG", filepath.Join(t.TempDir(), "absent.json"))

	m := New(&client.Fake{}, "x")
	if m.myCall != config.DefaultCall || m.myGrid != config.DefaultGrid {
		t.Fatalf("defaults = %q/%q, want %q/%q",
			m.myCall, m.myGrid, config.DefaultCall, config.DefaultGrid)
	}
}
