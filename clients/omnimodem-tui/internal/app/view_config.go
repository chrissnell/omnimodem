package app

import (
	"fmt"
	"strings"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
	"github.com/charmbracelet/bubbles/list"
	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
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
	modeIdx   int
	rx        list.Model
	tx        list.Model
	ptt       list.Model
	rxID      string
	txID      string
	pttID     string
	methodIdx int
	focus     cfgFocus
}

func newDevList(title string) list.Model {
	l := list.New(nil, list.NewDefaultDelegate(), 0, 0)
	l.Title = title
	l.SetShowHelp(false)
	l.SetShowStatusBar(false)
	return l
}

func newConfigView(m *Model) *configView {
	name := textinput.New()
	name.SetValue("vfo-a")
	name.Focus()
	return &configView{
		m:    m,
		name: name,
		rx:   newDevList("RX device (capture)"),
		tx:   newDevList("TX device (playback)"),
		ptt:  newDevList("PTT device"),
	}
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
		return v, v.afterAudio()
	case pttBoundMsg:
		v.m.pop() // bind complete
		return v, snapshotCmd(v.m.c)
	case tea.KeyMsg:
		// Tab/Shift-Tab always traverse fields.
		switch msg.String() {
		case "tab":
			if v.focus < fLast {
				v.focus++
			}
			v.syncFocus()
			return v, nil
		case "shift+tab":
			if v.focus > fName {
				v.focus--
			}
			v.syncFocus()
			return v, nil
		}
		// When the name field is focused, every other key is text input — so the
		// name can contain 'a', spaces, etc. (form-action keys are off-limits here).
		if v.focus == fName {
			var cmd tea.Cmd
			v.name, cmd = v.name.Update(msg)
			return v, cmd
		}
		// Non-name fields: form-action keys, then route to the focused widget.
		switch msg.String() {
		case "left":
			v.cycle(-1)
			return v, nil
		case "right":
			v.cycle(+1)
			return v, nil
		case "enter", " ":
			return v.commit()
		case "a":
			if v.canApply() {
				return v, v.apply()
			}
			v.m.toast = ui.NewToast("pick an RX device first", ui.SeverityWarn)
			return v, nil
		}
	}
	// route to the focused list (cursor moves, '/' filter)
	var cmd tea.Cmd
	switch v.focus {
	case fRx:
		v.rx, cmd = v.rx.Update(msg)
	case fTx:
		v.tx, cmd = v.tx.Update(msg)
	case fPtt:
		v.ptt, cmd = v.ptt.Update(msg)
	}
	return v, cmd
}

// activeList is the device list shown/driven for the current focus (RX by
// default, so the user always sees a list to pick from).
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

// syncFocus mirrors the focus index into the name textinput's blink state.
func (v *configView) syncFocus() {
	if v.focus == fName {
		v.name.Focus()
	} else {
		v.name.Blur()
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

// commit records a list highlight into the chosen id (or applies on the Apply field).
func (v *configView) commit() (View, tea.Cmd) {
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
	case fApply:
		if v.canApply() {
			return v, v.apply()
		}
		v.m.toast = ui.NewToast("pick an RX device first", ui.SeverityWarn)
	}
	return v, nil
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
	b.WriteString(mark(fMode, "Mode    ‹ "+v.modeLabel()+" ›  (←/→)") + "\n")
	b.WriteString(mark(fRx, "RX dev  "+chosen(v.rxID)) + "\n")
	b.WriteString(mark(fTx, "TX dev  "+chosenOrSame(v.txID)) + "\n")
	b.WriteString(mark(fPtt, "PTT dev "+chosen(v.pttID)) + "   " +
		mark(fMethod, "method ‹ "+methodLabel(v.method())+" › (←/→)") + "\n")
	b.WriteString(mark(fApply, "Apply   "+applyHint(v.canApply())) + "\n\n")

	// Show the device list for whichever device field is focused (RX by default).
	lst, label := v.activeList()
	listH := h - 9
	if listH < 3 {
		listH = 3
	}
	lst.SetSize(w, listH)
	b.WriteString(ui.Dim.Render(label+" — <enter> choose · </> filter") + "\n")
	b.WriteString(lst.View())
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
	return []ui.Hint{
		{Key: "tab", Action: "field"}, {Key: "←/→", Action: "cycle"}, {Key: "enter", Action: "select"},
		{Key: "/", Action: "filter"}, {Key: "a", Action: "apply"}, {Key: "esc", Action: "cancel"},
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
