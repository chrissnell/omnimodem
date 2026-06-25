// Package ui holds reusable, view-agnostic TUI widgets and the shared theme.
package ui

import "github.com/charmbracelet/lipgloss"

// A BBS-reader inspired 16-color DOS palette: a black desktop with bright-cyan
// double borders, blue toolbars (top menu + bottom status bar), bright-yellow
// titles/hotkeys, white body text, and a white-on-bright-blue selection bar.
// Using the ANSI 0–15 slots keeps it faithful on any terminal.
var (
	ColorAccent = lipgloss.Color("14") // bright cyan: borders, focus, links
	ColorDim    = lipgloss.Color("8")  // gray: hints
	ColorError  = lipgloss.Color("9")  // bright red: errors
	ColorOK     = lipgloss.Color("10") // bright green: connected/OK
	ColorFg     = lipgloss.Color("15") // bright white: body text
	ColorTitle  = lipgloss.Color("11") // bright yellow: titles, headers, hotkeys
	ColorPanel  = lipgloss.Color("0")  // black: panel / desktop background
	ColorBar    = lipgloss.Color("4")  // blue: top menu + bottom status toolbars
	ColorSel    = lipgloss.Color("12") // bright blue: selected-row bar
)

var (
	// Text styles on the black panel background.
	Accent = lipgloss.NewStyle().Foreground(ColorAccent)
	Dim    = lipgloss.NewStyle().Foreground(ColorDim)
	Title  = lipgloss.NewStyle().Foreground(ColorTitle).Bold(true)

	// Blue toolbars with bright text — the menu/status bars on the black desktop.
	MenuBar   = lipgloss.NewStyle().Background(ColorBar).Foreground(ColorFg)
	StatusBar = lipgloss.NewStyle().Background(ColorBar).Foreground(ColorFg)

	// Footer hotkeys live on the status bar: a bold yellow key, white action.
	FooterKey  = lipgloss.NewStyle().Background(ColorBar).Foreground(ColorTitle).Bold(true)
	FooterText = lipgloss.NewStyle().Background(ColorBar).Foreground(ColorFg)
)
