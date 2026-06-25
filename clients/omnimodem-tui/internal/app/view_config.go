package app

import (
	"fmt"
	"strings"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	"github.com/charmbracelet/bubbles/list"
	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

// devEntry is a list.Item (and list.DefaultItem) for a device.
type devEntry struct{ id, label string }

func (d devEntry) Title() string       { return d.label }
func (d devEntry) Description() string { return d.id }
func (d devEntry) FilterValue() string { return d.label + " " + d.id }

// PTT methods offered in the picker.
var pttMethods = []pb.PttMethod{
	pb.PttMethod_PTT_METHOD_VOX,
	pb.PttMethod_PTT_METHOD_SERIAL_RTS,
	pb.PttMethod_PTT_METHOD_SERIAL_DTR,
	pb.PttMethod_PTT_METHOD_CM108,
	pb.PttMethod_PTT_METHOD_GPIO,
	pb.PttMethod_PTT_METHOD_NONE,
}

type cfgFocus int

const (
	fName cfgFocus = iota
	fCall
	fGrid
	fMode
	fRx
	fTx
	fPtt
	fMethod
	fApply
	fLast = fApply
)

type configView struct {
	m         *Model
	name      textinput.Model
	call      textinput.Model
	grid      textinput.Model
	modeIdx   int
	rx        list.Model
	tx        list.Model
	ptt       list.Model
	rxID      string
	txID      string
	pttID     string
	methodIdx int
	focus     cfgFocus
	picking   bool // a device-picker modal is open over the form
}

func newDevList(title string) list.Model {
	// One line per device. Many devices report label == device_id (virtual
	// loopbacks especially), so the default two-line delegate printed each name
	// twice; the id is kept only as filter text, not a second visible row.
	del := list.NewDefaultDelegate()
	del.ShowDescription = false
	del.SetSpacing(0)
	// DOS dialog palette: white rows on the black panel, white-on-blue highlight.
	del.Styles.NormalTitle = del.Styles.NormalTitle.Foreground(ui.ColorFg).Background(ui.ColorPanel)
	del.Styles.DimmedTitle = del.Styles.DimmedTitle.Foreground(ui.ColorDim).Background(ui.ColorPanel)
	del.Styles.SelectedTitle = del.Styles.SelectedTitle.
		Foreground(ui.ColorFg).Background(ui.ColorSel).Bold(true).
		BorderLeftForeground(ui.ColorSel)
	l := list.New(nil, del, 0, 0)
	l.Title = title
	l.SetShowTitle(false)  // the modal frame supplies the title
	l.SetShowHelp(false)
	l.SetShowStatusBar(false)
	return l
}

func newConfigView(m *Model) *configView {
	name := textinput.New()
	name.Focus()
	call := textinput.New()
	call.Placeholder = "N0CALL"
	call.CharLimit = 12
	call.SetValue(m.myCall)
	grid := textinput.New()
	grid.Placeholder = "AA00"
	grid.CharLimit = 8
	grid.SetValue(m.myGrid)
	v := &configView{
		m:    m,
		name: name,
		call: call,
		grid: grid,
		rx:   newDevList("RX device (capture)"),
		tx:   newDevList("TX device (playback)"),
		ptt:  newDevList("PTT device"),
	}
	// Preload the channel's persisted config (surfaced in the snapshot) so
	// reopening Configure shows what's already saved instead of blank defaults.
	if cl := m.live[m.sel]; cl != nil && cl.name != "" {
		v.name.SetValue(cl.name)
		v.modeIdx = modeIdxByLabel(cl.mode)
		v.rxID = cl.deviceID
		v.txID = cl.txDeviceID
		v.pttID = cl.pttDeviceID
		v.methodIdx = methodIdxOf(cl.pttMethod)
	} else {
		v.name.SetValue("vfo-a")
	}
	return v
}

// modeIdxByLabel returns the index of a mode label in `modes`, or 0 when the
// label is unknown/empty (a fresh channel falls back to the first mode).
func modeIdxByLabel(label string) int {
	for i := range modes {
		if modes[i].label == label {
			return i
		}
	}
	return 0
}

// methodIdxOf returns the index of a PTT method in `pttMethods`, or 0 when not
// found (UNSPECIFIED on an unconfigured channel maps to the default method).
func methodIdxOf(m pb.PttMethod) int {
	for i, pm := range pttMethods {
		if pm == m {
			return i
		}
	}
	return 0
}

func (v *configView) modeLabel() string { return modes[v.modeIdx].label }
func (v *configView) method() pb.PttMethod {
	return pttMethods[v.methodIdx]
}

func (v *configView) setDevices(devs []*pb.DeviceInfo) {
	var capItems, playItems, allItems []list.Item
	for _, d := range devs {
		e := devEntry{id: d.GetDeviceId(), label: d.GetLabel()}
		if d.GetHasCapture() {
			capItems = append(capItems, e)
		}
		if d.GetHasPlayback() {
			playItems = append(playItems, e)
		}
		allItems = append(allItems, e)
	}
	v.rx.SetItems(capItems)
	v.tx.SetItems(playItems)
	v.ptt.SetItems(allItems)
}

func (v *configView) canApply() bool { return v.rxID != "" }

// --- bind pipeline: ConfigureChannel → ConfigureAudio → ConfigurePtt ---

func (v *configView) apply() tea.Cmd {
	req := &pb.ConfigureChannelRequest{
		Channel:    v.m.sel,
		Name:       v.name.Value(),
		Mode:       v.modeLabel(),
		ModeParams: modeParamsFor(v.modeLabel(), nil),
	}
	c := v.m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.ConfigureChannel(ctx, req); err != nil {
			return rpcErrMsg{fmt.Errorf("configure channel: %w", err)}
		}
		return channelBoundMsg{}
	}
}

func (v *configView) afterChannel() tea.Cmd {
	req := &pb.ConfigureAudioRequest{
		Channel: v.m.sel, DeviceId: v.rxID, SampleRate: 48000, TxDeviceId: v.txID,
	}
	c := v.m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.ConfigureAudio(ctx, req)
		if err != nil {
			return rpcErrMsg{fmt.Errorf("configure audio: %w", err)}
		}
		return audioCfgMsg{resp}
	}
}

func (v *configView) afterAudio() tea.Cmd {
	req := &pb.ConfigurePttRequest{Channel: v.m.sel, DeviceId: v.pttID, Method: v.method()}
	c := v.m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.ConfigurePtt(ctx, req); err != nil {
			return rpcErrMsg{fmt.Errorf("configure ptt: %w", err)}
		}
		return pttBoundMsg{}
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
		// tx_rate == 0 means the daemon bound the channel RX-only: the TX device
		// had no usable playback (an input-only device, or TX left defaulting to
		// the capture device). Transmit will be silent until a real output device
		// is picked for TX — surface that instead of letting it fail quietly.
		if msg.resp.GetActualTxSampleRate() == 0 {
			v.m.toast = ui.NewToast(
				"Bound RX-only — no usable TX device; transmit is silent. Pick an output device for TX.",
				ui.SeverityWarn)
		}
		return v, v.afterAudio()
	case pttBoundMsg:
		v.m.pop() // bind complete
		return v, snapshotCmd(v.m.c)
	case tea.KeyMsg:
		// A device-picker modal is open: it captures all keys. Enter records the
		// highlighted device and closes the modal; esc cancels and closes. While
		// the list's own filter is active, hand keys straight to it (so esc clears
		// the filter and enter applies it, rather than closing the modal).
		if v.picking {
			lst, _ := v.activeList()
			if lst.FilterState() == list.Filtering {
				var cmd tea.Cmd
				*lst, cmd = lst.Update(msg)
				return v, cmd
			}
			switch msg.String() {
			case "esc":
				v.picking = false
				return v, nil
			case "enter", " ":
				v.choose()
				v.picking = false
				return v, nil
			}
			var cmd tea.Cmd
			*lst, cmd = lst.Update(msg)
			return v, cmd
		}
		// Form navigation (no modal open).
		switch msg.String() {
		case "esc":
			v.m.pop() // cancel configuration, back to Channels
			return v, nil
		case "tab", "down":
			if v.focus < fLast {
				v.focus++
			}
			v.syncFocus()
			return v, nil
		case "shift+tab", "up":
			if v.focus > fName {
				v.focus--
			}
			v.syncFocus()
			return v, nil
		}
		// Text fields take every other key (so values may hold 'a', spaces, etc.);
		// ←/→ move the cursor. Call/Grid keep the operator station identity in sync.
		switch v.focus {
		case fName:
			var cmd tea.Cmd
			v.name, cmd = v.name.Update(msg)
			return v, cmd
		case fCall:
			var cmd tea.Cmd
			v.call, cmd = v.call.Update(msg)
			v.m.myCall = strings.ToUpper(strings.TrimSpace(v.call.Value()))
			return v, cmd
		case fGrid:
			var cmd tea.Cmd
			v.grid, cmd = v.grid.Update(msg)
			v.m.myGrid = strings.ToUpper(strings.TrimSpace(v.grid.Value()))
			return v, cmd
		}
		// Selector/device/apply fields: cycle, open the picker, or apply.
		switch msg.String() {
		case "left":
			v.cycle(-1)
		case "right":
			v.cycle(+1)
		case "enter", " ":
			return v.commit()
		case "a":
			if v.canApply() {
				return v, v.apply()
			}
			v.m.toast = ui.NewToast("pick an RX device first", ui.SeverityWarn)
		}
		return v, nil
	}
	return v, nil
}

// activeList is the device list for the field being picked (the focused device
// field). Returns a pointer so the modal can drive it in place.
func (v *configView) activeList() (*list.Model, string) {
	switch v.focus {
	case fTx:
		return &v.tx, "TX device (playback)"
	case fPtt:
		return &v.ptt, "PTT device"
	default:
		return &v.rx, "RX device (capture)"
	}
}

// syncFocus mirrors the focus index into the text inputs' blink state.
func (v *configView) syncFocus() {
	v.name.Blur()
	v.call.Blur()
	v.grid.Blur()
	switch v.focus {
	case fName:
		v.name.Focus()
	case fCall:
		v.call.Focus()
	case fGrid:
		v.grid.Focus()
	}
}

// cycle steps the mode or method selector when one is focused.
func (v *configView) cycle(d int) {
	switch v.focus {
	case fMode:
		v.modeIdx = (v.modeIdx + d + len(modes)) % len(modes)
	case fMethod:
		v.methodIdx = (v.methodIdx + d + len(pttMethods)) % len(pttMethods)
	}
}

// commit reacts to Enter on the focused field: open the device picker on a
// device field, apply on the Apply field. Mode/method use ←/→, not Enter.
func (v *configView) commit() (View, tea.Cmd) {
	switch v.focus {
	case fRx, fTx, fPtt:
		v.picking = true
	case fApply:
		if v.canApply() {
			return v, v.apply()
		}
		v.m.toast = ui.NewToast("pick an RX device first", ui.SeverityWarn)
	}
	return v, nil
}

// choose records the highlighted device in the open picker into the chosen id.
func (v *configView) choose() {
	switch v.focus {
	case fRx:
		if it, ok := v.rx.SelectedItem().(devEntry); ok {
			v.rxID = it.id
		}
	case fTx:
		if it, ok := v.tx.SelectedItem().(devEntry); ok {
			v.txID = it.id
		}
	case fPtt:
		if it, ok := v.ptt.SelectedItem().(devEntry); ok {
			v.pttID = it.id
		}
	}
}

func (v *configView) Render(w, h int) string {
	mark := func(f cfgFocus, s string) string {
		if v.focus == f {
			return ui.Accent.Render("▸ " + s)
		}
		return "  " + s
	}
	chosen := func(id string) string {
		if id == "" {
			return ui.Dim.Render("(none)")
		}
		return ui.Accent.Render("✓ " + id)
	}
	var b strings.Builder
	b.WriteString(mark(fName, "Name    "+v.name.View()) + "\n")
	b.WriteString(mark(fCall, "Call    "+v.call.View()) + "\n")
	b.WriteString(mark(fGrid, "Grid    "+v.grid.View()) + "\n")
	b.WriteString(mark(fMode, "Mode    ‹ "+v.modeLabel()+" ›  (←/→)") + "\n")
	b.WriteString(mark(fRx, "RX dev  "+chosen(v.rxID)) + "\n")
	b.WriteString(mark(fTx, "TX dev  "+chosenOrSame(v.txID)) + "\n")
	b.WriteString(mark(fPtt, "PTT dev "+chosen(v.pttID)) + "   " +
		mark(fMethod, "method ‹ "+methodLabel(v.method())+" › (←/→)") + "\n")
	b.WriteString(mark(fApply, "Apply   "+applyHint(v.canApply())) + "\n")

	// The device picker is a modal: it appears only while a device field is being
	// chosen, and disappears once a device is selected (or the pick is cancelled).
	if !v.picking {
		b.WriteString("\n" + ui.Dim.Render("‹↑/↓› field · ‹←/→› cycle · ‹enter› pick/apply · ‹a› apply · ‹esc› cancel"))
		return b.String()
	}
	lst, label := v.activeList()
	modalW := w
	if modalW > 64 {
		modalW = 64
	}
	// Hug the device count so the box doesn't sprawl with empty rows, but cap to
	// the space available below the form.
	listH := len(lst.Items())
	if listH < 3 {
		listH = 3
	}
	if max := h - 9; max >= 3 && listH > max {
		listH = max
	}
	lst.SetSize(modalW-4, listH)
	box := ui.Modal(label+"  ‹enter› choose · ‹esc› cancel · ‹/› filter", lst.View(), modalW)
	b.WriteString("\n" + lipgloss.PlaceHorizontal(w, lipgloss.Center, box))
	return b.String()
}

func chosenOrSame(id string) string {
	if id == "" {
		return ui.Dim.Render("(same as RX)")
	}
	return ui.Accent.Render("✓ " + id)
}

func (v *configView) Title() string { return fmt.Sprintf("Configure ch%d", v.m.sel) }

func (v *configView) Hints() []ui.Hint {
	if v.picking {
		return []ui.Hint{
			{Key: "↑/↓", Action: "device"}, {Key: "enter", Action: "choose"},
			{Key: "/", Action: "filter"}, {Key: "esc", Action: "cancel"},
		}
	}
	return []ui.Hint{
		{Key: "↑/↓", Action: "field"}, {Key: "←/→", Action: "cycle"}, {Key: "enter", Action: "pick/apply"},
		{Key: "a", Action: "apply"}, {Key: "esc", Action: "cancel"},
	}
}

func applyHint(ok bool) string {
	if ok {
		return "↵"
	}
	return "(pick RX device)"
}

func methodLabel(m pb.PttMethod) string {
	return strings.TrimPrefix(m.String(), "PTT_METHOD_")
}
