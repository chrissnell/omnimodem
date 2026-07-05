package ui

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func key(s string) tea.KeyMsg {
	switch s {
	case "up":
		return tea.KeyMsg{Type: tea.KeyUp}
	case "down":
		return tea.KeyMsg{Type: tea.KeyDown}
	case "left":
		return tea.KeyMsg{Type: tea.KeyLeft}
	case "right":
		return tea.KeyMsg{Type: tea.KeyRight}
	case "enter":
		return tea.KeyMsg{Type: tea.KeyEnter}
	case "space":
		return tea.KeyMsg{Type: tea.KeySpace}
	case "backspace":
		return tea.KeyMsg{Type: tea.KeyBackspace}
	default:
		return tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(s)}
	}
}

func typeStr(f *SettingsForm, s string) (changed bool) {
	for _, r := range s {
		c, _ := f.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{r}})
		changed = changed || c
	}
	return
}

func demoFields() []Field {
	return []Field{
		{Key: "call", Label: "Call", Kind: FieldText, Default: ""},
		{Key: "wpm", Label: "WPM", Kind: FieldNumber, Default: "20"},
		{Key: "tx", Label: "Transmit", Kind: FieldToggle, Default: "1"},
		{Key: "bw", Label: "Bandwidth", Kind: FieldEnum, Default: "500",
			Options: []Option{{"250 Hz", "250"}, {"500 Hz", "500"}, {"1000 Hz", "1000"}}},
		{Key: "center", Label: "Center", Kind: FieldNumber, Default: "1500", Advanced: true, Unit: "Hz"},
		{Key: "reverse", Label: "Reverse", Kind: FieldToggle, Default: "0", Advanced: true},
	}
}

// A text field must accept typed characters and report the change.
func TestSettingsTextEdits(t *testing.T) {
	f := NewSettingsForm(demoFields(), nil)
	// Focus starts on the first row (Call, a text field).
	if !typeStr(f, "K5ABC") {
		t.Fatal("typing into a text field must report a change")
	}
	if got := f.Value("call"); got != "K5ABC" {
		t.Fatalf("text value = %q, want K5ABC", got)
	}
}

// A number field must reject non-numeric characters.
func TestSettingsNumberFiltersInput(t *testing.T) {
	f := NewSettingsForm(demoFields(), nil)
	f.Update(key("down")) // -> WPM
	f.inputs[1].SetValue("")
	f.values["wpm"] = ""
	typeStr(f, "3a5b.")
	if got := f.Value("wpm"); got != "35." {
		t.Fatalf("number field must keep only numerics, got %q", got)
	}
}

// A toggle flips on space/enter/left/right and reports the change.
func TestSettingsToggleFlips(t *testing.T) {
	f := NewSettingsForm(demoFields(), nil)
	f.Update(key("down")) // WPM
	f.Update(key("down")) // Transmit (default on)
	changed, _ := f.Update(key("space"))
	if !changed || isOn(f.Value("tx")) {
		t.Fatalf("space must flip the toggle off, got %q changed=%v", f.Value("tx"), changed)
	}
	f.Update(key("right"))
	if !isOn(f.Value("tx")) {
		t.Fatal("right must flip the toggle back on")
	}
}

// An enum cycles through its options with left/right and wraps.
func TestSettingsEnumCycles(t *testing.T) {
	f := NewSettingsForm(demoFields(), nil)
	for i := 0; i < 3; i++ {
		f.Update(key("down")) // -> Bandwidth
	}
	if f.Value("bw") != "500" {
		t.Fatalf("enum should start at default 500, got %q", f.Value("bw"))
	}
	f.Update(key("right"))
	if f.Value("bw") != "1000" {
		t.Fatalf("right should advance to 1000, got %q", f.Value("bw"))
	}
	f.Update(key("right")) // wrap to first
	if f.Value("bw") != "250" {
		t.Fatalf("right past the end should wrap to 250, got %q", f.Value("bw"))
	}
	f.Update(key("left"))
	if f.Value("bw") != "1000" {
		t.Fatalf("left should wrap back to 1000, got %q", f.Value("bw"))
	}
}

// Advanced fields are hidden until the expander is opened; navigation cannot
// reach them while collapsed, and expanding reveals them.
func TestSettingsAdvancedHiddenUntilExpanded(t *testing.T) {
	f := NewSettingsForm(demoFields(), nil)
	if strings.Contains(f.View(60), "Center") {
		t.Fatal("advanced field must be hidden while collapsed")
	}
	// Walk to the last visible row: Call, WPM, Transmit, Bandwidth, [Advanced].
	rows := f.rows()
	if got := len(rows); got != 5 {
		t.Fatalf("collapsed form should show 4 basic rows + expander = 5, got %d", got)
	}
	for i := 0; i < len(rows)-1; i++ {
		f.Update(key("down"))
	}
	// Focus is on the Advanced expander; open it.
	f.Update(key("enter"))
	if !f.showAdv {
		t.Fatal("enter on the expander must reveal the advanced section")
	}
	if !strings.Contains(f.View(60), "Center") {
		t.Fatal("advanced field must render once expanded")
	}
	if len(f.rows()) != 7 {
		t.Fatalf("expanded form should show 6 fields + expander = 7 rows, got %d", len(f.rows()))
	}
}

// A form with no advanced fields shows no expander row.
func TestSettingsNoAdvancedNoExpander(t *testing.T) {
	f := NewSettingsForm([]Field{{Key: "center", Label: "Center", Kind: FieldNumber, Default: "1500"}}, nil)
	if strings.Contains(f.View(60), "Advanced") {
		t.Fatal("a form without advanced fields must not draw the expander")
	}
	if len(f.rows()) != 1 {
		t.Fatalf("want a single row, got %d", len(f.rows()))
	}
}

// initial overrides seed values over field defaults.
func TestSettingsInitialOverridesDefaults(t *testing.T) {
	f := NewSettingsForm(demoFields(), map[string]string{"wpm": "28", "bw": "1000"})
	if f.Value("wpm") != "28" || f.Value("bw") != "1000" {
		t.Fatalf("initial values must override defaults, got wpm=%q bw=%q", f.Value("wpm"), f.Value("bw"))
	}
	if f.Value("center") != "1500" {
		t.Fatalf("unset keys keep their default, got center=%q", f.Value("center"))
	}
}

// Plain navigation reports no change (so a host won't auto-persist on arrows).
func TestSettingsNavigationIsNotAChange(t *testing.T) {
	f := NewSettingsForm(demoFields(), nil)
	if changed, _ := f.Update(key("down")); changed {
		t.Fatal("moving focus must not count as a value change")
	}
	if changed, _ := f.Update(key("up")); changed {
		t.Fatal("moving focus must not count as a value change")
	}
}

// Values returns every field, advanced included, even while collapsed.
func TestSettingsValuesIncludeAdvanced(t *testing.T) {
	f := NewSettingsForm(demoFields(), nil)
	vals := f.Values()
	for _, k := range []string{"call", "wpm", "tx", "bw", "center", "reverse"} {
		if _, ok := vals[k]; !ok {
			t.Fatalf("Values must include %q even when advanced is collapsed", k)
		}
	}
}
