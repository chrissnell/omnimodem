package config

import (
	"path/filepath"
	"testing"
)

// A round trip through Save/Load returns exactly what was written.
func TestSaveLoadRoundTrip(t *testing.T) {
	t.Setenv("OMNIMODEM_TUI_CONFIG", filepath.Join(t.TempDir(), "config.json"))

	want := Identity{Call: "NW5W", Grid: "EM10"}
	if err := Save(want); err != nil {
		t.Fatalf("Save: %v", err)
	}
	if got := Load(); got != want {
		t.Fatalf("Load = %+v, want %+v", got, want)
	}
}

// A missing file yields the defaults, not an error, so first run starts clean.
func TestLoadMissingFileReturnsDefaults(t *testing.T) {
	t.Setenv("OMNIMODEM_TUI_CONFIG", filepath.Join(t.TempDir(), "absent.json"))

	got := Load()
	if got.Call != DefaultCall || got.Grid != DefaultGrid {
		t.Fatalf("Load = %+v, want defaults %q/%q", got, DefaultCall, DefaultGrid)
	}
}

// A field left blank in the stored file keeps its default rather than showing
// an empty value.
func TestLoadFillsBlankFieldsWithDefaults(t *testing.T) {
	t.Setenv("OMNIMODEM_TUI_CONFIG", filepath.Join(t.TempDir(), "config.json"))

	if err := Save(Identity{Call: "K1ABC", Grid: ""}); err != nil {
		t.Fatalf("Save: %v", err)
	}
	got := Load()
	if got.Call != "K1ABC" {
		t.Fatalf("Call = %q, want K1ABC", got.Call)
	}
	if got.Grid != DefaultGrid {
		t.Fatalf("blank Grid = %q, want default %q", got.Grid, DefaultGrid)
	}
}
