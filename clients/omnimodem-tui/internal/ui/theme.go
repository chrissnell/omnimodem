// Package ui holds reusable, view-agnostic TUI widgets and the shared theme.
package ui

import "github.com/charmbracelet/lipgloss"

// A Turbo-Vision / Norton-Commander inspired 16-color DOS palette: a blue
// desktop, bright-cyan/white text, yellow titles, a light-gray menu bar and a
// cyan status bar. Using the ANSI 0–15 slots keeps it faithful on any terminal.
var (
	ColorAccent = lipgloss.Color("14") // bright cyan: focus, selection
	ColorDim    = lipgloss.Color("8")  // gray: inactive borders/hints
	ColorError  = lipgloss.Color("9")  // bright red: errors
	ColorOK     = lipgloss.Color("10") // bright green: connected/OK
	ColorFg     = lipgloss.Color("15") // bright white: body text
	ColorTitle  = lipgloss.Color("11") // bright yellow: titles
	ColorPanel  = lipgloss.Color("4")  // DOS blue: panel background
	ColorBar    = lipgloss.Color("6")  // cyan: status bar
	ColorMenu   = lipgloss.Color("7")  // light gray: top menu bar
	ColorInk    = lipgloss.Color("0")  // black: text on the light bars
)

var (
	// Text styles used on the blue panel background.
	Accent = lipgloss.NewStyle().Foreground(ColorAccent)
	Dim    = lipgloss.NewStyle().Foreground(ColorDim)
	Title  = lipgloss.NewStyle().Foreground(ColorTitle).Bold(true)

	// Bar styles (top menu, bottom status) — dark ink on a light fill.
	MenuBar   = lipgloss.NewStyle().Background(ColorMenu).Foreground(ColorInk)
	StatusBar = lipgloss.NewStyle().Background(ColorBar).Foreground(ColorInk)

	// Footer hotkeys live on the status bar: a bold black key, plain action.
	FooterKey  = lipgloss.NewStyle().Background(ColorBar).Foreground(ColorInk).Bold(true)
	FooterText = lipgloss.NewStyle().Background(ColorBar).Foreground(ColorInk)
)
