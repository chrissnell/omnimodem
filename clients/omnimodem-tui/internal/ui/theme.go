// Package ui holds reusable, view-agnostic TUI widgets and the shared theme.
package ui

import "github.com/charmbracelet/lipgloss"

// Palette — one small, consistent set of colors used across the app.
var (
	ColorAccent = lipgloss.Color("39")  // bright blue: focus, selection
	ColorDim    = lipgloss.Color("241") // muted: borders, hints
	ColorError  = lipgloss.Color("203") // red: error toasts
	ColorOK     = lipgloss.Color("78")  // green: connected/OK
	ColorFg     = lipgloss.Color("252")
)

var (
	Accent     = lipgloss.NewStyle().Foreground(ColorAccent)
	Dim        = lipgloss.NewStyle().Foreground(ColorDim)
	Title      = lipgloss.NewStyle().Foreground(ColorAccent).Bold(true)
	FooterKey  = lipgloss.NewStyle().Foreground(ColorAccent)
	FooterText = lipgloss.NewStyle().Foreground(ColorDim)
)
