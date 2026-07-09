package app

import (
	"strconv"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

// This file maps each mode family to the settings it exposes, expressed as
// ui.Field descriptors. The reusable ui.SettingsForm draws and edits them, so a
// new mode only declares WHAT it has to tune here — never HOW to draw it. Fields
// marked Advanced are tucked behind the form's collapsible advanced section.

// centerField is the audio-center knob shared by the whole PSK/MFSK/DominoEX/
// THOR/Hell submode family. It's the one setting those modes carry.
func centerField(def float64) ui.Field {
	return ui.Field{
		Key: "center", Label: "Center", Kind: ui.FieldNumber,
		Default: num(def), Unit: "Hz", Help: "Audio center frequency (Hz)",
	}
}

// enumFrom builds a set of radio-style options from numeric values, using the
// number as both label and stored value (e.g. the Olivia tone-count/bandwidth
// choosers).
func enumFrom(vals []float64) []ui.Option {
	opts := make([]ui.Option, len(vals))
	for i, v := range vals {
		opts[i] = ui.Option{Label: num(v), Value: num(v)}
	}
	return opts
}

// modeFields returns the settings a mode surfaces, in display order. An empty
// slice means the mode has no operator-tunable settings (e.g. the windowed
// weak-signal modes, whose timing is fixed by the standard).
func modeFields(label string) []ui.Field {
	switch label {
	case "cw":
		return []ui.Field{
			{Key: "wpm", Label: "Speed", Kind: ui.FieldNumber, Default: "20", Unit: "WPM"},
			{Key: "tone", Label: "Tone", Kind: ui.FieldNumber, Default: "700", Unit: "Hz",
				Help: "Sidetone / mark frequency"},
		}
	case "rtty":
		return []ui.Field{
			{Key: "baud", Label: "Baud", Kind: ui.FieldEnum, Default: "45.45",
				Options: []ui.Option{
					{Label: "45.45", Value: "45.45"}, {Label: "50", Value: "50"},
					{Label: "75", Value: "75"}, {Label: "100", Value: "100"},
				}},
			{Key: "shift", Label: "Shift", Kind: ui.FieldEnum, Default: "170",
				Options: enumFrom([]float64{85, 170, 200, 425, 850})},
			// Advanced: the tuning tweaks a casual operator leaves alone.
			{Key: "center", Label: "Center", Kind: ui.FieldNumber, Default: "0", Unit: "Hz",
				Advanced: true, Help: "Audio center; 0 uses the 2210 Hz default"},
			{Key: "reverse", Label: "Reverse", Kind: ui.FieldToggle, Default: "0",
				Advanced: true, Help: "Swap mark/space (depends on sideband)"},
		}
	case "olivia":
		return []ui.Field{
			{Key: "tones", Label: "Tones", Kind: ui.FieldEnum, Default: "32",
				Options: enumFrom([]float64{2, 4, 8, 16, 32, 64})},
			{Key: "bw", Label: "Bandwidth", Kind: ui.FieldEnum, Default: "1000", Unit: "Hz",
				Options: enumFrom([]float64{125, 250, 500, 1000, 2000})},
		}
	case "afsk1200":
		return []ui.Field{
			{Key: "tx", Label: "Transmit", Kind: ui.FieldToggle, Default: "1",
				Help: "Enable the transmit modulator"},
		}
	case "fsq", "fsq-1.5", "fsq-2", "fsq-4.5", "fsq-6":
		// FSQ shares the family audio-center knob but also carries the FSQCALL
		// directed flag, which turns on the CRC8 selective-call header (the
		// operator callsign is injected from the station identity, not edited here).
		return []ui.Field{
			centerField(1500),
			{Key: "directed", Label: "Directed", Kind: ui.FieldToggle, Default: "0",
				Help: "Send the FSQCALL CRC8 directed header + selective-call framing"},
		}
	case "js8":
		// JS8 runs one of four fixed speeds; the daemon windows each at its own T/R
		// period. Higher speeds trade robustness for throughput.
		return []ui.Field{
			{Key: "sub", Label: "Speed", Kind: ui.FieldEnum, Default: "normal",
				Options: []ui.Option{
					{Label: "Normal (15s)", Value: "normal"},
					{Label: "Fast (10s)", Value: "fast"},
					{Label: "Turbo (6s)", Value: "turbo"},
					{Label: "Slow (30s)", Value: "slow"},
				}},
		}
	case "fst4":
		// FST4's defining knob is the T/R period: longer slots dig deeper on LF/MF
		// at the cost of QSO rate. The values are the WSJT-X FST4 sequence lengths.
		return []ui.Field{
			{Key: "tr", Label: "T/R period", Kind: ui.FieldEnum, Default: "15", Unit: "s",
				Options: enumFrom([]float64{15, 30, 60, 120, 300, 900, 1800}),
				Help:    "Transmit/receive slot length (seconds)"},
		}
	case "msk144":
		// MSK144 is streaming (no T/R slot); its one knob is the audio centre.
		return []ui.Field{
			{Key: "freq", Label: "Center", Kind: ui.FieldNumber, Default: "1500", Unit: "Hz",
				Help: "Audio center frequency (Hz)"},
		}
	}

	// The submode families all share a single audio-center knob. Its default is
	// the mode's own center param, so the form seeds the same value the daemon
	// would use if Center were left untouched (psk31 1000, navtex 1000, wefax
	// 1900, everything else 1500) instead of a hardcoded guess.
	if fam := submodeFamily(label); fam != "" {
		return []ui.Field{centerField(centerDefault(label))}
	}

	// Contestia's tones/bandwidth are fixed by the submode label, and the remaining
	// windowed modes (ft8/ft4/jt65/jt9/wspr and the jt4 submodes) have no operator
	// settings.
	return nil
}

// submodeFamily reports whether a label belongs to one of the submode families
// whose only setting is audio center (returns the family name, else "").
func submodeFamily(label string) string {
	mi := modeByLabel(label)
	if mi == nil {
		return ""
	}
	// These families carry a "center" modeParam and nothing else.
	for _, p := range mi.params {
		if p.key == "center" {
			return "submode"
		}
	}
	return ""
}

// centerDefault returns a submode-family mode's audio-center default, read from
// its modeInfo center param so the settings form and the daemon agree on the
// untouched value (navtex/sitorb 1000 Hz, wefax 1900 Hz, psk31 1000 Hz). Falls
// back to 1500 Hz for a mode with no center param.
func centerDefault(label string) float64 {
	if mi := modeByLabel(label); mi != nil {
		for _, p := range mi.params {
			if p.key == "center" {
				return p.def
			}
		}
	}
	return 1500
}

// newModeSettingsForm builds a settings form for a mode. initial overrides seed
// current values (over each field's default) by key.
func newModeSettingsForm(label string, initial map[string]float64) *ui.SettingsForm {
	var seed map[string]string
	if len(initial) > 0 {
		seed = make(map[string]string, len(initial))
		for k, v := range initial {
			seed[k] = num(v)
		}
	}
	return ui.NewSettingsForm(modeFields(label), seed)
}

// modeValsFrom converts a settings form's string values into the float64 map
// modeParamsFor consumes. Non-numeric (free-text) values are skipped, since no
// current mode carries a text setting the daemon reads as a number.
func modeValsFrom(f *ui.SettingsForm) map[string]float64 {
	if f == nil {
		return nil
	}
	vals := f.Values()
	if len(vals) == 0 {
		return nil
	}
	out := make(map[string]float64, len(vals))
	for k, s := range vals {
		if v, err := strconv.ParseFloat(s, 64); err == nil {
			out[k] = v
		}
	}
	return out
}

// num formats a float without a trailing ".0" so "45.45" and "170" both read
// naturally in the UI and round-trip through strconv.ParseFloat.
func num(v float64) string {
	return strconv.FormatFloat(v, 'g', -1, 64)
}

