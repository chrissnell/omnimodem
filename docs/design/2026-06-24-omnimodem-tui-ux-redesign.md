# Omnimodem TUI — UX Redesign (k9s-style windowed app)

> Status: **Design / RFC** for review. Supersedes the screen/UX portions of
> `docs/design/2026-06-23-omnimodem-tui-client.md` (its gRPC/transport/event-stream
> design still holds). Targets the existing `clients/omnimodem-tui/` Go client.

## 1. Why

The first TUI shipped functional plumbing but **placeholder views**: plain stacked
text, no interactive widgets, no layout. The concrete failure an operator hits:

- The configuration screen lists audio devices as **static text with no cursor or
  selection** — there is no way to actually choose one. `rxDev` stays empty, so
  `ConfigureAudio` is sent with an empty `device_id` and the daemon rejects it
  (`InvalidArgument: device_id must not be empty`). **You cannot configure a rig.**
- No windowing, borders, focus, filtering, or contextual key hints. It does not
  read as an application.

Goal: a **k9s-style windowed TUI** — bordered panes, a persistent header and a
contextual hotkey footer, `:`-command navigation between views, and **selectable
lists / real forms everywhere**. Usable enough that an operator configures a rig
and transmits without reading source.

## 2. Goals & non-goals

**Goals**
1. **Selectable everything.** Device pickers (RX / TX / PTT), mode picker, and
   PTT-method picker are navigable lists (`j/k`/arrows, `Enter` to choose, `/` to
   filter); numeric params are typed fields. Configuration is impossible to get
   wrong by omission (Apply is gated on a valid selection).
2. **Windowed layout.** Full-screen, bordered panes that reflow on resize
   (`WindowSizeMsg`), a header (app · connection · daemon socket · version) and a
   **contextual hotkey footer**.
3. **k9s navigation idioms.** `:` command bar to switch views; single-key actions
   per view; `/` filter; `?` help overlay; `Esc`/`q` to go back; `Ctrl-C` quit.
4. **Consistent theme** (Lipgloss): a small palette, clear focus highlighting,
   borders, title bars.
5. Preserve the working, tested layers untouched: the gRPC `ModemClient`, the
   `SubscribeEvents`→channel→`waitForEvent` bridge, and live-state folding.
6. **Errors as transient toasts**, not a raw RPC string dumped in the status line.

**Non-goals (this redesign)**
- New daemon capabilities — purely a client-side UX rebuild against today's API.
- RX decode display (still deferred; its panes are reserved, see §6.4).
- Mouse support, theming config files, i18n.

## 3. Framework decision

**Decision: keep Bubble Tea + Lipgloss + Bubbles, rebuilt into a windowed layout.**
Only the *view/update* layer changes; the gRPC client and event bridge are
framework-agnostic and stay.

- **Bubbles** gives the interactive widgets we lack today: `list` (filterable,
  cursored — fixes the device-picker blocker), `textinput`, `viewport`, `help`,
  `key` (declarative keymaps), `spinner`.
- **Lipgloss** gives borders, titled panes, `JoinHorizontal/Vertical` layout, and
  a theme — enough for a k9s-like windowed look.

**Alternative considered — `tview`/`tcell` (k9s's actual stack).** It is more
"widgets-with-focus and Flex/Pages layout" out of the box, so it is the closer
*literal* match to k9s. Rejected for this pass because it would discard the
already-tested Bubble Tea view layer for stylistic parity we can reach with
Lipgloss/Bubbles, and it fragments the client across two paradigms. *If on review
you specifically want the tview architecture, say so — it changes §4–§5 but not
§6's screen behavior, and I'll revise before any code.*

## 4. Architecture

The root `Model` becomes a **window manager** over a stack of **views**. Today's
ad-hoc `screen` enum + per-screen `update*/view*` methods are replaced by a small
view abstraction so each screen is an isolated, testable unit.

```
type View interface {
    Update(tea.Msg) (View, tea.Cmd)   // handle input/events; return next state
    View(w, h int) string             // render into a w×h content area
    Title() string                    // shown in the pane title / breadcrumb
    Footer() []key.Binding            // contextual hotkeys for the footer
}
```

- **Root model** owns: the `ModemClient`, the event channel + `cancel`, shared
  **live state** (channel map from events), terminal `w/h`, a **view stack**
  (push on drill-in, pop on `Esc`), the command bar, the help overlay, and a
  transient **toast** (error/info with a TTL).
- **Chrome** (always rendered by the root): top **header** bar, the active view's
  bordered content pane, and the **footer** hotkey strip (built from the active
  view's `Footer()` plus global keys). The command bar (`:`) and help (`?`) render
  as overlays.
- **Event routing.** The root keeps draining `SubscribeEvents` and folds LOSSY
  events into shared live state (unchanged), *then* forwards the event to the
  active view (so Operate can consume `SpectrumFrame`/`TransmitComplete`). Exactly
  one `waitForEvent` stays outstanding (carried over from the current design, which
  the code review verified).
- **Resize.** `WindowSizeMsg` updates `w/h`; the root computes the content rect
  (minus header/footer/borders) and passes it to `View(w,h)`. (Today's code stores
  `w/h` but never uses them — that's why nothing reflows.)
- **Focus.** Within a view, a `focus int` selects the active widget; `Tab`/`Shift-Tab`
  cycle; only the focused widget receives key input. Lipgloss highlights the
  focused pane border.

**Reusable components** (new `internal/app/ui/` package):
- `Frame(title, body, focused)` — a titled, bordered pane (focused → accent border).
- `Header(conn, addr, version)` and `Footer(bindings)`.
- `ListPane` — thin wrapper over `bubbles/list` with our styling + a typed
  `Selected()` accessor (used for devices, modes, methods, channels).
- `Form` — an ordered set of fields (`ListField`, `TextField`, `SelectField`) with
  focus traversal and a `Validate()`/`Values()` surface.
- `toast` — transient message with severity + TTL (driven by `tickMsg`).

## 5. Navigation & input model

| Key | Scope | Action |
|---|---|---|
| `:` | global | open command bar (`:channels`, `:devices`, `:config`, `:operate`, `:quit`) |
| `?` | global | toggle help overlay (lists all bindings) |
| `Esc` / `q` | global | pop view / close overlay (Operate: also halts TX if active) |
| `Ctrl-C` | global | cancel event stream + quit |
| `↑/↓` `j/k` | list/form | move cursor |
| `Enter` | list/form | select / drill in / apply |
| `/` | list | filter |
| `Tab`/`Shift-Tab` | form | next/prev field |

The footer always shows the active view's bindings (k9s-style), e.g. on Channels:
`<enter> operate · <c> configure · </> filter · <:> cmd · <?> help`.

## 6. Screens

All screens render inside the bordered chrome. Wireframes are illustrative.

### 6.1 Channels (home)
A `ListPane`/table of channels from live state. Columns: id, name, mode (+params),
bound device, PTT, live RX dBFS, TX-lease holder. Live-updated from the event
stream.

```
┌ omnimodem ───────────────── ● connected · /tmp/omnimodem/omnimodem.sock · v0.x ┐
│ Channels (2)                                                                    │
│ ┌─────────────────────────────────────────────────────────────────────────┐  │
│ │ ▸ ch0  vfo-a   psk31 @1000   BlackHole 2ch        PTT ▢   RX −18 dBFS     │  │
│ │   ch1  vfo-b   ft8           —                    PTT ▢   RX  −−          │  │
│ └─────────────────────────────────────────────────────────────────────────┘  │
│ <enter> operate  <c> configure  <n> new ch  </> filter  <:> cmd  <?> help      │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 6.2 Configure (the fix)
A `Form` with these fields, each a real widget:

- **Name** — `TextField`.
- **Mode** — `SelectField` (psk31 / rtty / cw / ft8 / afsk1200); selecting reveals
  that mode's **param fields** (CW wpm/tone, RTTY baud/shift, PSK31 center) as
  `TextField`s, sent via the typed `mode_params` oneof.
- **RX device** — `ListField` over capture-capable `DeviceInfo` (`/` filters); the
  **selected** id fills `device_id`.
- **TX device** — `ListField` (playback-capable), default "(same as RX)".
- **PTT device** + **PTT method** — `ListField` + `SelectField`
  (NONE/VOX/SERIAL_RTS/SERIAL_DTR/CM108/GPIO); pin/line/invert fields appear for
  the methods that need them.
- **Gain** — RX/TX sliders (`SetAudioGain`).
- **udev helper** — action key that calls `SuggestUdevRule` for the PTT device and
  shows the rule in a scrollable panel.

```
┌ Configure ch0 ─────────────────────────────────────────────────────────────── ┐
│ Name   [vfo-a            ]   Mode  ‹ psk31 ▾ ›  center [1000]                    │
│                                                                                 │
│ RX device  (capture)                    PTT  device ‹ hw:1 ▾ ›  method ‹ VOX ▾ ›│
│ ┌──────────────────────────────┐                                               │
│ │ ▸ BlackHole 2ch              │        Gain  RX ▮▮▮▮▯▯ +6 dB   TX ▮▮▮▯▯▯ 0 dB │
│ │   MacBook Pro Microphone     │                                               │
│ │   LG UltraFine Display Audio │        [u] suggest udev rule                   │
│ │   Microsoft Teams Audio      │                                               │
│ └──────────────────────────────┘                                               │
│ <tab> next field  <enter/space> select  </> filter  <a> apply  <esc> cancel     │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Validation:** `Apply` is disabled (and tells you why) until an RX device is
chosen, so the empty-`device_id` error is structurally impossible. The bind runs
the existing `ConfigureChannel → ConfigureAudio → ConfigurePtt` pipeline; a failed
step surfaces as a toast and leaves you in the form to retry.

### 6.3 Operate
The §6.4-era Operate content (transcript + compose + macro bar + waterfall for
ragchew; auto-sequence ladder + slot clock for FT8) re-housed inside the bordered
chrome, with the macro bar and TX/halt affordances in the footer. Behavior is the
post-review implementation; only the framing/styling changes.

### 6.4 Devices (optional, k9s "resource" view)
`:devices` → a read-only `ListPane` of all enumerated devices (id, label,
capture/playback), live with hotplug. Handy for finding a device id and a natural
k9s-ism; low cost on top of `ListPane`.

## 7. Errors, theme, testing

- **Errors:** RPC failures and validation become `toast`s (severity-colored,
  auto-dismiss), not a raw string appended to the status line.
- **Theme:** one Lipgloss palette (base/accent/dim/error); focused pane uses the
  accent border; consistent title bars. No runtime theming.
- **Testing:** every `View.Update` stays unit-testable against the `Fake`
  `ModemClient` (the pattern the review praised). Add a test that a configured RX
  selection produces a non-empty `device_id` in `ConfigureAudio` (regression for
  the reported bug). Optionally golden-frame a couple of `View()` renders with
  `teatest`.

## 8. Phasing

Designed as one coherent UI, delivered in reviewable phases so the blocker is
fixed first:

1. **Chrome + Channels + Configure** — layout (Frame/Header/Footer), `ListPane`,
   `Form`, the device/mode/method pickers, validation. **This makes configuration
   work** and is shippable on its own (answers the "fix-first" need).
2. **Operate** re-housed in the chrome; toasts; resize reflow everywhere.
3. **Polish** — `:` command bar, `?` help overlay, `/` filter, Devices view.

## 9. Open questions

1. **Framework** — proceeding with Bubble Tea + Lipgloss/Bubbles (§3). Switch to
   `tview` only if you want literal k9s architecture.
2. **Channels as `list` vs `table`** — `bubbles/table` gives aligned columns;
   `list` gives `/` filtering. Leaning `table` for Channels, `list` for pickers.
3. **New-channel flow** — `<n>` on Channels to create a channel id before
   configuring, or fold channel creation into the Configure form? (Leaning: fold in.)
