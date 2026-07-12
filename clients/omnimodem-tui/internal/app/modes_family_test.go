package app

import "testing"

// Every mode must land in exactly one family, and no mode may fall through to
// the "Other" bucket — an unclassified mode would silently vanish from the
// cascading selector.
func TestEveryModeHasAFamily(t *testing.T) {
	seen := 0
	for i := range families {
		if families[i].name == "Other" {
			var labels []string
			for _, mi := range families[i].modes {
				labels = append(labels, modes[mi].label)
			}
			t.Fatalf("unclassified modes fell into the Other family: %v", labels)
		}
		if len(families[i].modes) == 0 {
			t.Fatalf("family %q has no modes", families[i].name)
		}
		seen += len(families[i].modes)
	}
	if seen != len(modes) {
		t.Fatalf("families cover %d modes, but there are %d", seen, len(modes))
	}
}

// A mode index must map back to a family that actually contains it.
func TestFamilyIdxOfModeIsConsistent(t *testing.T) {
	for i := range modes {
		fi := familyIdxOfMode(i)
		fam := families[fi]
		found := false
		for _, mi := range fam.modes {
			if mi == i {
				found = true
				break
			}
		}
		if !found {
			t.Fatalf("mode %q maps to family %q, which does not contain it",
				modes[i].label, fam.name)
		}
	}
}

// Spot-check the intended groupings: the DominoEX speeds share one family, and a
// standalone mode like FT8 is a family of one.
func TestFamilyGroupingSpotChecks(t *testing.T) {
	byName := func(name string) modeFamilyGroup {
		for i := range families {
			if families[i].name == name {
				return families[i]
			}
		}
		t.Fatalf("family %q not found", name)
		return modeFamilyGroup{}
	}

	dominoex := byName("DominoEX")
	if len(dominoex.modes) < 5 {
		t.Fatalf("DominoEX should collect all speeds, got %d", len(dominoex.modes))
	}
	for _, mi := range dominoex.modes {
		if familyName(modes[mi].label) != "DominoEX" {
			t.Fatalf("DominoEX family holds a non-DominoEX mode: %q", modes[mi].label)
		}
	}

	if ft8 := byName("FT8"); len(ft8.modes) != 1 || modes[ft8.modes[0]].label != "ft8" {
		t.Fatalf("FT8 must be a single-member family, got %+v", ft8)
	}

	// The PSK label space splits into its fldigi sub-families.
	for label, want := range map[string]string{
		"psk31":     "PSK",
		"psk1000":   "PSK",
		"qpsk63":    "QPSK",
		"psk125r":   "PSK-R",
		"psk63f":    "PSK-R",
		"psk63rc4":  "PSK-RC",
		"psk500rc4": "PSK-RC",
		"psk250c6":  "PSK-C",
		"psk1000c2": "PSK-C",
	} {
		if got := familyName(label); got != want {
			t.Fatalf("familyName(%q) = %q, want %q", label, got, want)
		}
	}

	// SSTV submodes (image, no params) collapse into one SSTV family; Hell and
	// WEFAX — the other image modes — stay separate.
	if familyName("scottie1") != "SSTV" || familyName("pd120") != "SSTV" {
		t.Fatalf("SSTV colour modes must map to the SSTV family")
	}
	if familyName("feldhell") != "Hell" || familyName("wefax576") != "WEFAX" {
		t.Fatalf("Hell/WEFAX must not be swept into SSTV")
	}
}

// ADS-B must be a selectable mode: present in the modes table, classified into
// its own "ADS-B" family so it appears in the channel-config picker, and its
// config output must be the bare "adsb" string with no params (the wire form the
// daemon parses into ModeConfig::Adsb).
func TestADSBIsSelectable(t *testing.T) {
	mi := modeByLabel("adsb")
	if mi == nil {
		t.Fatal("adsb missing from modes table — it won't appear in the mode picker")
	}
	if got := familyName("adsb"); got != "ADS-B" {
		t.Fatalf("adsb family = %q, want %q", got, "ADS-B")
	}
	if got := displayMode("adsb"); got != "ADS-B" {
		t.Fatalf("adsb display = %q, want %q", got, "ADS-B")
	}
	if got := modeStringFor("adsb", nil); got != "adsb" {
		t.Fatalf("adsb mode string = %q, want %q", got, "adsb")
	}
	if mp := modeParamsFor("adsb", nil); mp != nil {
		t.Fatalf("adsb ModeParams = %v, want nil (no operator params)", mp)
	}
}
