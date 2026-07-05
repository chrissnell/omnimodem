# TUI per-mode settings framework — code map

Reference for the reusable settings UI in the omnimodem TUI. Purpose: let a
future agent add settings for a new mode without re-inventing input widgets.

## Where everything lives

| Concern | Location |
| --- | --- |
| Reusable settings widget (`SettingsForm`, `Field`, `FieldKind`, `Option`) | `clients/omnimodem-tui/internal/ui/settings.go` |
| Per-mode field declarations (`modeFields`) + form builder | `clients/omnimodem-tui/internal/app/mode_settings.go` |
| Mode table + `modeParamsFor` (string→typed `ModeParams`) | `clients/omnimodem-tui/internal/app/modes.go` |
| Config-screen integration (Settings row + modal) | `clients/omnimodem-tui/internal/app/view_config.go` |
| Typed params messages | `proto/omnimodem.proto` (`ModeParams` oneof) |

## The widget

`ui.SettingsForm` is mode-agnostic. It takes a `[]ui.Field` and renders/edits
whichever kinds it's given, so modes share one look and one set of key bindings.

Field kinds:

- `FieldText` — free text (e.g. a callsign)
- `FieldNumber` — numeric entry (input filtered to digits/`.`/leading `-`)
- `FieldToggle` — boolean on/off (space/enter/←/→ flips)
- `FieldEnum` — pick one of `Options` (←/→ cycles, wraps)

Set `Advanced: true` on a field to tuck it behind the form's collapsible
"Advanced settings" expander. Values are stored/read as strings by `Key`;
`Update` returns a `changed` bool so the host can auto-persist only on real edits.

## Adding settings for a new mode

1. Add the typed params message to `proto/omnimodem.proto` (if not already there)
   and regenerate `internal/pb`.
2. In `modes.go`, add the mode to the `modes` table and a case in `modeParamsFor`
   that reads its keys via `get(key, default)` and fills the typed params.
3. In `mode_settings.go`, add a case to `modeFields(label)` returning the mode's
   `[]ui.Field`. Use the same string keys the `modeParamsFor` case reads. Mark
   rarely-touched knobs `Advanced: true`.

That's it — the config screen's Settings row, editor modal, change detection, and
auto-apply pipeline all work off `modeFields`/`modeParamsFor` with no further
wiring. Modes with no tunable settings return an empty slice and the Settings row
reads "no settings".
