# TUI per-mode settings framework — code map

Reference for the reusable settings UI in the omnimodem TUI. Purpose: let a
future agent add settings for a new mode without re-inventing input widgets.

## Where everything lives

| Concern | Location |
| --- | --- |
| Reusable settings widget (`SettingsForm`, `Field`, `FieldKind`, `Option`) | `clients/omnimodem-tui/internal/ui/settings.go` |
| Per-mode field declarations (`modeFields`) + form builder | `clients/omnimodem-tui/internal/app/mode_settings.go` |
| Mode table + `modeParamsFor` (string→typed `ModeParams`) | `clients/omnimodem-tui/internal/app/modes.go` |
| Mode-family grouping (`familyName`, `families`, cascading selector) | `clients/omnimodem-tui/internal/app/modes.go` |
| Config-screen integration (Family/Mode rows, Settings row + modal) | `clients/omnimodem-tui/internal/app/view_config.go` |
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

## Mode families (cascading selector)

The Configure screen picks a mode with two cascading rows instead of one long
cycle over ~180 modes:

- **Family** — the mode family (PSK, DominoEX, THOR, SSTV, FT8, …). Cycling it
  lands on the family's first submode.
- **Mode** — the specific submode within the selected family, shown with an
  `n/total` position. A single-member family (CW, FT8, RTTY, …) shows the lone
  mode with an "(only mode)" note and nothing to cycle.

Families are **computed**, never hand-maintained: `families` is built once from
the `modes` table by `familyName(label)`, so membership can't drift from the
source of truth. `familyName` classifies by label prefix/suffix (with the PSK
label space split into PSK / QPSK / PSK-R / PSK-RC / PSK-C, and the remaining
`shape: "image"` modes swept into SSTV after Hell/WEFAX).

**Adding a mode:** if its label starts with an existing family's prefix (e.g.
another `dominoexNN`) it's grouped automatically. A label that matches no family
falls into an "Other" bucket — `TestEveryModeHasAFamily` fails loudly so it never
reaches the UI. Give a genuinely new scheme its own `familyName` case.
