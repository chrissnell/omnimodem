package app

import (
	"fmt"
	"image"
	"os"
	"path/filepath"
	"strings"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/ui"
)

type transcriptLine struct {
	t   time.Time
	dir rune // '›' TX, '‹' RX
	txt string
}

// operateView is the per-channel operate screen. Its surface depends on the
// mode's shape: ragchew (transcript + compose + macros + waterfall) for chat
// modes, the auto-sequence ladder for sequencer modes (FT8/FT4/JT65/JT9), or a
// receive-only spot monitor for beacon modes (WSPR).
type operateView struct {
	m          *Model
	compose    string
	transcript []transcriptLine
	tx         txState
	rxWf       waterfall
	txWf       waterfall
	rxOpen     bool // the last transcript line is an in-progress received line
	draining   bool // a TX-waterfall scroll-off animation is in flight
	myCall     string
	myGrid     string
	theirCall  string
	rst        string
	seq        *ft8Seq
	beacon     bool       // beacon monitor (WSPR): no ladder/compose; enter keys a beacon
	raster     *rasterBuf // facsimile raster (Hell): received image column stream
	modeLabel  string     // active mode label, for the surface header
	slotSecs   float64    // T/R slot length for sequencer/beacon modes
	qlog       qsoLog

	// Picture send (image-shape modes): a file picker overlay and the staged
	// image awaiting transmit. picker != nil means the dialog is open and owns
	// all keys; staged != nil means a picture is chosen and previewed.
	picker *ui.ImagePicker
	staged *stagedImage
}

// stagedImage is a picture chosen from the picker, held ready to transmit with a
// live preview shown on the operate surface.
type stagedImage struct {
	name string
	img  image.Image
	w, h int
	size int64 // source file size, for the preview header
}

func newOperateView(m *Model) *operateView {
	v := &operateView{
		m:      m,
		myCall: m.myCall,
		myGrid: m.myGrid,
		rst:    "599",
	}
	if cl := m.live[m.sel]; cl != nil {
		v.modeLabel = baseModeLabel(cl.mode)
		if mi := modeByLabel(cl.mode); mi != nil {
			v.slotSecs = mi.slotSecs
			// FST4's T/R period is operator-selectable and carried in the mode
			// string's tail; honour it so the slot clock and TX watchdog match the
			// configured sequence length rather than the table's 15 s default.
			if mi.label == "fst4" {
				v.slotSecs = modeStringParam(cl.mode, "tr", mi.slotSecs)
			}
			switch mi.shape {
			case "sequencer":
				v.seq = newFT8Seq(v.myCall, v.myGrid)
			case "beacon":
				v.beacon = true
			case "image":
				// Facsimile (Hell): a scrolling raster RX surface. TX still composes
				// text — the mode paints it as a pixel raster on the wire.
				v.raster = &rasterBuf{}
			}
		}
	}
	// Size the TX watchdog to the mode's slot length now that it's known: windowed
	// modes wait for the daemon's slot-align count-off before keying, so a fixed
	// timeout would abort long-slot modes before they ever transmit.
	dog := txWatchdog(v.slotSecs)
	v.tx = txState{watchdog: dog, baseDog: dog}
	return v
}

func (v *operateView) Update(msg tea.Msg) (View, tea.Cmd) {
	switch msg := msg.(type) {
	case eventMsg:
		// The window manager has already folded this into live state and will
		// re-issue waitForEvent; here we only react to operate-specific events.
		if sf := msg.ev.GetSpectrumFrame(); sf != nil {
			if sf.GetTransmit() {
				v.txWf.push(sf)
			} else {
				v.rxWf.push(sf)
			}
		}
		if rf := msg.ev.GetRxFrame(); rf != nil && rf.GetChannel() == v.m.sel {
			if v.raster != nil {
				v.raster.push(rf.GetImage()) // facsimile: accumulate the raster columns
			} else {
				v.appendRx(string(rf.GetData()))
			}
		}
		if tf := msg.ev.GetTransmitFailed(); tf != nil && v.tx.active() {
			// The burst never keyed (e.g. the message can't be encoded in this
			// mode). Tell the operator instead of leaving them with silence; the
			// TransmitComplete that follows resets state and releases the lease.
			v.m.toast = ui.NewToast("TX failed: "+tf.GetReason(), ui.SeverityError)
		}
		if tc := msg.ev.GetTransmitComplete(); tc != nil && v.tx.active() {
			v.tx.onComplete()
			// A picture is a one-shot send: once it's on the air, clear the staged
			// slot so enter doesn't silently re-transmit the same file.
			v.staged = nil
			return v, releaseLeaseCmd(v.m.c, v.m.sel)
		}
		return v, nil
	case spectrumCfgMsg:
		// The per-frame events carry the frequency axis; nothing to do here.
		return v, nil
	case leaseMsg:
		if msg.resp.GetGranted() {
			v.tx.onLeaseGranted()
			if v.tx.image != nil {
				return v, transmitImageCmd(v.m.c, v.m.sel, v.tx.image)
			}
			return v, transmitCmd(v.m.c, v.m.sel, v.tx.payload)
		}
		v.tx.halt()
		v.m.toast = ui.NewToast(fmt.Sprintf("TX lease held by CH%d", msg.resp.GetHeldBy()), ui.SeverityWarn)
		return v, nil
	case transmitMsg:
		v.tx.id = msg.id
		return v, nil
	case tickMsg:
		if v.tx.watchdogExpired(time.Time(msg)) {
			v.tx.halt()
			v.m.toast = ui.NewToast("TX watchdog: aborted", ui.SeverityError)
			return v, releaseLeaseCmd(v.m.c, v.m.sel)
		}
		// Once a transmission ends, scroll its waterfall off to black. The fast
		// drain animation runs only while there's something to clear.
		if !v.draining && !v.tx.active() && v.txWf.hasSignal() {
			v.draining = true
			return v, txDrainCmd()
		}
		return v, nil
	case txDrainMsg:
		if v.tx.active() || !v.txWf.hasSignal() {
			if !v.txWf.hasSignal() {
				v.txWf.rows = nil // fully scrolled off — leave the pane blank
			}
			v.draining = false
			return v, nil
		}
		v.txWf.pushBlank()
		return v, txDrainCmd()
	case tea.KeyMsg:
		// While the picture picker is open it owns every key.
		if v.picker != nil {
			switch v.picker.Update(msg) {
			case ui.PickerSelected:
				v.stageImage(v.picker.Selected())
				v.picker = nil
			case ui.PickerCancelled:
				v.picker = nil
			}
			return v, nil
		}
		switch msg.String() {
		case "esc":
			// Leave operate: halt any TX, stop the spectrum, then pop back.
			v.m.pop()
			if v.tx.active() {
				v.tx.halt()
				return v, tea.Batch(releaseLeaseCmd(v.m.c, v.m.sel), disableSpectrumCmd(v.m.c, v.m.sel))
			}
			return v, disableSpectrumCmd(v.m.c, v.m.sel)
		case "ctrl+x":
			// Halt TX in place, stay on the screen.
			if v.tx.active() {
				v.tx.halt()
				return v, releaseLeaseCmd(v.m.c, v.m.sel)
			}
			// Idle picture mode: clear a staged image without transmitting.
			if v.staged != nil {
				v.staged = nil
			}
			return v, nil
		case "ctrl+o":
			// Picture modes: open the file picker to choose an image to send.
			if v.raster != nil {
				v.picker = ui.NewImagePicker(pickerStartDir())
			}
			return v, nil
		case "enter":
			if v.beacon {
				return v, v.beaconSend() // WSPR: key one beacon (call/grid/power)
			}
			if v.seq != nil {
				return v, v.ft8Send()
			}
			if v.raster != nil && v.staged != nil {
				return v, v.sendImage() // picture mode with an image staged
			}
			return v, v.sendCompose()
		case "f1", "f2", "f3", "f4", "f5":
			if v.beacon || v.seq != nil || v.staged != nil {
				return v, nil // no free-text macros on the ladder/beacon/staged surfaces
			}
			v.compose = expandMacro(macroForKey(msg.String()), macroCtx{
				myCall: v.myCall, theirCall: v.theirCall, rst: v.rst,
			})
			return v, nil
		case "backspace":
			if v.staged == nil && len(v.compose) > 0 {
				v.compose = v.compose[:len(v.compose)-1]
			}
		default:
			// While a picture is staged the compose line is hidden behind its
			// preview, so swallow text input rather than growing it invisibly.
			if v.seq == nil && !v.beacon && v.staged == nil && len(msg.Runes) > 0 {
				v.compose += string(msg.Runes)
			}
		}
	}
	return v, nil
}

// ft8Send transmits the next ladder message; CQ does not advance, RR73 logs once.
func (v *operateView) ft8Send() tea.Cmd {
	if v.tx.active() {
		return nil
	}
	seq := v.seq
	var msg string
	if seq.dxCall == "" {
		msg = seq.cq()
	} else {
		msg = seq.current()
		if seq.step == ladderRR73Step {
			v.qlog.add(seq.dxCall, seq.dxGrid, v.rst)
		}
		seq.advance()
	}
	v.transcript = append(v.transcript, transcriptLine{t: time.Now(), dir: '›', txt: msg})
	v.rxOpen = false // a TX line closes any in-progress received line
	v.tx.begin([]byte(msg))
	return acquireLeaseCmd(v.m.c, v.m.sel)
}

// wsprBeaconPowerDBm is the default reported power for a hand-keyed WSPR beacon
// (37 dBm ≈ 5 W). WSPR carries the operator's TX power in the message itself.
const wsprBeaconPowerDBm = 37

// beaconSend keys a single WSPR beacon ("CALL GRID DBM"). WSPR has no QSO ladder
// or free-text compose — it only ever transmits the operator's call, 4-char grid,
// and power — so the beacon surface transmits on demand rather than staying
// receive-only (previously enter did nothing here, so WSPR made no sound at all).
func (v *operateView) beaconSend() tea.Cmd {
	if v.tx.active() {
		return nil
	}
	grid := v.myGrid
	if len(grid) > 4 {
		grid = grid[:4] // WSPR type-1 messages carry a 4-char Maidenhead locator
	}
	if v.myCall == "" || len(grid) < 4 {
		v.m.toast = ui.NewToast("Set your call and 4-char grid in Configure to beacon", ui.SeverityWarn)
		return nil
	}
	msg := fmt.Sprintf("%s %s %d", v.myCall, grid, wsprBeaconPowerDBm)
	v.transcript = append(v.transcript, transcriptLine{t: time.Now(), dir: '›', txt: msg})
	v.tx.begin([]byte(msg))
	return acquireLeaseCmd(v.m.c, v.m.sel)
}

// appendRx folds streaming decoded text into the transcript. Modes like PSK31
// decode roughly a character at a time, so received text is accumulated onto a
// single in-progress received line and broken into a new line at each newline.
func (v *operateView) appendRx(s string) {
	for _, r := range s {
		if r == '\n' || r == '\r' {
			v.rxOpen = false
			continue
		}
		if !v.rxOpen {
			v.transcript = append(v.transcript, transcriptLine{t: time.Now(), dir: '‹', txt: ""})
			v.rxOpen = true
		}
		last := &v.transcript[len(v.transcript)-1]
		last.txt += string(r)
	}
}

// pickerStartDir chooses where the picture picker opens: the operator's home
// directory when known (that's where photos usually live), else the working dir.
func pickerStartDir() string {
	if home, err := os.UserHomeDir(); err == nil {
		return home
	}
	return "."
}

// maxStageBytes caps the file we'll read into memory to stage. Facsimile
// pictures are tiny (kilobytes), so anything past a few MB is a mistaken pick;
// refusing it keeps a huge file from being read (and later decoded) whole on the
// synchronous UI thread.
const maxStageBytes = 8 << 20 // 8 MiB

// stageImage decodes a chosen file into the staging slot for preview and TX. The
// image is downsampled to a mode-appropriate raster only at send time. Failures
// leave any previously staged image untouched and surface a toast.
func (v *operateView) stageImage(path string) {
	var size int64
	if fi, err := os.Stat(path); err == nil {
		if fi.Size() > maxStageBytes {
			v.m.toast = ui.NewToast(fmt.Sprintf("Image too large (%s, max %s)",
				ui.HumanSize(fi.Size()), ui.HumanSize(maxStageBytes)), ui.SeverityError)
			return
		}
		size = fi.Size()
	}
	img, err := ui.DecodeImageFile(path)
	if err != nil {
		v.m.toast = ui.NewToast("Cannot decode image: "+err.Error(), ui.SeverityError)
		return
	}
	b := img.Bounds()
	v.staged = &stagedImage{name: filepath.Base(path), img: img, w: b.Dx(), h: b.Dy(), size: size}
	v.compose = "" // the preview replaces the compose line; drop any half-typed text
	v.m.toast = ui.NewToast(fmt.Sprintf("Staged %s — enter to transmit", v.staged.name), ui.SeverityInfo)
}

// sendImage transmits the staged picture over the current picture-capable mode.
// The decoded image is downsampled to a mode-appropriate raster and handed to the
// daemon's TransmitImage encoder — not the text Transmit RPC — so the picture is
// modulated as pixels rather than dumped as garbage "text" the modulator rejects.
func (v *operateView) sendImage() tea.Cmd {
	if v.tx.active() || v.staged == nil {
		return nil
	}
	ps, ok := buildPictureSend(v.modeLabel, v.staged.img)
	if !ok {
		v.m.toast = ui.NewToast(
			fmt.Sprintf("%s can't transmit a picture — use an SSTV or WEFAX mode", displayMode(v.modeLabel)),
			ui.SeverityError)
		return nil
	}
	v.tx.beginImage(ps)
	return acquireLeaseCmd(v.m.c, v.m.sel)
}

func (v *operateView) sendCompose() tea.Cmd {
	line := strings.TrimSpace(v.compose)
	if line == "" || v.tx.active() {
		return nil
	}
	v.transcript = append(v.transcript, transcriptLine{t: time.Now(), dir: '›', txt: line})
	v.rxOpen = false // a TX line closes any in-progress received line
	v.tx.begin([]byte(line))
	v.compose = ""
	return acquireLeaseCmd(v.m.c, v.m.sel)
}

func (v *operateView) Render(w, h int) string {
	// The picture picker takes over the whole surface while open.
	if v.picker != nil {
		modalW := w
		if modalW > 88 {
			modalW = 88
		}
		// picker.View wraps a list/preview of height bodyH in its own chrome
		// (title, top+bottom border, path, hint); on top of that the "\n" top
		// margin and the enclosing ui.Frame's title row each cost a line. Reserve
		// all of it so the modal fills the surface exactly instead of growing one
		// line past it — which pushes the frame (and the modal's own title) off
		// the top of the screen.
		const chrome = 8
		bodyH := h - chrome
		if bodyH < 6 {
			bodyH = 6
		}
		box := v.picker.View(modalW, bodyH)
		return "\n" + lipgloss.PlaceHorizontal(w, lipgloss.Center, box,
			lipgloss.WithWhitespaceBackground(ui.ColorPanel))
	}

	var b strings.Builder

	// Two waterfalls side by side, fixed at the top: RX (received) on the left,
	// TX (transmitted) on the right.
	wfRows := h / 3
	if wfRows < 3 {
		wfRows = 3
	}
	if wfRows > 8 {
		wfRows = 8
	}
	const gap = 2
	col := (w - gap) / 2
	if col < 8 {
		col = 8
	}
	// Every cell carries the black panel background (label padding, the gap, and
	// the waterfall rows) so no grey leaks through between the colored spans.
	head := ui.Title.Background(ui.ColorPanel).Width(col)
	bodyH := wfRows + 1 // waterfall rows + axis line
	// msgPane centers a message both ways in a pane's body area, on black.
	msgPane := func(msg string) string {
		text := lipgloss.NewStyle().Foreground(ui.ColorAccent).Background(ui.ColorPanel).
			Width(col).Align(lipgloss.Center).Render(msg)
		return lipgloss.Place(col, bodyH, lipgloss.Center, lipgloss.Center, text,
			lipgloss.WithWhitespaceBackground(ui.ColorPanel))
	}
	column := func(label, override string, wf *waterfall) string {
		if override != "" {
			return head.Render(label) + "\n" + msgPane(override)
		}
		return head.Render(label) + "\n" + wf.render(col, wfRows) + "\n" + wf.axis(col)
	}
	rxMsg := ""
	if v.tx.active() {
		rxMsg = "RX channel muted during TX" // the rig can't receive while keyed
	}
	txMsg := ""
	if len(v.txWf.rows) == 0 {
		txMsg = "waterfall idle" // nothing transmitted (or it has scrolled off)
	}
	gapBlock := lipgloss.NewStyle().Background(ui.ColorPanel).Width(gap).Height(wfRows + 2).Render("")
	b.WriteString(lipgloss.JoinHorizontal(
		lipgloss.Top,
		column("RX", rxMsg, &v.rxWf),
		gapBlock,
		column("TX", txMsg, &v.txWf),
	) + "\n\n")

	if v.seq != nil {
		b.WriteString(fmt.Sprintf("%s · slot %.1f/%gs · DX [%s %s]\n\n",
			displayMode(v.modeLabel), slotPosition(time.Now(), v.slotSecs), v.slotSecs,
			orDash(v.seq.dxCall), v.seq.dxGrid))
		b.WriteString("next: " + v.seq.current() + "\n")
		b.WriteString("cq:   " + v.seq.cq() + "\n\n")
		b.WriteString(fmt.Sprintf("logged QSOs: %d", len(v.qlog.entries)))
		return b.String()
	}
	if v.beacon {
		// Spot monitor: show decoded spots; enter keys a beacon, no compose/ladder.
		for _, l := range v.transcript {
			b.WriteString(fmt.Sprintf("%s %c %s\n", l.t.Format("15:04"), l.dir, l.txt))
		}
		b.WriteString(fmt.Sprintf("%s beacon · slot %.0f/%gs · spots: %d",
			displayMode(v.modeLabel), slotPosition(time.Now(), v.slotSecs), v.slotSecs, len(v.transcript)))
		return b.String()
	}
	if v.raster != nil {
		// Facsimile / picture: a scrolling raster of the received image columns,
		// then either the staged-picture preview (ready to transmit) or the text
		// compose line (the mode also paints typed text into pixels on TX).
		b.WriteString(fmt.Sprintf("%s · facsimile raster · %d cols\n\n",
			displayMode(v.modeLabel), len(v.raster.cols)))
		b.WriteString(v.raster.render(w) + "\n\n")
		if v.staged != nil {
			b.WriteString(v.stagedPreview(w, h))
			return b.String()
		}
		b.WriteString("› " + v.compose)
		if v.tx.active() {
			b.WriteString("   " + ui.Accent.Render("[TX]"))
		} else {
			b.WriteString("\n\n" + ui.Dim.Render("‹ctrl+o› choose a picture to send"))
		}
		return b.String()
	}
	for _, l := range v.transcript {
		b.WriteString(fmt.Sprintf("%s %c %s\n", l.t.Format("15:04"), l.dir, l.txt))
	}
	b.WriteString("› " + v.compose)
	if v.tx.active() {
		b.WriteString("   " + ui.Accent.Render("[TX]"))
	}
	return b.String()
}

// stagedPreview draws the chosen picture (half-block thumbnail) with its name,
// dimensions, and the transmit/replace hints — the "preview before TX" surface.
// It sizes the thumbnail to the space left below the raster (h) so it can't
// overrun the frame on a short terminal.
func (v *operateView) stagedPreview(w, h int) string {
	s := v.staged
	previewCols := w
	if previewCols > 56 {
		previewCols = 56
	}
	// Leave room for the header, the blank spacers, and the hint line; clamp to a
	// sane band so tiny frames still show something and tall ones don't sprawl.
	previewRows := h - 6
	if previewRows < 3 {
		previewRows = 3
	}
	if previewRows > 12 {
		previewRows = 12
	}
	art := ui.RenderImageHalfBlock(s.img, previewCols, previewRows)
	header := ui.Title.Render("Ready to send: ") +
		ui.Accent.Render(fmt.Sprintf("%s  %d×%d  %s", s.name, s.w, s.h, ui.HumanSize(s.size)))
	var b strings.Builder
	b.WriteString(header + "\n\n")
	b.WriteString(art + "\n\n")
	if v.tx.active() {
		b.WriteString(ui.Accent.Render("[TX] transmitting picture…"))
	} else {
		b.WriteString(ui.Dim.Render("‹enter› transmit   ‹ctrl+o› choose another   ‹ctrl+x› cancel"))
	}
	return b.String()
}

func (v *operateView) Title() string {
	cl := v.m.live[v.m.sel]
	mode := "—"
	if cl != nil {
		mode = orNone(displayMode(cl.mode))
	}
	return fmt.Sprintf("Operate CH%d · %s", v.m.sel, mode)
}

func (v *operateView) Hints() []ui.Hint {
	if v.beacon {
		return []ui.Hint{
			{Key: "enter", Action: "beacon"}, {Key: "ctrl+x", Action: "halt"}, {Key: "esc", Action: "back"},
		}
	}
	if v.seq != nil {
		return []ui.Hint{
			{Key: "enter", Action: "send next"}, {Key: "ctrl+x", Action: "halt"}, {Key: "esc", Action: "back"},
		}
	}
	if v.raster != nil {
		if v.picker != nil {
			return []ui.Hint{
				{Key: "↑/↓", Action: "browse"}, {Key: "enter", Action: "select"}, {Key: "esc", Action: "cancel"},
			}
		}
		if v.staged != nil {
			return []ui.Hint{
				{Key: "enter", Action: "transmit"}, {Key: "ctrl+o", Action: "change"},
				{Key: "ctrl+x", Action: "cancel"}, {Key: "esc", Action: "back"},
			}
		}
		return []ui.Hint{
			{Key: "ctrl+o", Action: "pick picture"}, {Key: "enter", Action: "send text"},
			{Key: "ctrl+x", Action: "halt"}, {Key: "esc", Action: "back"},
		}
	}
	return []ui.Hint{
		{Key: "enter", Action: "send"}, {Key: "f1-f5", Action: "macros"},
		{Key: "ctrl+x", Action: "halt"}, {Key: "esc", Action: "back"},
	}
}
