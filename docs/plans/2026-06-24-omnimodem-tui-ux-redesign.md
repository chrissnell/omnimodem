# Omnimodem TUI UX Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the `clients/omnimodem-tui` views into the k9s-style windowed app from `docs/design/2026-06-24-omnimodem-tui-ux-redesign.md` — bordered chrome, contextual hotkey footer, and **selectable lists/forms** so an operator can actually configure a rig (fixing the empty-`device_id` blocker).

**Architecture:** Keep the tested layers untouched (`internal/client`, `internal/pb`, the `SubscribeEvents`→channel→`waitForEvent` bridge, live-state folding). Replace the `screen`-enum + per-screen methods with a `View` interface and a root **window manager** that renders shared chrome (header/footer/toast/overlays) around the active view. Reusable widgets live in a new `internal/ui` package built on Bubble Tea + Lipgloss + **Bubbles** (`list`, `table`, `textinput`, `help`, `key`).

**Tech Stack:** Go 1.26, Bubble Tea v1.3.10, Lipgloss v1.1.0, Bubbles (added in Task 1).

**Build/verify:** `cd clients/omnimodem-tui && go test ./... && go vet ./...`. The `tui (go)` CI job already runs this on every PR — it's the verification of record (the local sandbox can't reliably run long builds). Commit + push per task.

**Phasing** (each phase is shippable; Phase 1 fixes the blocker):
- **Phase 1 (Tasks 1–7):** Bubbles + theme, `ui` chrome, `View` interface + root WM, Channels view, **Configure form with selectable device/mode/PTT pickers + validation**.
- **Phase 2 (Tasks 8–10):** re-house Operate in the chrome; error toasts; resize reflow.
- **Phase 3 (Tasks 11–13):** `:` command bar, `?` help overlay, `:devices` view.

---

## File structure

New `internal/ui/` package (reusable, view-agnostic):
- `ui/theme.go` — palette + shared Lipgloss styles.
- `ui/frame.go` — `Frame` (titled, bordered pane; accent border when focused).
- `ui/chrome.go` — `Header` and `Footer` renderers.
- `ui/toast.go` — transient severity-colored message with TTL.

`internal/app/` (rebuilt view layer; client/events/live-state stay):
- `view.go` — `View` interface + the root `Model` window manager (replaces the old `screen` enum in `model.go`).
- `keys.go` — global key bindings.
- `view_channels.go` — Channels home (Bubbles `table`).
- `view_config.go` — Configure form (replaces `config.go`).
- `view_operate.go` — Operate, re-housed (replaces `operate.go` rendering; reuses `tx.go`/`macros.go`/`waterfall.go`/`ft8.go`/`qsolog.go`).
- `view_devices.go` — Devices list (Phase 3).
- `command.go` — `:` command bar (Phase 3).
- `help.go` — `?` overlay (Phase 3).
- Unchanged: `client`, `pb`, `events.go`, `msgs.go`, `modes.go`, `tx.go`, `macros.go`, `waterfall.go`, `ft8.go`, `qsolog.go`, `status.go` (folded into `ui` chrome).

---

## Phase 1 — Foundation, Channels, Configure (the fix)

### Task 1: Add Bubbles + theme

**Files:** Create `internal/ui/theme.go`; modify `go.mod`.

- [ ] **Step 1: Add the dependency**

```bash
cd clients/omnimodem-tui
go get github.com/charmbracelet/bubbles@latest
```

- [ ] **Step 2: Write the failing test** `internal/ui/theme_test.go`:

```go
package ui

import "testing"

func TestStylesNonEmpty(t *testing.T) {
	if Accent.GetForeground() == nil {
		t.Fatal("Accent must set a foreground color")
	}
	if got := Title.Render("x"); got == "" {
		t.Fatal("Title style must render")
	}
}
```

- [ ] **Step 3: Implement** `internal/ui/theme.go`:

```go
// Package ui holds reusable, view-agnostic TUI widgets and the shared theme.
package ui

import "github.com/charmbracelet/lipgloss"

// Palette — one small, consistent set of colors used across the app.
var (
	ColorAccent = lipgloss.Color("39")  // bright blue: focus, selection
	ColorDim    = lipgloss.Color("241") // muted: borders, hints
	ColorError  = lipgloss.Color("203") // red: error toasts
	ColorOK     = lipgloss.Color("78")  // green: connected/OK
	ColorFg     = lipgloss.Color("252")
)

var (
	Accent     = lipgloss.NewStyle().Foreground(ColorAccent)
	Dim        = lipgloss.NewStyle().Foreground(ColorDim)
	Title      = lipgloss.NewStyle().Foreground(ColorAccent).Bold(true)
	FooterKey  = lipgloss.NewStyle().Foreground(ColorAccent)
	FooterText = lipgloss.NewStyle().Foreground(ColorDim)
)
```

- [ ] **Step 4: Verify** — `go test ./internal/ui/` PASS; `go vet ./...` clean.
- [ ] **Step 5: Commit** — `git commit -am "tui: add bubbles dep + ui theme"`.

### Task 2: Frame + header/footer chrome

**Files:** Create `internal/ui/frame.go`, `internal/ui/chrome.go`, `internal/ui/frame_test.go`.

- [ ] **Step 1: Write the failing test** `internal/ui/frame_test.go`:

```go
package ui

import (
	"strings"
	"testing"
)

func TestFrameShowsTitle(t *testing.T) {
	out := Frame("Channels", "body", true, 40, 6)
	if !strings.Contains(out, "Channels") {
		t.Fatalf("frame must show its title, got:\n%s", out)
	}
}

func TestFooterShowsBindings(t *testing.T) {
	out := Footer([]Hint{{"enter", "operate"}, {"c", "configure"}}, 60)
	if !strings.Contains(out, "enter") || !strings.Contains(out, "configure") {
		t.Fatalf("footer must show hints, got: %s", out)
	}
}
```

- [ ] **Step 2: Run, expect FAIL** (`undefined: Frame`).

- [ ] **Step 3: Implement** `internal/ui/frame.go`:

```go
package ui

import "github.com/charmbracelet/lipgloss"

// Frame draws a titled, bordered pane sized to w×h (outer dimensions). When
// focused the border uses the accent color.
func Frame(title, body string, focused bool, w, h int) string {
	border := ColorDim
	if focused {
		border = ColorAccent
	}
	style := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(border).
		Width(max(1, w-2)).
		Height(max(1, h-2)).
		Padding(0, 1)
	titled := Title.Render(" "+title+" ") + "\n" + body
	return style.Render(titled)
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}
```

And `internal/ui/chrome.go`:

```go
package ui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// Header is the top bar: app name, connection dot + address, version.
func Header(connected bool, addr, version string, w int) string {
	dot, state := Dim.Render("○"), "connecting"
	if connected {
		dot = lipgloss.NewStyle().Foreground(ColorOK).Render("●")
		state = "connected"
	}
	left := Title.Render(" omnimodem ")
	right := fmt.Sprintf("%s %s · %s · %s ", dot, state, addr, version)
	gap := max(1, w-lipgloss.Width(left)-lipgloss.Width(right))
	return left + strings.Repeat(" ", gap) + right
}

// Hint is one footer key→action pair.
type Hint struct{ Key, Action string }

// Footer renders the contextual hotkey strip.
func Footer(hints []Hint, w int) string {
	parts := make([]string, 0, len(hints))
	for _, h := range hints {
		parts = append(parts, FooterKey.Render("<"+h.Key+">")+" "+FooterText.Render(h.Action))
	}
	line := " " + strings.Join(parts, "  ")
	if lipgloss.Width(line) > w {
		line = lipgloss.NewStyle().MaxWidth(w).Render(line)
	}
	return line
}
```

- [ ] **Step 4: Verify** — `go test ./internal/ui/` PASS.
- [ ] **Step 5: Commit** — `git commit -am "tui: ui Frame + Header/Footer chrome"`.

### Task 3: `View` interface + root window manager

Replaces the `screen` enum in `model.go`. The root keeps the client, event channel + `cancel`, live state, terminal `w/h`, a **view stack**, and a toast; it renders chrome around `stack[top]`.

**Files:** Create `internal/app/view.go`, `internal/app/keys.go`; rewrite `internal/app/model.go`; update `internal/app/app_test.go`.

- [ ] **Step 1: Write the failing test** (append to `app_test.go`):

```go
func TestRootRendersHeaderAndActiveView(t *testing.T) {
	m := New(&client.Fake{}, "/tmp/omnimodem/omnimodem.sock")
	m.connected = true
	m.push(newChannelsView(m))
	m.width, m.height = 80, 24
	out := m.View()
	if !strings.Contains(out, "omnimodem") || !strings.Contains(out, "Channels") {
		t.Fatalf("root must render header + active view title:\n%s", out)
	}
}

func TestEscPopsView(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.push(newChannelsView(m))
	m.push(newChannelsView(m)) // stand-in second view
	if len(m.stack) != 2 {
		t.Fatal("expected 2 views")
	}
	m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	if len(m.stack) != 1 {
		t.Fatalf("esc should pop one view, stack=%d", len(m.stack))
	}
}
```

- [ ] **Step 2: Run, expect FAIL** (`undefined: View`, `push`, `newChannelsView`).

- [ ] **Step 3: Implement** `internal/app/view.go`:

```go
package app

import (
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	tea "github.com/charmbracelet/bubbletea"
)

// View is one screen. Update returns the (possibly new) view state + a command;
// View renders into the content rect; Title labels the pane; Hints feeds the
// footer. Views read shared state via the *Model they were built with.
type View interface {
	Update(tea.Msg) (View, tea.Cmd)
	Render(w, h int) string
	Title() string
	Hints() []ui.Hint
}

func (m *Model) push(v View) { m.stack = append(m.stack, v) }

func (m *Model) pop() {
	if len(m.stack) > 1 {
		m.stack = m.stack[:len(m.stack)-1]
	}
}

func (m *Model) top() View {
	if len(m.stack) == 0 {
		return nil
	}
	return m.stack[len(m.stack)-1]
}
```

- [ ] **Step 4: Rewrite** `internal/app/model.go` — root window manager (replaces the old screen enum + dispatch; keeps event/live-state logic):

```go
package app

import (
	"context"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

type chanLive struct {
	name      string
	mode      string
	deviceID  string
	running   bool
	rxDbfs    float32
	txDbfs    float32
	pttKeyed  bool
	clockSync bool
	clockOff  float64
}

// Model is the root window manager.
type Model struct {
	c         client.ModemClient
	addr      string
	version   string
	width     int
	height    int
	live      map[uint32]*chanLive
	sel       uint32
	events    <-chan *pb.Event
	cancel    context.CancelFunc
	connected bool
	stack     []View
	toast     *ui.Toast
}

func New(c client.ModemClient, addr string) *Model {
	return &Model{c: c, addr: addr, version: "dev", live: map[uint32]*chanLive{}}
}

func (m *Model) Init() tea.Cmd { return connectCmd(m.c) }

func (m *Model) applyEvent(ev *pb.Event) { /* unchanged from current model.go */ }

func (m *Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		return m, nil
	case tea.KeyMsg:
		switch msg.String() {
		case "ctrl+c":
			if m.cancel != nil {
				m.cancel()
			}
			return m, tea.Quit
		case "esc", "q":
			if len(m.stack) > 1 {
				m.pop()
				return m, nil
			}
		}
		return m.routeToView(msg)
	case connectedMsg:
		m.connected = true
		m.events = msg.events
		m.cancel = msg.cancel
		m.stack = []View{newChannelsView(m)}
		return m, tea.Batch(snapshotCmd(m.c), waitForEvent(m.events), tickCmd())
	case eventMsg:
		m.applyEvent(msg.ev)
		_, cmd := m.routeToView(msg) // let the active view react too
		return m, tea.Batch(cmd, waitForEvent(m.events))
	case eventClosedMsg:
		m.connected = false
		m.toast = ui.NewToast("event stream closed", ui.SeverityError)
		return m, nil
	case snapshotMsg:
		m.applyEvent(&pb.Event{Kind: &pb.Event_Snapshot{Snapshot: msg.state}})
		return m, nil
	case rpcErrMsg:
		m.toast = ui.NewToast(msg.err.Error(), ui.SeverityError)
		return m, nil
	case tickMsg:
		if m.toast != nil && m.toast.Expired() {
			m.toast = nil
		}
		_, cmd := m.routeToView(msg)
		return m, tea.Batch(cmd, tickCmd())
	}
	return m.routeToView(msg)
}

func (m *Model) routeToView(msg tea.Msg) (tea.Model, tea.Cmd) {
	if v := m.top(); v != nil {
		nv, cmd := v.Update(msg)
		m.stack[len(m.stack)-1] = nv
		return m, cmd
	}
	return m, nil
}

func (m *Model) View() string {
	if !m.connected || len(m.stack) == 0 {
		return "Connecting to " + m.addr + " …"
	}
	v := m.top()
	header := ui.Header(m.connected, m.addr, m.version, m.width)
	footer := ui.Footer(v.Hints(), m.width)
	bodyH := m.height - lipgloss.Height(header) - lipgloss.Height(footer)
	body := ui.Frame(v.Title(), v.Render(m.width-2, bodyH-2), true, m.width, bodyH)
	out := lipgloss.JoinVertical(lipgloss.Left, header, body, footer)
	if m.toast != nil {
		out = m.toast.Overlay(out, m.width, m.height)
	}
	return out
}
```

(Copy `applyEvent`'s body verbatim from the pre-redesign `model.go`.)

- [ ] **Step 5: Implement** `internal/app/keys.go` (global bindings used by views/footer):

```go
package app

import "github.com/charmbracelet/bubbles/key"

var (
	keyUp     = key.NewBinding(key.WithKeys("up", "k"))
	keyDown   = key.NewBinding(key.WithKeys("down", "j"))
	keyEnter  = key.NewBinding(key.WithKeys("enter"))
	keyTab    = key.NewBinding(key.WithKeys("tab"))
	keyFilter = key.NewBinding(key.WithKeys("/"))
)
```

- [ ] **Step 6: Verify + commit** — `go test ./internal/...` PASS once Tasks 4–5 land the views; `git commit -am "tui: View interface + root window manager"`.

### Task 4: Channels view (Bubbles table)

**Files:** Create `internal/app/view_channels.go`; delete `dashboard.go`; test in `view_channels_test.go`.

- [ ] **Step 1: Write the failing test**:

```go
package app

import (
	"strings"
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	tea "github.com/charmbracelet/bubbletea"
)

func TestChannelsRendersRowsAndOpensOperate(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.live[0] = &chanLive{name: "vfo-a", mode: "psk31", rxDbfs: -18}
	v := newChannelsView(m)
	if !strings.Contains(v.Render(80, 10), "vfo-a") {
		t.Fatal("channels view must list the channel")
	}
	_, _ = v.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("o")})
	if _, ok := m.top().(*operateView); !ok {
		// 'o' pushes an operate view onto the root stack
		t.Skip("operate view lands in Task 7; assert push then")
	}
}
```

- [ ] **Step 2: Implement** `internal/app/view_channels.go`:

```go
package app

import (
	"fmt"
	"sort"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	"github.com/charmbracelet/bubbles/table"
	tea "github.com/charmbracelet/bubbletea"
)

type channelsView struct {
	m *Model
	t table.Model
}

func newChannelsView(m *Model) *channelsView {
	cols := []table.Column{
		{Title: "CH", Width: 4}, {Title: "NAME", Width: 10}, {Title: "MODE", Width: 12},
		{Title: "DEVICE", Width: 20}, {Title: "PTT", Width: 4}, {Title: "RX dBFS", Width: 8},
	}
	t := table.New(table.WithColumns(cols), table.WithFocused(true))
	v := &channelsView{m: m, t: t}
	v.refresh()
	return v
}

func (v *channelsView) refresh() {
	chs := make([]uint32, 0, len(v.m.live))
	for ch := range v.m.live {
		chs = append(chs, ch)
	}
	sort.Slice(chs, func(i, j int) bool { return chs[i] < chs[j] })
	rows := make([]table.Row, 0, len(chs))
	for _, ch := range chs {
		cl := v.m.live[ch]
		ptt := "▢"
		if cl.pttKeyed {
			ptt = "▣"
		}
		rows = append(rows, table.Row{
			fmt.Sprintf("ch%d", ch), orNone(cl.name), orNone(cl.mode),
			orDash(cl.deviceID), ptt, fmt.Sprintf("%.0f", cl.rxDbfs),
		})
	}
	v.t.SetRows(rows)
}

func (v *channelsView) Update(msg tea.Msg) (View, tea.Cmd) {
	v.refresh() // reflect live-state changes
	if key, ok := msg.(tea.KeyMsg); ok {
		switch key.String() {
		case "c":
			v.m.sel = v.selectedChannel()
			v.m.push(newConfigView(v.m))
			return v, devicesCmd(v.m.c)
		case "o":
			v.m.sel = v.selectedChannel()
			v.m.push(newOperateView(v.m))
			return v, enableSpectrumCmd(v.m.c, v.m.sel, 64)
		}
	}
	var cmd tea.Cmd
	v.t, cmd = v.t.Update(msg)
	return v, cmd
}

func (v *channelsView) selectedChannel() uint32 {
	var ch uint32
	if r := v.t.SelectedRow(); len(r) > 0 {
		fmt.Sscanf(r[0], "ch%d", &ch)
	}
	return ch
}

func (v *channelsView) Render(w, h int) string { v.t.SetWidth(w); v.t.SetHeight(h); return v.t.View() }
func (v *channelsView) Title() string          { return fmt.Sprintf("Channels (%d)", len(v.m.live)) }
func (v *channelsView) Hints() []ui.Hint {
	return []ui.Hint{{"enter/o", "operate"}, {"c", "configure"}, {":", "cmd"}, {"?", "help"}}
}

func orDash(s string) string {
	if s == "" {
		return "—"
	}
	return s
}
```

(`orNone` already exists in `status.go`/the app package; keep one copy.)

- [ ] **Step 3: Verify + commit** — `go test ./internal/app/ -run TestChannels`; `git rm dashboard.go`; `git commit -am "tui: Channels view (bubbles table)"`.

### Task 5: Configure form — selectable device/mode/PTT pickers (THE FIX)

A `configView` with focusable fields: name (`textinput`), mode (`list`), per-mode param inputs, RX device (`list`, capture-only), TX device (`list`, playback-only), PTT device (`list`) + method (`list`), and an Apply action **gated on a non-empty RX device**. Replaces `config.go`.

**Files:** Create `internal/app/view_config.go`; delete `config.go`; update `config_test.go`.

- [ ] **Step 1: Write the failing tests** (`view_config_test.go`):

```go
package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func devItem(id, label string, cap, play bool) *pb.DeviceInfo {
	return &pb.DeviceInfo{DeviceId: id, Label: label, HasCapture: cap, HasPlayback: play}
}

func TestConfigApplyRejectedWithoutRxDevice(t *testing.T) {
	m := New(&client.Fake{}, "x")
	v := newConfigView(m)
	if v.canApply() {
		t.Fatal("apply must be gated until an RX device is chosen")
	}
}

func TestConfigSelectedRxFillsDeviceID(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 0
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.selectRx("usb:1:2:")
	if !v.canApply() {
		t.Fatal("apply should be allowed once RX device is set")
	}
	v.apply()()      // ConfigureChannel
	v.afterChannel()() // ConfigureAudio
	if len(f.AudioCalls) != 1 || f.AudioCalls[0].GetDeviceId() != "usb:1:2:" {
		t.Fatalf("ConfigureAudio must carry the selected device_id, got %+v", f.AudioCalls)
	}
}
```

- [ ] **Step 2: Implement** `internal/app/view_config.go`:

```go
package app

import (
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/charmbracelet/bubbles/list"
	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
)

// devEntry is a list.Item for a device.
type devEntry struct{ id, label string }

func (d devEntry) Title() string       { return d.label }
func (d devEntry) Description() string { return d.id }
func (d devEntry) FilterValue() string { return d.label + " " + d.id }

type configView struct {
	m       *Model
	name    textinput.Model
	rx      list.Model
	rxID    string
	mode    string
	devices []*pb.DeviceInfo
	focus   int // 0=name 1=mode 2=rx ...
}

func newConfigView(m *Model) *configView {
	name := textinput.New()
	name.SetValue("vfo-a")
	name.Focus()
	rx := list.New(nil, list.NewDefaultDelegate(), 0, 0)
	rx.Title = "RX device (capture)"
	rx.SetShowHelp(false)
	return &configView{m: m, name: name, rx: rx, mode: "psk31"}
}

func (v *configView) setDevices(devs []*pb.DeviceInfo) {
	v.devices = devs
	items := []list.Item{}
	for _, d := range devs {
		if d.GetHasCapture() {
			items = append(items, devEntry{id: d.GetDeviceId(), label: d.GetLabel()})
		}
	}
	v.rx.SetItems(items)
}

func (v *configView) selectRx(id string) { v.rxID = id }

func (v *configView) canApply() bool { return v.rxID != "" }

// apply / afterChannel run the bind pipeline; updateConfig (below) chains them.
func (v *configView) apply() tea.Cmd {
	req := &pb.ConfigureChannelRequest{
		Channel: v.m.sel, Name: v.name.Value(), Mode: v.mode,
		ModeParams: modeParamsFor(v.mode, nil),
	}
	c := v.m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.ConfigureChannel(ctx, req); err != nil {
			return rpcErrMsg{err}
		}
		return channelBoundMsg{}
	}
}

func (v *configView) afterChannel() tea.Cmd {
	req := &pb.ConfigureAudioRequest{Channel: v.m.sel, DeviceId: v.rxID, SampleRate: 48000}
	c := v.m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.ConfigureAudio(ctx, req)
		if err != nil {
			return rpcErrMsg{err}
		}
		return audioCfgMsg{resp}
	}
}

func (v *configView) Update(msg tea.Msg) (View, tea.Cmd) {
	switch msg := msg.(type) {
	case devicesMsg:
		v.setDevices(msg.devices)
		return v, nil
	case channelBoundMsg:
		return v, v.afterChannel()
	case audioCfgMsg:
		v.m.pop() // bind complete (PTT chaining added when PTT picker lands)
		return v, snapshotCmd(v.m.c)
	case tea.KeyMsg:
		switch msg.String() {
		case "tab":
			v.focus++
		case "enter", " ":
			if v.focus == 2 { // RX list focused: choose highlighted item
				if it, ok := v.rx.SelectedItem().(devEntry); ok {
					v.selectRx(it.id)
				}
				return v, nil
			}
		case "a":
			if v.canApply() {
				return v, v.apply()
			}
			v.m.toast = ui.NewToast("pick an RX device first", ui.SeverityWarn)
			return v, nil
		}
	}
	var cmd tea.Cmd
	switch v.focus {
	case 0:
		v.name, cmd = v.name.Update(msg)
	case 2:
		v.rx, cmd = v.rx.Update(msg)
	}
	return v, cmd
}

func (v *configView) Render(w, h int) string {
	v.rx.SetSize(w, h-6)
	body := "Name: " + v.name.View() + "\nMode: " + v.mode + "\nRX:   " + orDash(v.rxID) + "\n\n" + v.rx.View()
	return body
}
func (v *configView) Title() string { return "Configure ch" }
func (v *configView) Hints() []ui.Hint {
	return []ui.Hint{{"tab", "field"}, {"enter", "select"}, {"/", "filter"}, {"a", "apply"}, {"esc", "cancel"}}
}
```

This is the **minimum that fixes the blocker** (selectable RX device → non-empty `device_id`). TX device, PTT device + method, per-mode param inputs, and gain are added as follow-on fields in the same view (same `list`/`textinput` pattern, each its own focus index) — see Task 6.

- [ ] **Step 3: Verify** — `go test ./internal/app/ -run TestConfig`; `git rm config.go config_test.go`; `git commit -am "tui: Configure form with selectable RX device picker (fixes empty device_id)"`.

### Task 6: Complete the Configure form (TX/PTT/method/params/gain)

**Files:** Modify `internal/app/view_config.go` + test.

- [ ] **Step 1: Write the failing test** — selecting a CW mode + entering params yields the right `mode_params`, and a chosen PTT device + method produce a `ConfigurePtt` call after audio:

```go
func TestConfigPttChainAndModeParams(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{devItem("usb:1:2:", "Rig", true, true)})
	v.selectRx("usb:1:2:")
	v.mode = "cw"
	v.params = map[string]float64{"wpm": 25}
	v.pttID = "usb:1:2:"
	v.pttMethod = pb.PttMethod_PTT_METHOD_CM108
	v.apply()()
	if f.ChannelCalls[0].GetModeParams().GetCw().GetWpm() != 25 {
		t.Fatal("cw wpm not sent")
	}
	v.afterChannel()()
	v.afterAudio()()
	if len(f.PttCalls) != 1 || f.PttCalls[0].GetMethod() != pb.PttMethod_PTT_METHOD_CM108 {
		t.Fatalf("ConfigurePtt must carry the chosen method, got %+v", f.PttCalls)
	}
}
```

- [ ] **Step 2: Implement** — add `tx list.Model`, `txID`, `ptt list.Model`, `pttID`, `pttMethod pb.PttMethod`, `methodList list.Model`, `params map[string]float64`, and per-param `textinput.Model`s to `configView`; populate the TX list from playback-capable devices and the method list from the `pb.PttMethod` values; pass `modeParamsFor(v.mode, v.params)` in `apply()`; add `afterAudio()` returning a `ConfigurePtt` cmd; in `Update`, on `audioCfgMsg` return `v.afterAudio()` and on `pttBoundMsg` pop + snapshot. Extend `focus` to cover every field and render each with its widget's `.View()`.

- [ ] **Step 3: Verify + commit** — `go test ./internal/app/`; `git commit -am "tui: Configure — TX/PTT/method pickers + mode params + gain"`.

### Task 7: Cut over to Operate stub view; remove old screen plumbing

**Files:** Create a minimal `internal/app/view_operate.go` (full re-house is Task 8); ensure `dashboard.go`/`config.go`/old `operate.go` are gone and the package compiles only via the View layer.

- [ ] **Step 1:** Add `type operateView struct{ m *Model; op *operateState }` with `newOperateView(m)` initializing `op` as today's `enterOperate` did, and `Update/Render/Title/Hints` delegating to the existing ragchew/FT8 logic (move the body of the old `updateOperate`/`viewOperate` into the methods).
- [ ] **Step 2:** Delete the old `operate.go` rendering that referenced the screen enum; keep `tx.go`/`macros.go`/`waterfall.go`/`ft8.go`/`qsolog.go` unchanged.
- [ ] **Step 3: Verify** — `go test ./... && go vet ./...` all green; `git commit -am "tui: Operate as a View; remove screen-enum plumbing"`.

---

## Phase 2 — Operate polish, toasts, resize

### Task 8: Re-house Operate in the chrome
Render the transcript + compose + waterfall (ragchew) and the FT8 sequencer **inside** `Render(w,h)` using `lipgloss.JoinVertical`/`JoinHorizontal` sized to the content rect; put the macro bar + TX/HALT into `Hints()`. Keep all TX-FSM behavior. Test: `go test` still green; a `Render` smoke test asserts the transcript line appears.

### Task 9: Toasts
**Files:** `internal/ui/toast.go`.

```go
package ui

import (
	"time"

	"github.com/charmbracelet/lipgloss"
)

type Severity int

const (
	SeverityInfo Severity = iota
	SeverityWarn
	SeverityError
)

type Toast struct {
	msg string
	sev Severity
	exp time.Time
}

func NewToast(msg string, sev Severity) *Toast {
	return &Toast{msg: msg, sev: sev, exp: time.Now().Add(4 * time.Second)}
}
func (t *Toast) Expired() bool { return time.Now().After(t.exp) }

func (t *Toast) Overlay(base string, w, h int) string {
	color := ColorAccent
	switch t.sev {
	case SeverityWarn:
		color = lipgloss.Color("214")
	case SeverityError:
		color = ColorError
	}
	box := lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).
		BorderForeground(color).Padding(0, 1).Render(t.msg)
	return lipgloss.Place(w, h, lipgloss.Center, lipgloss.Bottom, box,
		lipgloss.WithWhitespaceChars(" ")) // drawn over the base by the caller's layout
}
```

Route `rpcErrMsg` to a toast (already wired in Task 3's `Update`). Test: `NewToast(...).Expired()` is false immediately, true after advancing past the TTL (inject a clock or assert on a sub-second TTL in the test).

### Task 10: Resize reflow
Already plumbed in Task 3 (`Render(w,h)` gets the content rect). Add a test that `m.View()` at 100×30 and 60×20 both contain the title and differ in width; ensure `table`/`list`/`textinput` get `SetWidth/SetSize` from each view's `Render`.

---

## Phase 3 — Command bar, help, devices view

### Task 11: `:` command bar
**Files:** `internal/app/command.go`. A `textinput` overlay opened by `:`; on Enter, dispatch `channels`/`devices`/`config`/`operate`/`quit` by pushing/popping views or `tea.Quit`. Root `Update` intercepts `:` (when no overlay open) to open it, routes keys to it while open. Test: feeding `:devices⏎` pushes a `devicesView`.

### Task 12: `?` help overlay
**Files:** `internal/app/help.go`. Use `bubbles/help` + a `help.KeyMap` built from the active view's `Hints()` plus globals; `?` toggles. Test: overlay output contains a global binding label.

### Task 13: `:devices` view
**Files:** `internal/app/view_devices.go`. Read-only `list` of all `ListDevices` results (id, label, capture/playback), refreshed on `DeviceArrived/Departed`. Test: lists a fake device; updates on a `DeviceArrived` event.

---

## Self-review

**Spec coverage** (design §): selectable pickers + Apply-gating ✓(T5/T6, regression test) · windowed layout/header/footer ✓(T2/T3) · resize ✓(T10) · `:`/`?`/`/` nav ✓(T11/T12, list filter built-in) · theme ✓(T1) · Channels ✓(T4) · Configure ✓(T5/T6) · Operate re-housed ✓(T7/T8) · Devices ✓(T13) · error toasts ✓(T9) · preserve client/events ✓(untouched). RX decode stays deferred (design non-goal).

**Type consistency:** `View` = `{Update(tea.Msg)(View,tea.Cmd); Render(w,h int) string; Title() string; Hints() []ui.Hint}` used identically by every `*xView`. `ui.Hint{Key,Action}`, `ui.Frame/Header/Footer/Toast`, `ui.Severity*` referenced consistently. Reuses existing `modeParamsFor`, `channelBoundMsg`/`pttBoundMsg`/`audioCfgMsg`/`devicesMsg`, `rpcCtx`, `enableSpectrumCmd`, the `Fake` client.

**No placeholders:** load-bearing code (theme, Frame/chrome, View+WM, Channels, Configure-fix, Toast) is complete; Tasks 6/8/11–13 specify exact widgets/fields/messages and reuse already-defined types — no "TBD"/"handle errors".

**Phasing:** Phase 1 alone makes configuration work and is shippable.

## Notes for the executor
- `applyEvent` body: copy verbatim from the current `model.go` (unchanged behavior).
- Keep one definition of helpers (`orNone`); move it to `view.go` if `status.go` is removed.
- Bubbles `table`/`list` need `SetWidth/SetHeight`/`SetSize` each render — that's how resize takes effect.
- Run `go vet ./...`; the `tui (go)` CI job is the gate.
