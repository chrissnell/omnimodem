package app

import "testing"

// isSDRDevice must route both a remote rtl_tcp endpoint and a locally-attached
// USB dongle to the tuning view; a plain sound card must not.
func TestIsSDRDevice(t *testing.T) {
	cases := map[string]bool{
		"rtltcp:127.0.0.1:1234": true,
		"rtl:serial:00000001":   true,
		"rtl:topo:1-4":          true,
		"alsa:hw:1,0":           false,
		"topo:1-4":              false,
		"":                      false,
	}
	for id, want := range cases {
		if got := isSDRDevice(id); got != want {
			t.Errorf("isSDRDevice(%q) = %v, want %v", id, got, want)
		}
	}
}
