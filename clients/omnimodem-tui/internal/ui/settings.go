package ui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
)

// FieldKind is the interaction shape of one setting: a free-text box, a numeric
// box, an on/off toggle, or a one-of-N chooser. A mode declares its settings as
// a []Field and the SettingsForm draws and edits whichever kinds it's given, so
// a new mode never has to hand-roll its own input widgets.
type FieldKind int

const (
	FieldText   FieldKind = iota // free text (e.g. a callsign)
	FieldNumber                  // numeric entry (digits, '.', leading '-')
	FieldToggle                  // boolean on/off
	FieldEnum                    // pick one of Options (radio / dropdown)
)

// Option is one choice in a FieldEnum: Label is shown, Value is stored.
type Option struct {
	Label string
	Value string
}

// Field describes a single editable setting. Key is the stable identifier the
// caller reads back with Value/Values; Label is the display name. Advanced moves
// the field into a collapsible "Advanced settings" section so a mode can surface
// its everyday knobs up front and tuck the rarely-touched ones away. Default
// seeds the value when the caller supplies no initial override.
type Field struct {
	Key      string
	Label    string
	Kind     FieldKind
	Advanced bool
	Help     string // one-line hint shown under the field when focused
	Default  string

	// Text / Number
	Placeholder string
	Unit        string // suffix shown after the value, e.g. "Hz"

	// Enum
	Options []Option
}

// advToggle is the sentinel row index for the "Advanced settings" expander.
const advToggle = -1

// SettingsForm is a reusable, mode-agnostic settings editor. It owns the field
// descriptors, the current string values, and the focus/expansion state, and
// draws every field consistently so modes share one look and one set of key
// bindings. It is a plain widget: the host view drives it with key messages and
// reads Values() back — it performs no I/O of its own.
type SettingsForm struct {
	fields  []Field
	values  map[string]string
	inputs  []textinput.Model // parallel to fields; only Text/Number slots are live
	focus   int               // index into the current visible rows
	showAdv bool              // advanced section expanded
}

// NewSettingsForm builds a form for the given fields. initial overrides a field's
// Default by key (nil means "use every default"). Text/number fields get a live
// text input seeded with their value.
func NewSettingsForm(fields []Field, initial map[string]string) *SettingsForm {
	f := &SettingsForm{
		fields: fields,
		values: make(map[string]string, len(fields)),
		inputs: make([]textinput.Model, len(fields)),
	}
	for i := range fields {
		fld := &fields[i]
		val := fld.Default
		if initial != nil {
			if v, ok := initial[fld.Key]; ok {
				val = v
			}
		}
		f.values[fld.Key] = val
		if fld.Kind == FieldText || fld.Kind == FieldNumber {
			ti := textinput.New()
			ti.Prompt = ""
			ti.Placeholder = fld.Placeholder
			ti.CharLimit = 16
			ti.SetValue(val)
			f.inputs[i] = ti
		}
	}
	f.syncFocus()
	return f
}

// HasFields reports whether the form has anything to edit. A mode with no
// settings (e.g. ft8) yields an empty form; the host can skip opening it.
func (f *SettingsForm) HasFields() bool { return len(f.fields) > 0 }

// hasAdvanced reports whether any field is tucked into the advanced section.
func (f *SettingsForm) hasAdvanced() bool {
	for i := range f.fields {
		if f.fields[i].Advanced {
			return true
		}
	}
	return false
}

// rows returns the currently visible rows in display order: the basic fields,
// then the advanced expander (if any advanced fields exist), then the advanced
// fields (only while expanded). Values are field indices, or advToggle.
func (f *SettingsForm) rows() []int {
	var rows []int
	for i := range f.fields {
		if !f.fields[i].Advanced {
			rows = append(rows, i)
		}
	}
	if f.hasAdvanced() {
		rows = append(rows, advToggle)
		if f.showAdv {
			for i := range f.fields {
				if f.fields[i].Advanced {
					rows = append(rows, i)
				}
			}
		}
	}
	return rows
}

// Update feeds one key message to the focused row and reports whether a value
// changed (so the host can auto-persist only on real edits, not navigation).
// Up/Down (and Tab/Shift-Tab) move between rows; Left/Right cycle an enum, flip a
// toggle, or move the text cursor; Enter/Space flip a toggle or expand/collapse
// the advanced section; other keys edit the focused text/number box.
func (f *SettingsForm) Update(msg tea.KeyMsg) (bool, tea.Cmd) {
	rows := f.rows()
	if len(rows) == 0 {
		return false, nil
	}
	if f.focus >= len(rows) {
		f.focus = len(rows) - 1
	}
	switch msg.String() {
	case "up", "shift+tab":
		if f.focus > 0 {
			f.focus--
			f.syncFocus()
		}
		return false, nil
	case "down", "tab":
		if f.focus < len(rows)-1 {
			f.focus++
			f.syncFocus()
		}
		return false, nil
	}

	cur := rows[f.focus]
	if cur == advToggle {
		switch msg.String() {
		case "enter", " ", "left", "right":
			f.showAdv = !f.showAdv
			f.syncFocus()
		}
		return false, nil
	}

	fld := &f.fields[cur]
	switch fld.Kind {
	case FieldToggle:
		switch msg.String() {
		case "enter", " ", "left", "right":
			return f.set(fld.Key, flip(f.values[fld.Key])), nil
		}
		return false, nil
	case FieldEnum:
		switch msg.String() {
		case "left":
			return f.set(fld.Key, f.cycle(fld, -1)), nil
		case "right", "enter", " ":
			return f.set(fld.Key, f.cycle(fld, +1)), nil
		}
		return false, nil
	default: // FieldText / FieldNumber
		before := f.inputs[cur].Value()
		var cmd tea.Cmd
		f.inputs[cur], cmd = f.inputs[cur].Update(msg)
		got := f.inputs[cur].Value()
		if fld.Kind == FieldNumber {
			if filtered := filterNumeric(got); filtered != got {
				got = filtered
				f.inputs[cur].SetValue(got)
			}
		}
		if got != before {
			f.values[fld.Key] = got
			return true, cmd
		}
		return false, cmd
	}
}

// set stores a value and reports whether it actually changed.
func (f *SettingsForm) set(key, val string) bool {
	if f.values[key] == val {
		return false
	}
	f.values[key] = val
	return true
}

// cycle steps a FieldEnum's value by d (wrapping) and returns the new value. An
// unrecognised current value (or an option-less field) leaves it unchanged.
func (f *SettingsForm) cycle(fld *Field, d int) string {
	if len(fld.Options) == 0 {
		return f.values[fld.Key]
	}
	cur := f.values[fld.Key]
	idx := 0
	for i, o := range fld.Options {
		if o.Value == cur {
			idx = i
			break
		}
	}
	idx = (idx + d + len(fld.Options)) % len(fld.Options)
	return fld.Options[idx].Value
}

// syncFocus points the blink at the focused text/number input (if any) and blurs
// the rest, so only the active box shows a cursor.
func (f *SettingsForm) syncFocus() {
	rows := f.rows()
	var active int = -1
	if f.focus >= 0 && f.focus < len(rows) {
		active = rows[f.focus]
	}
	for i := range f.inputs {
		if f.fields[i].Kind == FieldText || f.fields[i].Kind == FieldNumber {
			if i == active {
				f.inputs[i].Focus()
			} else {
				f.inputs[i].Blur()
			}
		}
	}
}

// Value returns the current string value for a key (empty if unknown).
func (f *SettingsForm) Value(key string) string { return f.values[key] }

// Values returns a copy of every current value, keyed by field key. Advanced
// fields are included whether or not the section is expanded.
func (f *SettingsForm) Values() map[string]string {
	out := make(map[string]string, len(f.values))
	for k, v := range f.values {
		out[k] = v
	}
	return out
}

// View renders the form. width is the available inner width (used to size text
// boxes); the caller supplies any surrounding frame/title.
func (f *SettingsForm) View(width int) string {
	rows := f.rows()
	const labelW = 14
	var b strings.Builder
	for ri, r := range rows {
		focused := ri == f.focus
		marker := "  "
		if focused {
			marker = Accent.Render("▸ ")
		}
		if r == advToggle {
			glyph := "▸"
			if f.showAdv {
				glyph = "▾"
			}
			line := glyph + " Advanced settings"
			if focused {
				line = Accent.Render(line)
			} else {
				line = Dim.Render(line)
			}
			b.WriteString(marker + line + "\n")
			continue
		}
		fld := &f.fields[r]
		label := fmt.Sprintf("%-*s", labelW, fld.Label)
		widget := f.widget(r, width-labelW-4)
		b.WriteString(marker + label + widget + "\n")
		if focused && fld.Help != "" {
			b.WriteString("  " + strings.Repeat(" ", labelW) + Dim.Render(fld.Help) + "\n")
		}
	}
	return strings.TrimRight(b.String(), "\n")
}

// widget renders the value portion of a field row.
func (f *SettingsForm) widget(i, w int) string {
	fld := &f.fields[i]
	switch fld.Kind {
	case FieldToggle:
		if isOn(f.values[fld.Key]) {
			return Accent.Render("[x] On")
		}
		return Dim.Render("[ ] Off")
	case FieldEnum:
		return Accent.Render("‹ "+f.optionLabel(fld)+" ›") + Dim.Render("  (←/→)")
	default:
		// Numbers are short, so give them a compact box; free text may fill the row
		// (leaving room for the unit suffix, if any). Either way keep the box inside
		// the available width so the unit never wraps past the frame.
		width := 10
		if fld.Kind == FieldText {
			width = w
		}
		if fld.Unit != "" {
			width = min(width, w-len(fld.Unit)-1)
		}
		if width < 4 {
			width = 4
		}
		f.inputs[i].Width = width
		out := f.inputs[i].View()
		if fld.Unit != "" {
			out += " " + Dim.Render(fld.Unit)
		}
		return out
	}
}

// optionLabel is the display label for a FieldEnum's current value (falling back
// to the raw value when it matches no option).
func (f *SettingsForm) optionLabel(fld *Field) string {
	cur := f.values[fld.Key]
	for _, o := range fld.Options {
		if o.Value == cur {
			return o.Label
		}
	}
	return cur
}

// flip toggles a boolean string value ("1" ⇄ "0").
func flip(v string) string {
	if isOn(v) {
		return "0"
	}
	return "1"
}

// isOn reports whether a stored toggle value is truthy.
func isOn(v string) bool { return v == "1" || v == "true" || v == "on" }

// filterNumeric strips everything but digits, one decimal point, and a leading
// minus, so a FieldNumber box can only ever hold a parseable number.
func filterNumeric(s string) string {
	var b strings.Builder
	seenDot := false
	for i, r := range s {
		switch {
		case r >= '0' && r <= '9':
			b.WriteRune(r)
		case r == '.' && !seenDot:
			seenDot = true
			b.WriteRune(r)
		case r == '-' && i == 0:
			b.WriteRune(r)
		}
	}
	return b.String()
}
