// Package config persists client-side operator preferences that the daemon does
// not own. The station identity (callsign and Maidenhead grid) is used only by
// the TUI -- FT8/WSPR message text, macros, and the QSO log are all built
// frontend-side and the modem only ever transmits finished text -- so it lives
// in a frontend-local file rather than the modem's config store.
package config

import (
	"encoding/json"
	"os"
	"path/filepath"
)

// Placeholders shown until the operator sets their own on the Configure screen.
const (
	DefaultCall = "N0CALL"
	DefaultGrid = "AA00"
)

// Identity is the operator's station identity.
type Identity struct {
	Call string `json:"call"`
	Grid string `json:"grid"`
}

// Path is the config file location: OMNIMODEM_TUI_CONFIG when set (an explicit
// override, also handy for tests), else <os.UserConfigDir>/omnimodem-tui/config.json.
func Path() (string, error) {
	if p := os.Getenv("OMNIMODEM_TUI_CONFIG"); p != "" {
		return p, nil
	}
	dir, err := os.UserConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "omnimodem-tui", "config.json"), nil
}

// Load reads the persisted identity. A missing or unreadable file yields the
// defaults with no error, so a first run starts clean instead of failing.
func Load() Identity {
	id := Identity{Call: DefaultCall, Grid: DefaultGrid}
	p, err := Path()
	if err != nil {
		return id
	}
	data, err := os.ReadFile(p)
	if err != nil {
		return id
	}
	var stored Identity
	if err := json.Unmarshal(data, &stored); err != nil {
		return id
	}
	// Keep the defaults for any field the stored file left blank, so a
	// half-written config never shows an empty callsign.
	if stored.Call != "" {
		id.Call = stored.Call
	}
	if stored.Grid != "" {
		id.Grid = stored.Grid
	}
	return id
}

// Save writes the identity, creating the parent directory as needed.
func Save(id Identity) error {
	p, err := Path()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(id, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(p, data, 0o644)
}
