# TUI UI component library — code map

Reference for the reusable presentation layer in the omnimodem TUI. Purpose: let
a future agent build a new screen that looks like the rest of the app without
re-inventing panels, tables, or the palette. The Configure screen
(`internal/app/view_config.go`) is the worked example — copy its composition.

## Where everything lives

| Concern | Location |
| --- | --- |
| Palette (colours + text styles) | `internal/ui/theme.go` |
| `Card` — rounded titled panel (the building block) | `internal/ui/card.go` |
| `Table` / `TableInset` — bordered data table + inset variant | `internal/ui/table.go` |
| `Modal` — centered dialog box | `internal/ui/modal.go` |
| `Frame` — the window chrome each view is wrapped in (root) | `internal/ui/frame.go` |
| `Header` / `Footer` / `Hint` — top + bottom bars | `internal/ui/chrome.go` |
| `SettingsForm` — mode-agnostic settings editor | `internal/ui/settings.go` |

Everything is drawn with `github.com/charmbracelet/lipgloss` and its built-in
`lipgloss/table` — no external table/TUI deps (bubble-table was evaluated and
rejected: its current release is on the incompatible charm v2 stack).

## Cards

`ui.Card(title, body, focused, w)` is a rounded-border panel with a titled header
and a hairline rule. `focused` lights the border/title in the accent colour; the
outer width is `w` and the body must be pre-wrapped to `ui.CardInnerWidth(w)`.
Compose a screen by building each section's body, wrapping it in a `Card`, and
joining with lipgloss:

```go
left  := lipgloss.JoinVertical(lipgloss.Left, ui.Card("STATION", s, focusA, lw), ui.Card("MODE", md, focusB, lw))
right := lipgloss.JoinVertical(lipgloss.Left, ui.Card("AUDIO", a, focusC, rw), ui.Card("RSID", r, focusD, rw))
cols  := lipgloss.JoinHorizontal(lipgloss.Top, left, "  ", right)
```

Light the card whose section owns the focused widget so the live pane is obvious
(Configure uses `focusBetween(lo, hi)`).

## Tables

`ui.Table(cols, rows, selected)` renders a rounded, bordered table; `selected` is
a 0-based row index (`< 0` = none) drawn with the selection bar. `ui.TableInset`
is the same table with no outer frame — drop it inside a `Card` (as the device
picker does) so the dialog keeps a single border. Columns are fixed-width
(`ui.Column{Title, Width}`); cells are truncated to fit, and `ui.TableWidth(cols)`
predicts the outer width so a surrounding box can hug it.

Column widths are set through the table's StyleFunc, **not** `Table.Width()` —
lipgloss/table drops the right border when an explicit width is combined with
`BorderColumn(false)`.

## Palette

16-colour DOS/BBS theme (`theme.go`): black desktop (`ColorPanel`), bright-cyan
accent/focus (`ColorAccent`), yellow titles (`ColorTitle`), grey hints
(`ColorDim`), blue selection bar (`ColorSel`). Use `ui.Accent` / `ui.Dim` /
`ui.Title` for text; cards default to a dim (unfocused) → accent (focused) border.

## Adding a screen

1. Build each section's body as plain rows (see `view_config.go`'s `*Body`
   helpers); size values to `ui.CardInnerWidth(cardW)` and `clip` long text.
2. Wrap sections in `ui.Card`, arrange with `lipgloss.JoinHorizontal/Vertical`.
3. For any list of records, use `ui.Table` (standalone) or `ui.TableInset`
   inside a `Card`.
4. Return the composition from `View.Render(w, h)`; the root `Frame` + `Header` +
   `Footer` chrome is applied for you.
