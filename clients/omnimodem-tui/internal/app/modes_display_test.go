package app

import (
	"strings"
	"testing"
)

// displayMode must render each mode in its conventional casing. Spot-check one
// representative from every family (including the irregular map entries and the
// param-suffixed daemon form) so a change to the rules or the map is caught.
func TestDisplayModeConventionalCasing(t *testing.T) {
	cases := map[string]string{
		"psk31":          "PSK31",
		"qpsk63":         "QPSK63",
		"psk63f":         "PSK63F",
		"psk125rc10":     "PSK125RC10",
		"psk500c4":       "PSK500C4",
		"dominoex4":      "DominoEX 4",
		"dominoexmicro":  "DominoEX Micro",
		"thor25x4":       "THOR 25x4",
		"thormicro":      "THOR Micro",
		"feldhell":       "Feld Hell",
		"hellx5":         "Hell X5",
		"ifkp":           "IFKP",
		"ifkp-slow":      "IFKP Slow",
		"fsq-1.5":        "FSQ-1.5",
		"mfsk64l":        "MFSK64L",
		"contestia8_500": "Contestia 8/500",
		"mt63_1000l":     "MT63-1000L",
		"throb1":         "Throb1",
		"throbx4":        "ThrobX4",
		"navtex":         "NAVTEX",
		"sitorb":         "SITOR-B",
		"wefax576":       "WEFAX-576",
		"rtty":           "RTTY",
		"cw":             "CW",
		"olivia":         "Olivia",
		"afsk1200":       "AFSK1200",
		"scottie1":       "Scottie 1",
		"scottiedx":      "Scottie DX",
		"martin2":        "Martin 2",
		"robot72":        "Robot 72",
		"sc2-180":        "SC2-180",
		"pd120":          "PD120",
		"mp73-n":         "MP73-N",
		"ft8":            "FT8",
		"jt4a":           "JT4A",
		"msk144":         "MSK144",
		"js8":            "JS8",
		// The daemon reports live modes with a param tail; displayMode must strip
		// it before casing.
		"feldhell:center=1500":   "Feld Hell",
		"rtty:baud=45,shift=170": "RTTY",
		"js8:sub=normal":         "JS8",
		"":                       "",
	}
	for in, want := range cases {
		if got := displayMode(in); got != want {
			t.Errorf("displayMode(%q) = %q, want %q", in, got, want)
		}
	}
}

// Every mode in the table must have a non-empty display name with no leftover
// lowercase run at the start (the tell-tale of a raw daemon label leaking into
// the UI). Mixed-case names like "DominoEX" or "Feld Hell" are fine — this only
// rejects a name that begins lowercase.
func TestDisplayModeNoRawLabelLeaks(t *testing.T) {
	for _, m := range modes {
		got := displayMode(m.label)
		if got == "" {
			t.Errorf("displayMode(%q) is empty", m.label)
			continue
		}
		if r := got[0]; r >= 'a' && r <= 'z' {
			t.Errorf("displayMode(%q) = %q starts lowercase", m.label, got)
		}
		if strings.Contains(got, "_") {
			t.Errorf("displayMode(%q) = %q still contains a raw underscore", m.label, got)
		}
	}
}
