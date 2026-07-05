package app

import (
	"fmt"
	"sort"
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
	fSettings
	fRx
	fTx
	fPtt
	fMethod
	fRsidTx
	fRsidRx
	fLast = fRsidRx
)

// cfgSig is the persistable slice of the form. It drives change detection so
// auto-apply fires only when a field actually changed, not on mere navigation.
type cfgSig struct {
	name      string
	modeIdx   int
	modeSig   string // the current mode's settings, serialized for change detection
	rxID      string
	txID      string
	pttID     string
	methodIdx int
	rsidTx    bool
	rsidRx    bool
}

type configView struct {
	m         *Model
	name      textinput.Model
	call      textinput.Model
	grid      textinput.Model
	modeIdx   int
	settings  *ui.SettingsForm // the current mode's editable settings
	rx        list.Model
	tx        list.Model
	ptt       list.Model
	rxID      string
	txID      string
	pttID     string
	methodIdx int
	rsidTx    bool // prepend the mode's RSID burst before each TX
	rsidRx    bool // run the RSID detector over received audio
	focus     cfgFocus
	picking   bool   // a device-picker modal is open over the form
	editing   bool   // the mode-settings modal is open over the form
	saved     cfgSig // last state CONFIRMED persisted to the daemon
	applying  bool   // a save pipeline is in flight (serializes auto-apply)
	inflight  cfgSig // the sig the in-flight pipeline is persisting
	closing   bool   // esc pressed; pop once the save fully drains
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
		v.rsidTx = cl.rsidTx
		v.rsidRx = cl.rsidRx
	} else {
		v.name.SetValue(defaultChannelName(m))
	}
	// Build the settings form for the preloaded mode so its params are ready to
	// edit and to seed the change-detection baseline. It opens at the mode's
	// DEFAULTS, not the saved values: the snapshot's ChannelInfo carries the mode
	// label but not its typed ModeParams, so there's nothing to reload yet. Until
	// ChannelInfo gains mode_params (daemon-side change), edited settings persist
	// within a session but revert to defaults on reopen — see GRA follow-up. Do
	// not "fix" this by seeding from stale local state; seed from the daemon once
	// it reports the saved params.
	v.settings = newModeSettingsForm(v.modeLabel(), nil)
	// Seed the change-detection baseline with the preloaded values so opening
	// the form doesn't spuriously re-persist what's already saved.
	v.saved = v.sig()
	return v
}

// rebuildSettings swaps in a fresh settings form for the current mode. Called
// when the mode changes, so the form always matches the selected mode's params.
func (v *configView) rebuildSettings() {
	v.settings = newModeSettingsForm(v.modeLabel(), nil)
}

// defaultChannelName picks the first "VFO-<letter>" not already taken by another
// channel, so a freshly added channel doesn't default to a name that's already in
// use (e.g. a second channel becomes VFO-B, not another VFO-A). Comparison is
// case-insensitive so a legacy lowercase "vfo-a" still claims the A slot; the
// returned name is upper-cased by convention.
func defaultChannelName(m *Model) string {
	used := make(map[string]bool, len(m.live))
	for _, cl := range m.live {
		if cl.name != "" {
			used[strings.ToUpper(cl.name)] = true
		}
	}
	for c := 'A'; c <= 'Z'; c++ {
		if name := "VFO-" + string(c); !used[name] {
			return name
		}
	}
	// 26 VFO letters exhausted: fall back to a channel-scoped name that's still
	// unique across ids.
	return fmt.Sprintf("VFO-%d", m.sel)
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

// sig snapshots the persistable form fields for change detection.
func (v *configView) sig() cfgSig {
	return cfgSig{
		name:      v.name.Value(),
		modeIdx:   v.modeIdx,
		modeSig:   v.modeSig(),
		rxID:      v.rxID,
		txID:      v.txID,
		pttID:     v.pttID,
		methodIdx: v.methodIdx,
		rsidTx:    v.rsidTx,
		rsidRx:    v.rsidRx,
	}
}

// modeSig serializes the current mode's settings into a stable string so a
// changed knob (e.g. RTTY shift) is detected and auto-applied like any other
// field. Keys are sorted for determinism.
func (v *configView) modeSig() string {
	if v.settings == nil {
		return ""
	}
	vals := v.settings.Values()
	keys := make([]string, 0, len(vals))
	for k := range vals {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var b strings.Builder
	for _, k := range keys {
		b.WriteString(k)
		b.WriteByte('=')
		b.WriteString(vals[k])
		b.WriteByte(';')
	}
	return b.String()
}

// maybePersist auto-applies the form when a field has changed since the last
// confirmed save. It is the replacement for the old Apply button: callers invoke
// it after any mutating action (cycling a selector, choosing a device, leaving
// the name field). Persisting the whole channel on each change re-drives the
// daemon's channel→audio→ptt rebind, so a mode switch takes effect on the live
// workers and every device choice (RX, TX, PTT) is saved together.
//
// It is a no-op when nothing changed, when no RX device is chosen yet (audio
// can't bind without a capture device), or when the name is empty (the daemon
// rejects it). It also serializes: while one pipeline is in flight it starts no
// second one (the completion handler re-checks and coalesces to the latest
// state). Serializing avoids two ConfigureChannel RPCs racing to the daemon out
// of order — the bind commands run in independent goroutines with no ordering
// guarantee — which could otherwise persist a stale mode.
func (v *configView) maybePersist() tea.Cmd {
	if v.applying {
		return nil // in flight; pttBoundMsg/rpcErrMsg will re-check
	}
	cur := v.sig()
	if cur == v.saved || !v.canApply() || v.name.Value() == "" {
		return nil
	}
	// v.saved is advanced only once the pipeline confirms (pttBoundMsg), so a
	// failed save is retried on the next change rather than silently believed
	// persisted.
	v.applying = true
	v.inflight = cur
	return v.persistAll()
}

// persistAll runs the whole ConfigureChannel → ConfigureAudio → ConfigurePtt
// save as ONE self-contained command: all field values are captured up front
// and the three RPCs run sequentially in a single goroutine, returning one
// saveDoneMsg. Keeping the pipeline in one command (rather than chained across
// view-routed messages) means the save no longer depends on the config view
// staying on the view stack — so leaving with <esc> mid-save still persists TX
// and PTT, not just RX. The captured values also match v.inflight exactly, so
// the confirmed-save baseline is deterministic.
//
// Caller guarantees (via maybePersist) a chosen RX device and a non-empty name,
// which are the daemon's preconditions for ConfigureChannel/ConfigureAudio.
func (v *configView) persistAll() tea.Cmd {
	ch := v.m.sel
	c := v.m.c
	chReq := &pb.ConfigureChannelRequest{
		Channel:    ch,
		Name:       v.name.Value(),
		Mode:       v.modeLabel(),
		ModeParams: modeParamsFor(v.modeLabel(), modeValsFrom(v.settings)),
		RsidTx:     v.rsidTx,
		RsidRx:     v.rsidRx,
	}
	audioReq := &pb.ConfigureAudioRequest{
		Channel: ch, DeviceId: v.rxID, SampleRate: 48000, TxDeviceId: v.txID,
	}
	pttReq := &pb.ConfigurePttRequest{Channel: ch, DeviceId: v.pttID, Method: v.method()}
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.ConfigureChannel(ctx, chReq); err != nil {
			return saveDoneMsg{err: fmt.Errorf("configure channel: %w", err)}
		}
		resp, err := c.ConfigureAudio(ctx, audioReq)
		if err != nil {
			return saveDoneMsg{err: fmt.Errorf("configure audio: %w", err)}
		}
		if err := c.ConfigurePtt(ctx, pttReq); err != nil {
			return saveDoneMsg{err: fmt.Errorf("configure ptt: %w", err)}
		}
		// tx_rate == 0 means the daemon bound the channel RX-only: the TX device
		// had no usable playback. Transmit stays silent until a real output
		// device is picked — surface that rather than failing quietly.
		return saveDoneMsg{warnRxOnly: resp.GetActualTxSampleRate() == 0}
	}
}

func (v *configView) Update(msg tea.Msg) (View, tea.Cmd) {
	switch msg := msg.(type) {
	case devicesMsg:
		v.setDevices(msg.devices)
		return v, nil
	case saveDoneMsg:
		v.applying = false
		if msg.err != nil {
			// The save failed. Show the error and do NOT advance v.saved, so the
			// change is retried on the next edit; don't auto-retry here (avoid
			// spinning on a hard failure). If the user already asked to leave,
			// honor that — we tried, and the toast explains what happened.
			v.m.toast = ui.NewToast(msg.err.Error(), ui.SeverityError)
			if v.closing {
				v.m.pop()
			}
			// Still refresh live state: a save can fail AFTER the daemon already
			// committed part of it — a PTT config persists before its driver is
			// opened, so a device-based method with no usable node reports an error
			// yet the device choice is saved. Reflect the daemon (the source of
			// truth) so reopening Configure shows what actually persisted.
			return v, snapshotCmd(v.m.c)
		}
		// Confirmed: advance the baseline to exactly what this save persisted.
		v.saved = v.inflight
		if msg.warnRxOnly {
			v.m.toast = ui.NewToast(
				"Bound RX-only — no usable TX device; transmit is silent. Pick an output device for TX.",
				ui.SeverityWarn)
		}
		// Coalesce any change that arrived while this save was in flight (e.g.
		// TX/PTT picked during an earlier RX save). Serializing this way keeps
		// the RPCs ordered and lets a pending change ride out even after esc.
		if cmd := v.maybePersist(); cmd != nil {
			return v, cmd
		}
		// Fully drained. If the user pressed esc mid-save, leave now — but still
		// refresh live state on the way out, so reopening Configure preloads the
		// devices just saved (m.live is only repopulated by a GetState; the
		// ChannelConfigured event carries no device fields).
		if v.closing {
			v.m.pop()
			return v, snapshotCmd(v.m.c)
		}
		// Refresh live state so the channel list underneath reflects the save.
		return v, snapshotCmd(v.m.c)
	case tea.KeyMsg:
		// The mode-settings modal is open: it captures all keys. Esc closes it and
		// flushes any change through the auto-apply pipeline; every other key drives
		// the settings form, and a real edit auto-applies immediately.
		if v.editing {
			if msg.String() == "esc" {
				v.editing = false
				return v, v.maybePersist()
			}
			changed, cmd := v.settings.Update(msg)
			if changed {
				if pc := v.maybePersist(); pc != nil {
					return v, tea.Batch(cmd, pc)
				}
			}
			return v, cmd
		}
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
				return v, v.maybePersist() // auto-apply the device choice
			}
			var cmd tea.Cmd
			*lst, cmd = lst.Update(msg)
			return v, cmd
		}
		// Form navigation (no modal open).
		switch msg.String() {
		case "esc":
			// Leave, but not before every change is durably saved. Mark closing
			// and let the save drain: if there's an unsaved change and nothing is
			// in flight, maybePersist starts the final save and the saveDoneMsg
			// handler pops when done. If a save is already in flight (possibly with
			// TX/PTT picked while it ran), stay until it completes and coalesces
			// the rest — then it pops. Only when there's nothing to save and
			// nothing in flight do we pop immediately. This is what makes a quick
			// "pick devices, hit esc" persist all of RX, TX, and PTT.
			v.closing = true
			if cmd := v.maybePersist(); cmd != nil {
				return v, cmd
			}
			if v.applying {
				return v, nil
			}
			v.m.pop()
			return v, nil
		case "tab", "down":
			if v.focus < fLast {
				v.focus++
			}
			v.syncFocus()
			// Commit-on-blur: a name edit persists when focus moves off it.
			return v, v.maybePersist()
		case "shift+tab", "up":
			if v.focus > fName {
				v.focus--
			}
			v.syncFocus()
			return v, v.maybePersist()
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
		// Selector/device fields: cycle a selector or open the device picker.
		// Changes auto-apply — there is no explicit Apply key.
		switch msg.String() {
		case "left":
			v.cycle(-1)
			return v, v.maybePersist()
		case "right":
			v.cycle(+1)
			return v, v.maybePersist()
		case "enter", " ":
			return v.commit()
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
		v.rebuildSettings() // the new mode exposes a different set of settings
	case fMethod:
		v.methodIdx = (v.methodIdx + d + len(pttMethods)) % len(pttMethods)
	case fRsidTx:
		v.rsidTx = !v.rsidTx // boolean toggle; direction is irrelevant
	case fRsidRx:
		v.rsidRx = !v.rsidRx
	}
}

// commit reacts to Enter on the focused field: open the device picker on a
// device field. Mode/method use ←/→, not Enter; other fields ignore Enter.
func (v *configView) commit() (View, tea.Cmd) {
	switch v.focus {
	case fRx, fTx, fPtt:
		v.picking = true
	case fSettings:
		// Open the mode-settings editor, unless the mode has nothing to tune.
		if v.settings != nil && v.settings.HasFields() {
			v.editing = true
		}
	case fRsidTx:
		v.rsidTx = !v.rsidTx
		return v, v.maybePersist()
	case fRsidRx:
		v.rsidRx = !v.rsidRx
		return v, v.maybePersist()
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
	chosen := func(id string) string {
		if id == "" {
			return ui.Dim.Render("(none)")
		}
		return ui.Accent.Render("✓ " + id)
	}
	field := func(f cfgFocus, label, val string) string {
		row := fmt.Sprintf("%-10s %s", label, val)
		if v.focus == f {
			return ui.Accent.Render("▸ " + row)
		}
		return "  " + row
	}
	cyc := "  " + ui.Dim.Render("(←/→)")

	var b strings.Builder
	b.WriteString(ui.Title.Render("STATION") + "\n")
	b.WriteString(field(fName, "Name", v.name.View()) + "\n")
	b.WriteString(field(fCall, "Call", v.call.View()) + "\n")
	b.WriteString(field(fGrid, "Grid", v.grid.View()) + "\n\n")

	b.WriteString(ui.Title.Render("MODE") + "\n")
	b.WriteString(field(fMode, "Mode", "‹ "+v.modeLabel()+" ›"+cyc) + "\n")
	b.WriteString(field(fSettings, "Settings", v.settingsSummary()) + "\n\n")

	b.WriteString(ui.Title.Render("AUDIO") + "\n")
	b.WriteString(field(fRx, "RX Device", chosen(v.rxID)) + "\n")
	b.WriteString(field(fTx, "TX Device", txDeviceLabel(v.txID, v.rxID)) + "\n")
	b.WriteString(field(fPtt, "PTT Device", chosen(v.pttID)) + "\n")
	b.WriteString(field(fMethod, "PTT Method", "‹ "+methodLabel(v.method())+" ›"+cyc) + "\n\n")

	onOff := func(b bool) string {
		if b {
			return ui.Accent.Render("on")
		}
		return ui.Dim.Render("off")
	}
	b.WriteString(ui.Title.Render("RSID") + "\n")
	b.WriteString(field(fRsidTx, "TX ident", onOff(v.rsidTx)+"  "+ui.Dim.Render("(space)")) + "\n")
	b.WriteString(field(fRsidRx, "RX detect", onOff(v.rsidRx)+"  "+ui.Dim.Render("(space)")) + "\n\n")

	b.WriteString(v.saveHint() + "\n")

	// The mode-settings editor is a modal over the form: it surfaces every knob
	// the selected mode exposes, drawn by the reusable ui.SettingsForm.
	if v.editing {
		modalW := w
		if modalW > 64 {
			modalW = 64
		}
		body := v.settings.View(modalW - 4)
		box := ui.Modal(
			v.modeLabel()+" settings  ‹↑/↓› field · ‹←/→/space› change · ‹esc› done",
			body, modalW)
		b.WriteString("\n" + lipgloss.PlaceHorizontal(w, lipgloss.Center, box))
		return b.String()
	}
	// The device picker is a modal: it appears only while a device field is being
	// chosen, and disappears once a device is selected (or the pick is cancelled).
	if !v.picking {
		b.WriteString("\n" + ui.Dim.Render("‹↑/↓› field · ‹←/→› cycle · ‹enter› pick device · ‹esc› done"))
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

// txDeviceLabel renders the TX device field. An empty txID means "TX follows the
// RX device" (single-rig default, and how the daemon reports TX when it mirrors
// RX). Show the effective device — the RX id — with a "(same as RX)" note rather
// than a bare "(same as RX)", which reads as "my TX choice wasn't saved" when the
// operator deliberately picked the same device (e.g. one BlackHole for both).
func txDeviceLabel(txID, rxID string) string {
	if txID != "" {
		return ui.Accent.Render("✓ " + txID)
	}
	if rxID != "" {
		return ui.Accent.Render("✓ "+rxID) + ui.Dim.Render("  (same as RX)")
	}
	return ui.Dim.Render("(same as RX)")
}

// settingsSummary renders the Settings row's value: a one-line digest of the
// mode's current knobs, with an ‹enter› cue when there's something to open.
func (v *configView) settingsSummary() string {
	sum := modeSettingsSummary(v.modeLabel(), v.settings)
	if v.settings != nil && v.settings.HasFields() {
		return sum + "  " + ui.Dim.Render("‹enter›")
	}
	return ui.Dim.Render(sum)
}

func (v *configView) Title() string { return fmt.Sprintf("Configure ch%d", v.m.sel) }

func (v *configView) Hints() []ui.Hint {
	if v.editing {
		return []ui.Hint{
			{Key: "↑/↓", Action: "field"}, {Key: "←/→", Action: "change"},
			{Key: "space", Action: "toggle"}, {Key: "esc", Action: "done"},
		}
	}
	if v.picking {
		return []ui.Hint{
			{Key: "↑/↓", Action: "device"}, {Key: "enter", Action: "choose"},
			{Key: "/", Action: "filter"}, {Key: "esc", Action: "cancel"},
		}
	}
	return []ui.Hint{
		{Key: "↑/↓", Action: "field"}, {Key: "←/→", Action: "cycle"}, {Key: "enter", Action: "pick"},
		{Key: "esc", Action: "done"},
	}
}

// saveHint replaces the old Apply button: fields auto-apply on change, so this
// just states the auto-save behaviour — or, until an RX device is chosen (the
// prerequisite for binding audio), tells the operator nothing will save yet.
func (v *configView) saveHint() string {
	if !v.canApply() {
		return "  " + ui.Dim.Render("Pick an RX device to save this channel")
	}
	return "  " + ui.Dim.Render("Changes save automatically")
}

func methodLabel(m pb.PttMethod) string {
	return strings.TrimPrefix(m.String(), "PTT_METHOD_")
}
