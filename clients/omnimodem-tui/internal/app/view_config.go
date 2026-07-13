package app

import (
	"fmt"
	"sort"
	"strconv"
	"strings"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

// devEntry is one enumerated audio device: its id, display label, and I/O caps.
// needsSetup marks a device present but not yet usable until an OS-level setup
// step runs (Linux DVB driver bound, Windows without WinUSB — run Zadig).
type devEntry struct {
	id, label         string
	capture, playback bool
	needsSetup        bool
}

// pickerKind is which pop-up chooser is open over the Configure form. All three
// share one scrolling, filterable table modal; they differ only in the rows they
// list and what choosing one does.
type pickerKind int

const (
	pickNone pickerKind = iota
	pickDevice
	pickFamily
	pickMode
)

// pickerRow is one selectable line in a picker modal: the table cells to show,
// the lowercased text the '/' filter matches against, and the action to run when
// it's chosen.
type pickerRow struct {
	cells  []string
	match  string
	choose func(v *configView)
}

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
	fFamily
	fMode
	fSettings
	fRx
	fTx
	fPtt
	fMethod
	fTxDelay
	fTxTail
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
	txDelayMs string
	txTailMs  string
}

type configView struct {
	m         *Model
	name      textinput.Model
	call      textinput.Model
	grid      textinput.Model
	familyIdx int              // index into `families`; the selected mode family
	modeIdx   int              // index into `modes`; the specific submode within that family
	settings  *ui.SettingsForm // the current mode's editable settings
	devs      []devEntry       // all enumerated devices (capability-flagged)
	picker    pickerKind       // which pop-up picker is open (pickNone = closed)
	pickIdx   int              // highlighted row in the open picker
	filter    textinput.Model  // '/' substring filter over the picker
	filtering bool             // the picker's filter input has focus
	rxID      string
	txID      string
	pttID     string
	methodIdx int
	rsidTx    bool // prepend the mode's RSID burst before each TX
	rsidRx    bool // run the RSID detector over received audio
	txDelay   textinput.Model
	txTail    textinput.Model
	focus     cfgFocus
	editing   bool   // the mode-settings modal is open over the form
	saved     cfgSig // last state CONFIRMED persisted to the daemon
	applying  bool   // a save pipeline is in flight (serializes auto-apply)
	inflight  cfgSig // the sig the in-flight pipeline is persisting
	// The mode + settings the in-flight pipeline is persisting, cached on the Model
	// once the save confirms so a reopen shows the saved values.
	inflightModeLabel string
	inflightModeVals  map[string]float64
	closing           bool // esc pressed; pop once the save fully drains
}

// newMsInput builds a small numeric text input for a millisecond timing value.
// The validator rejects any non-digit so the field only ever holds a parseable
// unsigned integer (empty is allowed mid-edit and treated as 0 on save).
func newMsInput(def string) textinput.Model {
	ti := textinput.New()
	ti.CharLimit = 5 // up to 65535 ms is plenty of lead-in/tail
	ti.Placeholder = def
	ti.SetValue(def)
	ti.Validate = func(s string) error {
		for _, r := range s {
			if r < '0' || r > '9' {
				return fmt.Errorf("digits only")
			}
		}
		return nil
	}
	return ti
}

// parseMs turns a millisecond text field into a uint32, treating empty or
// out-of-range input as 0 (the field validator already blocks non-digits).
func parseMs(s string) uint32 {
	if s == "" {
		return 0
	}
	n, err := strconv.ParseUint(s, 10, 32)
	if err != nil {
		return 0
	}
	return uint32(n)
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
	txDelay := newMsInput("300")
	txTail := newMsInput("50")
	filter := textinput.New()
	filter.Placeholder = "filter…"
	filter.Prompt = ""
	// Paint every text input on the panel background so the prompt/value/cursor
	// don't flash the terminal's own (grey) background inside the black cards.
	for _, ti := range []*textinput.Model{&name, &call, &grid, &txDelay, &txTail, &filter} {
		ti.PromptStyle = ti.PromptStyle.Background(ui.ColorPanel)
		ti.TextStyle = ti.TextStyle.Background(ui.ColorPanel)
		ti.PlaceholderStyle = ti.PlaceholderStyle.Background(ui.ColorPanel)
		ti.Cursor.Style = ti.Cursor.Style.Background(ui.ColorPanel)
		ti.Cursor.TextStyle = ti.Cursor.TextStyle.Background(ui.ColorPanel)
	}
	v := &configView{
		m:       m,
		name:    name,
		call:    call,
		grid:    grid,
		txDelay: txDelay,
		txTail:  txTail,
		filter:  filter,
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
		v.txDelay.SetValue(strconv.FormatUint(uint64(cl.pttTxDelayMs), 10))
		v.txTail.SetValue(strconv.FormatUint(uint64(cl.pttTxTailMs), 10))
	} else {
		v.name.SetValue(defaultChannelName(m))
	}
	// Point the family selector at whichever family owns the preloaded mode so the
	// cascading Family→Mode pair opens consistent with the saved submode.
	v.familyIdx = familyIdxOfMode(v.modeIdx)
	// Build the settings form for the preloaded mode, seeded from the last values
	// saved this session (see buildSettings), so reopening Configure shows what was
	// just set rather than mode defaults.
	v.buildSettings()
	// Seed the change-detection baseline with the preloaded values so opening
	// the form doesn't spuriously re-persist what's already saved.
	v.saved = v.sig()
	return v
}

// buildSettings swaps in a fresh settings form for the current mode, seeded from
// the channel's last-saved values when the cached mode matches. The daemon
// doesn't report saved ModeParams in the snapshot (ChannelInfo carries only the
// mode label — GRA-281), so this session cache is what makes an edited setting
// survive closing and reopening the Configure screen. A full app restart still
// falls back to defaults until the daemon surfaces the saved params.
func (v *configView) buildSettings() {
	label := v.modeLabel()
	var initial map[string]float64
	if sp, ok := v.m.modeParams[v.m.sel]; ok && sp.label == label {
		initial = sp.vals
	}
	v.settings = newModeSettingsForm(label, initial)
}

// rebuildSettings swaps in a fresh settings form for the current mode. Called
// when the mode changes, so the form always matches the selected mode's params.
func (v *configView) rebuildSettings() { v.buildSettings() }

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
	label = baseModeLabel(label)
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

func (v *configView) modeLabel() string  { return modes[v.modeIdx].label }
func (v *configView) familyName() string { return families[v.familyIdx].name }
func (v *configView) method() pb.PttMethod {
	return pttMethods[v.methodIdx]
}

func (v *configView) setDevices(devs []*pb.DeviceInfo) {
	v.devs = v.devs[:0]
	for _, d := range devs {
		v.devs = append(v.devs, devEntry{
			id: d.GetDeviceId(), label: d.GetLabel(),
			capture: d.GetHasCapture(), playback: d.GetHasPlayback(),
			needsSetup: d.GetNeedsSetup(),
		})
	}
}

// capabilityDevices is the device list for the focused field, filtered only by
// the field's required capability (RX→capture, TX→playback, PTT→any). The '/'
// text filter is applied generically on top by pickerRows.
func (v *configView) capabilityDevices() []devEntry {
	out := make([]devEntry, 0, len(v.devs))
	for _, d := range v.devs {
		switch v.focus {
		case fRx:
			if !d.capture {
				continue
			}
		case fTx:
			if !d.playback {
				continue
			}
		}
		out = append(out, d)
	}
	return out
}

// pickerAllRows builds the full (text-unfiltered) row set for the open picker,
// dispatching on its kind. Each row carries its display cells and the action that
// applies it when chosen.
func (v *configView) pickerAllRows() []pickerRow {
	var rows []pickerRow
	switch v.picker {
	case pickDevice:
		for _, d := range v.capabilityDevices() {
			d := d
			rows = append(rows, pickerRow{
				cells:  []string{d.label, d.id, ioFlags(d)},
				match:  strings.ToLower(d.label + " " + d.id),
				choose: func(v *configView) { v.setDevice(d.id) },
			})
		}
	case pickFamily:
		for i := range families {
			i := i
			fam := families[i]
			noun := "modes"
			if len(fam.modes) == 1 {
				noun = "mode"
			}
			rows = append(rows, pickerRow{
				cells:  []string{fam.name, fmt.Sprintf("%d %s", len(fam.modes), noun)},
				match:  strings.ToLower(fam.name),
				choose: func(v *configView) { v.selectFamily(i) },
			})
		}
	case pickMode:
		for _, mi := range families[v.familyIdx].modes {
			mi := mi
			rows = append(rows, pickerRow{
				cells:  []string{displayMode(modes[mi].label)},
				match:  strings.ToLower(modes[mi].label),
				choose: func(v *configView) { v.selectMode(mi) },
			})
		}
	}
	return rows
}

// pickerRows is the open picker's currently-visible rows: pickerAllRows narrowed
// by the active '/' filter text (matched against each row's match string).
func (v *configView) pickerRows() []pickerRow {
	all := v.pickerAllRows()
	q := strings.ToLower(strings.TrimSpace(v.filter.Value()))
	if q == "" {
		return all
	}
	out := make([]pickerRow, 0, len(all))
	for _, r := range all {
		if strings.Contains(r.match, q) {
			out = append(out, r)
		}
	}
	return out
}

// setDevice records a chosen device id into the focused device field.
func (v *configView) setDevice(id string) {
	switch v.focus {
	case fRx:
		v.rxID = id
	case fTx:
		v.txID = id
	case fPtt:
		v.pttID = id
	}
}

// selectFamily switches to family i and homes the mode on its first submode.
func (v *configView) selectFamily(i int) {
	v.familyIdx = i
	v.modeIdx = families[i].modes[0]
	v.rebuildSettings()
}

// selectMode selects submode modeIdx within the current family.
func (v *configView) selectMode(modeIdx int) {
	v.modeIdx = modeIdx
	v.rebuildSettings()
}

// pickerTitle titles the open picker modal.
func (v *configView) pickerTitle() string {
	switch v.picker {
	case pickFamily:
		return "Mode family"
	case pickMode:
		return "Mode — " + v.familyName()
	default:
		switch v.focus {
		case fTx:
			return "TX device (playback)"
		case fPtt:
			return "PTT device"
		default:
			return "RX device (capture)"
		}
	}
}

// pickerColumns sizes the open picker's table columns to a target modal width w.
func (v *configView) pickerColumns(w int) []ui.Column {
	switch v.picker {
	case pickFamily:
		return []ui.Column{{Title: "FAMILY", Width: clampInt(w-16, 18, 22)}, {Title: "MODES", Width: 9}}
	case pickMode:
		return []ui.Column{{Title: "MODE", Width: clampInt(w-6, 22, 28)}}
	default: // device
		// The I/O cell is normally width 5 ("RX·TX"), but a needs-setup device
		// widens it with a "⚠ setup" badge; size the column to the widest cell
		// actually present so the badge isn't truncated away. `avail` tracks the
		// column so DEVICE/ID give back exactly the width the badge takes.
		ioW := 5
		for _, d := range v.capabilityDevices() {
			if cw := lipgloss.Width(ioFlags(d)); cw > ioW {
				ioW = cw
			}
		}
		avail := clampInt(w-9-ioW, 24, 56)
		nameW := clampInt(avail*3/5, 12, 40)
		idW := clampInt(avail-nameW, 10, 30)
		return []ui.Column{{Title: "DEVICE", Width: nameW}, {Title: "ID", Width: idW}, {Title: "I/O", Width: ioW}}
	}
}

// clampPick keeps the picker cursor within the current (possibly filtered) row
// set, e.g. after typing narrows the list under the cursor.
func (v *configView) clampPick() {
	if n := len(v.pickerRows()); v.pickIdx >= n {
		v.pickIdx = n - 1
	}
	if v.pickIdx < 0 {
		v.pickIdx = 0
	}
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
		txDelayMs: v.txDelay.Value(),
		txTailMs:  v.txTail.Value(),
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
	v.inflightModeLabel = v.modeLabel()
	v.inflightModeVals = modeValsFrom(v.settings)
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
	mp := modeParamsFor(v.modeLabel(), modeValsFrom(v.settings))
	// FSQ's directed header carries the operator callsign, which isn't a numeric
	// setting; inject it from the station identity.
	if f := mp.GetFsq(); f != nil {
		f.Mycall = v.m.myCall
	}
	chReq := &pb.ConfigureChannelRequest{
		Channel: ch,
		Name:    v.name.Value(),
		// Most modes carry their settings in the typed ModeParams; FST4/JS8/MSK144
		// have no typed message, so their params ride the mode string's tail (the
		// daemon ignores the string when ModeParams is set, so this is safe for all).
		Mode:       modeStringFor(v.modeLabel(), v.settings.Values()),
		ModeParams: mp,
		RsidTx:     v.rsidTx,
		RsidRx:     v.rsidRx,
	}
	audioReq := &pb.ConfigureAudioRequest{
		Channel: ch, DeviceId: v.rxID, SampleRate: 48000, TxDeviceId: v.txID,
	}
	pttReq := &pb.ConfigurePttRequest{
		Channel:   ch,
		DeviceId:  v.pttID,
		Method:    v.method(),
		TxDelayMs: parseMs(v.txDelay.Value()),
		TxTailMs:  parseMs(v.txTail.Value()),
	}
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
		// Confirmed: advance the baseline to exactly what this save persisted, and
		// cache the persisted mode settings so reopening Configure shows them (the
		// daemon doesn't report them back — see buildSettings).
		v.saved = v.inflight
		v.m.modeParams[v.m.sel] = savedModeParams{
			label: v.inflightModeLabel, vals: v.inflightModeVals,
		}
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
		// A picker modal is open (device, family, or mode): it captures all keys.
		// Enter records the highlighted row and closes; esc cancels; '/' opens a
		// substring filter.
		if v.picker != pickNone {
			// While the filter input has focus, keys type into it. Esc clears and
			// blurs it; enter applies it and returns to row navigation.
			if v.filtering {
				switch msg.String() {
				case "esc":
					v.filtering = false
					v.filter.SetValue("")
					v.filter.Blur()
					v.clampPick()
					return v, nil
				case "enter":
					v.filtering = false
					v.filter.Blur()
					v.clampPick()
					return v, nil
				}
				var cmd tea.Cmd
				v.filter, cmd = v.filter.Update(msg)
				v.clampPick()
				return v, cmd
			}
			switch msg.String() {
			case "esc":
				v.closePicker()
				return v, nil
			case "/":
				v.filtering = true
				v.filter.Focus()
				return v, nil
			case "up", "k":
				if v.pickIdx > 0 {
					v.pickIdx--
				}
				return v, nil
			case "down", "j":
				if v.pickIdx < len(v.pickerRows())-1 {
					v.pickIdx++
				}
				return v, nil
			case "enter", " ":
				v.choose()
				v.closePicker()
				return v, v.maybePersist() // auto-apply the choice (mode/device change)
			}
			return v, nil
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
			// Call/Grid aren't part of the daemon channel config, so they ride
			// their own client-side save rather than maybePersist's RPC pipeline.
			v.m.persistIdentity()
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
			// Commit-on-blur: a name edit persists when focus moves off it, and
			// a call/grid edit persists to the client config file.
			v.m.persistIdentity()
			return v, v.maybePersist()
		case "shift+tab", "up":
			if v.focus > fName {
				v.focus--
			}
			v.syncFocus()
			v.m.persistIdentity()
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
		case fTxDelay:
			var cmd tea.Cmd
			v.txDelay, cmd = v.txDelay.Update(msg)
			// A digit edit changes the PTT timing — auto-apply like any field.
			if pc := v.maybePersist(); pc != nil {
				return v, tea.Batch(cmd, pc)
			}
			return v, cmd
		case fTxTail:
			var cmd tea.Cmd
			v.txTail, cmd = v.txTail.Update(msg)
			if pc := v.maybePersist(); pc != nil {
				return v, tea.Batch(cmd, pc)
			}
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

// openPicker opens a picker modal of the given kind, resetting the filter and
// homing the cursor on the currently-selected row (device / family / mode).
func (v *configView) openPicker(kind pickerKind) {
	v.picker = kind
	v.filtering = false
	v.filter.SetValue("")
	v.filter.Blur()
	v.pickIdx = v.pickerCurrentIndex()
}

// pickerCurrentIndex is the row index of the value already selected for the open
// picker, so the cursor opens on it rather than at the top.
func (v *configView) pickerCurrentIndex() int {
	switch v.picker {
	case pickFamily:
		return v.familyIdx
	case pickMode:
		return familyModePos(families[v.familyIdx], v.modeIdx)
	default: // device
		cur := v.currentDeviceID()
		for i, d := range v.capabilityDevices() {
			if d.id == cur {
				return i
			}
		}
	}
	return 0
}

// closePicker dismisses the open picker and clears its transient filter state.
func (v *configView) closePicker() {
	v.picker = pickNone
	v.filtering = false
	v.filter.SetValue("")
	v.filter.Blur()
}

// pickerOpen reports whether any picker modal is currently open.
func (v *configView) pickerOpen() bool { return v.picker != pickNone }

// currentDeviceID is the id already chosen for the focused device field.
func (v *configView) currentDeviceID() string {
	switch v.focus {
	case fTx:
		return v.txID
	case fPtt:
		return v.pttID
	default:
		return v.rxID
	}
}

// syncFocus mirrors the focus index into the text inputs' blink state.
func (v *configView) syncFocus() {
	v.name.Blur()
	v.call.Blur()
	v.grid.Blur()
	v.txDelay.Blur()
	v.txTail.Blur()
	switch v.focus {
	case fName:
		v.name.Focus()
	case fCall:
		v.call.Focus()
	case fGrid:
		v.grid.Focus()
	case fTxDelay:
		v.txDelay.Focus()
	case fTxTail:
		v.txTail.Focus()
	}
}

// cycle steps the focused selector: the mode family, the submode within that
// family, the PTT method, or an RSID toggle.
func (v *configView) cycle(d int) {
	switch v.focus {
	case fFamily:
		v.familyIdx = (v.familyIdx + d + len(families)) % len(families)
		// Landing on a new family selects its first submode so the Mode row is
		// never left pointing outside the family.
		v.modeIdx = families[v.familyIdx].modes[0]
		v.rebuildSettings() // the new mode exposes a different set of settings
	case fMode:
		fam := families[v.familyIdx]
		pos := (familyModePos(fam, v.modeIdx) + d + len(fam.modes)) % len(fam.modes)
		v.modeIdx = fam.modes[pos]
		v.rebuildSettings() // the new submode exposes a different set of settings
	case fMethod:
		v.methodIdx = (v.methodIdx + d + len(pttMethods)) % len(pttMethods)
	case fRsidTx:
		v.rsidTx = !v.rsidTx // boolean toggle; direction is irrelevant
	case fRsidRx:
		v.rsidRx = !v.rsidRx
	}
}

// commit reacts to Enter on the focused field: open the matching pop-up picker
// (family, mode, or device), or the settings editor. Selectors also accept ←/→
// as a quick in-place cycle; PTT method / RSID toggles have no modal.
func (v *configView) commit() (View, tea.Cmd) {
	switch v.focus {
	case fFamily:
		v.openPicker(pickFamily)
	case fMode:
		v.openPicker(pickMode)
	case fRx, fTx, fPtt:
		v.openPicker(pickDevice)
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

// choose applies the highlighted row of the open picker (sets the device, or
// switches family/mode) via that row's choose action.
func (v *configView) choose() {
	rows := v.pickerRows()
	if v.pickIdx < 0 || v.pickIdx >= len(rows) {
		return
	}
	rows[v.pickIdx].choose(v)
}

func (v *configView) Render(w, h int) string {
	// Two-column card layout: the left stack carries the station identity and the
	// mode selection; the right stack carries the audio path and RSID. Each card's
	// border lights up in the accent colour while it owns the focused field.
	gap := 2
	leftW := (w - gap) * 9 / 20 // ~45% to the left, the rest to the (device-heavy) right
	leftW = clampInt(leftW, 32, 48)
	rightW := w - gap - leftW
	if rightW < 34 {
		rightW = 34
	}

	station := ui.Card("STATION", v.stationBody(leftW), v.focusBetween(fName, fGrid), leftW)
	mode := ui.Card("MODE", v.modeBody(leftW), v.focusBetween(fFamily, fSettings), leftW)
	audio := ui.Card("AUDIO", v.audioBody(rightW), v.focusBetween(fRx, fTxTail), rightW)
	rsid := ui.Card("RSID", v.rsidBody(rightW), v.focusBetween(fRsidTx, fRsidRx), rightW)

	left := lipgloss.JoinVertical(lipgloss.Left, station, mode)
	right := lipgloss.JoinVertical(lipgloss.Left, audio, rsid)
	// Pad both columns and the gap between them to one shared height on the panel
	// background. Otherwise the shorter column's filler rows (and the bare-space
	// gap) sit on the terminal's own background and read as grey blocks beside and
	// below the cards.
	colH := max(lipgloss.Height(left), lipgloss.Height(right))
	panelBlock := func(w int, s string) string {
		return lipgloss.NewStyle().Background(ui.ColorPanel).Width(w).Height(colH).Render(s)
	}
	cols := lipgloss.JoinHorizontal(lipgloss.Top,
		panelBlock(leftW, left), panelBlock(gap, ""), panelBlock(rightW, right))

	body := cols + "\n" + v.saveHint()

	// The mode-settings editor is a modal over the form: it surfaces every knob
	// the selected mode exposes, drawn by the reusable ui.SettingsForm.
	if v.editing {
		modalW := clampInt(w, 32, 72)
		// Title is just "<mode> settings"; the hotkeys live on their own dim line
		// along the bottom of the dialog so the header stays clean and unwrapped.
		inner := v.settings.View(modalW-4) + "\n\n" +
			ui.Dim.Render("↑/↓ field · ←/→ change · space toggle · esc done")
		box := ui.Modal(displayMode(v.modeLabel())+" settings", inner, modalW)
		return body + "\n" + centerModal(box, w)
	}
	// A picker modal (family, mode, or device) overlays the form while open, and
	// disappears once a row is chosen or the pick is cancelled.
	if v.pickerOpen() {
		return body + "\n" + centerModal(v.pickerModal(w, h), w)
	}
	return body
}

// pickerModal renders the open picker as a focused Card: a borderless, scrolling
// table of the candidate rows with the cursor highlighted, an optional filter
// line, and a hint footer. Using a Card (not a second bordered box around the
// table) keeps the dialog to a single rounded frame, consistent with the form's
// cards. The same modal serves the family, mode, and device pickers.
func (v *configView) pickerModal(w, h int) string {
	cols := v.pickerColumns(w)
	rows := v.pickerRows()

	var body string
	if len(rows) == 0 {
		body = ui.Dim.Render("(no matches)")
	} else {
		// Window the rows around the cursor so a long list (all modes, every
		// family) can't grow the dialog past the screen; a dim counter shows
		// there's more off-window.
		maxRows := clampInt(h-16, 4, 12)
		start, end := pickWindow(v.pickIdx, len(rows), maxRows)
		data := make([][]string, 0, end-start)
		for _, r := range rows[start:end] {
			data = append(data, r.cells)
		}
		body = ui.TableInset(cols, data, v.pickIdx-start)
		if start > 0 || end < len(rows) {
			body += "\n" + ui.Dim.Render(fmt.Sprintf("  %d–%d of %d", start+1, end, len(rows)))
		}
	}
	if v.filtering || v.filter.Value() != "" {
		body += "\n" + ui.Accent.Render("/") + " " + v.filter.View()
	}
	body += "\n" + ui.Dim.Render("enter pick · / filter · esc")
	// A borderless inset table is TableWidth-2 wide; a Card adds border+padding (4),
	// so Card(width=TableWidth+2) makes the inner area hug the table exactly.
	return ui.Card(v.pickerTitle(), body, true, ui.TableWidth(cols)+2)
}

// pickWindow returns the [start,end) slice of n rows to show for a viewport of
// max rows, scrolled to keep the cursor idx visible (roughly centered).
func pickWindow(idx, n, max int) (int, int) {
	if n <= max {
		return 0, n
	}
	start := idx - max/2
	if start < 0 {
		start = 0
	}
	if start > n-max {
		start = n - max
	}
	return start, start + max
}

// ioFlags is the compact capability badge shown in the device table. A device
// that still needs an OS-level setup step is flagged so the operator knows why
// binding it will fail until they run the fix (see docs/running.md).
func ioFlags(d devEntry) string {
	var cap string
	switch {
	case d.capture && d.playback:
		cap = "RX·TX"
	case d.capture:
		cap = "RX"
	case d.playback:
		cap = "TX"
	default:
		cap = "—"
	}
	if d.needsSetup {
		return cap + " ⚠ setup"
	}
	return cap
}

// focusBetween reports whether the focused field lies in [lo, hi] — i.e. the card
// spanning those fields currently holds focus and should render highlighted.
func (v *configView) focusBetween(lo, hi cfgFocus) bool {
	return v.focus >= lo && v.focus <= hi && !v.pickerOpen() && !v.editing
}

// fieldRow renders one labeled row inside a card: a focus cursor, the padded
// label, then the value (built to fit the card by the body helpers). The cursor
// and label are drawn on the panel background so no bare literal follows the
// value's styled runs and flashes the terminal's own background.
func (v *configView) fieldRow(f cfgFocus, label, val string) string {
	cursor := ui.Body.Render("  ")
	if v.focus == f && !v.pickerOpen() && !v.editing {
		cursor = ui.Accent.Render("▸ ")
	}
	return cursor + ui.Body.Render(fmt.Sprintf("%-10s ", label)) + val
}

// valueWidth is the space left for a row's value inside a card of width cardW,
// after the focus cursor (2) and the padded label ("%-10s " = 11).
func valueWidth(cardW int) int {
	return ui.CardInnerWidth(cardW) - 13
}

func (v *configView) stationBody(cardW int) string {
	vw := valueWidth(cardW)
	// textinput.View renders the prompt ("> ") and reserves a cursor cell on top of
	// its text-area Width, so Width must be the value budget minus those or the row
	// overflows the card and lipgloss wraps the value (e.g. "vfo-a" → "vfo-"/"a").
	inputW := vw - lipgloss.Width(v.name.Prompt) - 1
	if inputW < 1 {
		inputW = 1
	}
	v.name.Width, v.call.Width, v.grid.Width = inputW, inputW, inputW
	return strings.Join([]string{
		v.fieldRow(fName, "Name", v.name.View()),
		v.fieldRow(fCall, "Call", v.call.View()),
		v.fieldRow(fGrid, "Grid", v.grid.View()),
	}, "\n")
}

func (v *configView) modeBody(cardW int) string {
	vw := valueWidth(cardW)
	// Family and Mode read as dropdowns (▾): enter pops a scrolling picker, and
	// ←/→ still quick-cycles in place.
	family := ui.Accent.Render(clip(v.familyName(), vw-2)) + ui.Dim.Render(" ▾")
	return strings.Join([]string{
		v.fieldRow(fFamily, "Family", family),
		v.fieldRow(fMode, "Mode", v.modeSelector(vw)),
		v.fieldRow(fSettings, "Settings", v.settingsSummary(vw)),
	}, "\n")
}

func (v *configView) audioBody(cardW int) string {
	vw := valueWidth(cardW)
	v.txDelay.Width, v.txTail.Width = 6, 6
	method := ui.Accent.Render("‹ " + methodLabel(v.method()) + " ›")
	return strings.Join([]string{
		v.fieldRow(fRx, "RX Device", deviceValue(v.rxID, vw)),
		v.fieldRow(fTx, "TX Device", txDeviceValue(v.txID, v.rxID, vw)),
		v.fieldRow(fPtt, "PTT Device", deviceValue(v.pttID, vw)),
		v.fieldRow(fMethod, "PTT Method", method),
		v.fieldRow(fTxDelay, "TX Delay", v.txDelay.View()+ui.Dim.Render(" ms")),
		v.fieldRow(fTxTail, "TX Tail", v.txTail.View()+ui.Dim.Render(" ms")),
	}, "\n")
}

func (v *configView) rsidBody(cardW int) string {
	onOff := func(on bool) string {
		if on {
			return ui.Accent.Render("● on")
		}
		return ui.Dim.Render("○ off")
	}
	return strings.Join([]string{
		v.fieldRow(fRsidTx, "TX ident", onOff(v.rsidTx)),
		v.fieldRow(fRsidRx, "RX detect", onOff(v.rsidRx)),
	}, "\n")
}

// deviceValue renders a chosen device id (or "(none)"), clipped to fit width w.
func deviceValue(id string, w int) string {
	if id == "" {
		return ui.Dim.Render("(none)")
	}
	return ui.Accent.Render("✓ " + clip(id, w-2))
}

// clip shortens a plain-text string to w display columns, marking a cut with an
// ellipsis. The caller must pass unstyled text (the value builders style after).
func clip(s string, w int) string {
	if w < 1 {
		w = 1
	}
	if lipgloss.Width(s) <= w {
		return s
	}
	r := []rune(s)
	for len(r) > 0 && lipgloss.Width(string(r))+1 > w {
		r = r[:len(r)-1]
	}
	return string(r) + "…"
}

// clampInt bounds v to [lo, hi].
func clampInt(v, lo, hi int) int {
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}

// centerModal horizontally centers a modal box within width w, painting the
// surrounding whitespace with the panel background. Without the explicit
// whitespace background, lipgloss leaves those padding cells unstyled — they then
// render as the terminal's default background (a grey rectangle beside the box)
// instead of the black desktop.
func centerModal(box string, w int) string {
	return lipgloss.PlaceHorizontal(w, lipgloss.Center, box,
		lipgloss.WithWhitespaceBackground(ui.ColorPanel))
}

// txDeviceValue renders the TX device field, clipped to width w. An empty txID
// means "TX follows the RX device" (single-rig default, and how the daemon
// reports TX when it mirrors RX). Show the effective device — the RX id — with a
// "(same as RX)" note rather than a bare "(same as RX)", which reads as "my TX
// choice wasn't saved" when the operator deliberately picked the same device
// (e.g. one BlackHole for both).
func txDeviceValue(txID, rxID string, w int) string {
	if txID != "" {
		return ui.Accent.Render("✓ " + clip(txID, w-2))
	}
	if rxID != "" {
		return ui.Accent.Render("✓ "+clip(rxID, w-16)) + ui.Dim.Render("  (same as RX)")
	}
	return ui.Dim.Render("(same as RX)")
}

// modeSelector renders the Mode row inside width w: the chosen submode as a
// dropdown (▾, enter opens the mode picker) plus its position within the family.
// A single-member family (CW, FT8, …) shows a quiet "(only mode)" note. The label
// is clipped so the row can never overflow the card and wrap.
func (v *configView) modeSelector(w int) string {
	fam := families[v.familyIdx]
	label := displayMode(modes[v.modeIdx].label)
	suffix := fmt.Sprintf(" ▾  %d/%d", familyModePos(fam, v.modeIdx)+1, len(fam.modes))
	if len(fam.modes) <= 1 {
		suffix = " ▾  (only mode)"
	}
	return ui.Accent.Render(clip(label, w-lipgloss.Width(suffix))) + ui.Dim.Render(suffix)
}

// settingsSummary renders the Settings row's value inside width w: an edit button
// (leading, so it lines up under the Family/Mode values) plus a dim count. The
// count is dropped before it would overflow the card, so the row never wraps.
func (v *configView) settingsSummary(w int) string {
	if v.settings == nil || !v.settings.HasFields() {
		return ui.Dim.Render(clip("no settings", w))
	}
	n := v.settings.NumFields()
	noun := "settings"
	if n == 1 {
		noun = "setting"
	}
	edit := ui.Accent.Render("✎ edit")
	count := fmt.Sprintf("  %d %s", n, noun)
	if lipgloss.Width("✎ edit")+lipgloss.Width(count) <= w {
		return edit + ui.Dim.Render(count)
	}
	return edit
}

func (v *configView) Title() string { return fmt.Sprintf("Configure CH%d", v.m.sel) }

func (v *configView) Hints() []ui.Hint {
	if v.editing {
		return []ui.Hint{
			{Key: "↑/↓", Action: "field"}, {Key: "←/→", Action: "change"},
			{Key: "space", Action: "toggle"}, {Key: "esc", Action: "done"},
		}
	}
	if v.pickerOpen() {
		return []ui.Hint{
			{Key: "↑/↓", Action: "move"}, {Key: "enter", Action: "choose"},
			{Key: "/", Action: "filter"}, {Key: "esc", Action: "cancel"},
		}
	}
	// The enter action depends on the focused field: family/mode and device fields
	// open their picker, the Settings row opens the editor, others have no enter.
	hints := []ui.Hint{{Key: "↑/↓", Action: "field"}, {Key: "←/→", Action: "cycle"}}
	switch v.focus {
	case fFamily, fMode:
		hints = append(hints, ui.Hint{Key: "enter", Action: "open picker"})
	case fRx, fTx, fPtt:
		hints = append(hints, ui.Hint{Key: "enter", Action: "pick device"})
	case fSettings:
		hints = append(hints, ui.Hint{Key: "enter", Action: "edit"})
	}
	return append(hints, ui.Hint{Key: "esc", Action: "done"})
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
