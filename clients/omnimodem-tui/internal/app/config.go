package app

import (
	"fmt"
	"strings"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

// configState is the configuration-screen form state.
type configState struct {
	devices   []*pb.DeviceInfo
	name      string
	modeLabel string
	params    map[string]float64
	rxDev     string // capture device id
	txDev     string // optional playback device id ("" = same as rxDev)
	pttDev    string
	pttMethod pb.PttMethod
	focus     int
	udev      string
}

func (m *Model) enterConfig() {
	m.screen = screenConfig
	cs := &configState{
		name:      "vfo-a",
		modeLabel: "psk31",
		params:    map[string]float64{},
		pttMethod: pb.PttMethod_PTT_METHOD_VOX,
	}
	if cl := m.live[m.sel]; cl != nil && cl.name != "" {
		cs.name = cl.name
	}
	m.cfg = cs
}

// applyConfig runs the first step of the bind pipeline (ConfigureChannel). On
// success updateConfig chains ConfigureAudio then ConfigurePtt.
func (m *Model) applyConfig() tea.Cmd {
	cs := m.cfg
	ch := m.sel
	c := m.c
	req := &pb.ConfigureChannelRequest{
		Channel:    ch,
		Name:       cs.name,
		Mode:       cs.modeLabel,
		ModeParams: modeParamsFor(cs.modeLabel, cs.params),
	}
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.ConfigureChannel(ctx, req); err != nil {
			return rpcErrMsg{err}
		}
		return channelBoundMsg{}
	}
}

func (m *Model) configureAudioCmd() tea.Cmd {
	cs := m.cfg
	ch := m.sel
	c := m.c
	req := &pb.ConfigureAudioRequest{Channel: ch, DeviceId: cs.rxDev, SampleRate: 48000, TxDeviceId: cs.txDev}
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

func (m *Model) configurePttCmd() tea.Cmd {
	cs := m.cfg
	ch := m.sel
	c := m.c
	req := &pb.ConfigurePttRequest{Channel: ch, DeviceId: cs.pttDev, Method: cs.pttMethod}
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.ConfigurePtt(ctx, req); err != nil {
			return rpcErrMsg{err}
		}
		return pttBoundMsg{}
	}
}

// setGainCmd applies RX/TX gain (linear multipliers) to the selected channel.
func (m *Model) setGainCmd(rx, tx float32) tea.Cmd {
	ch := m.sel
	c := m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.SetAudioGain(ctx, &pb.SetAudioGainRequest{Channel: ch, RxGain: rx, TxGain: tx}); err != nil {
			return rpcErrMsg{err}
		}
		return rpcOKMsg{what: "gain"}
	}
}

// udevCmd fetches an install-ready udev rule for the PTT device (config helper).
func (m *Model) udevCmd(dev string) tea.Cmd {
	c := m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		r, err := c.SuggestUdevRule(ctx, dev)
		if err != nil {
			return rpcErrMsg{err}
		}
		return rpcOKMsg{what: "udev:" + r.GetRule()}
	}
}

func (m *Model) updateConfig(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case devicesMsg:
		m.cfg.devices = msg.devices
		return m, nil
	case channelBoundMsg:
		return m, m.configureAudioCmd() // chain audio after channel
	case audioCfgMsg:
		return m, m.configurePttCmd() // chain ptt after audio
	case pttBoundMsg:
		m.screen = screenDashboard // bind complete
		return m, snapshotCmd(m.c)
	case rpcOKMsg:
		if strings.HasPrefix(msg.what, "udev:") {
			m.cfg.udev = strings.TrimPrefix(msg.what, "udev:")
		}
		return m, nil
	case tea.KeyMsg:
		switch msg.String() {
		case "esc":
			m.screen = screenDashboard
		case "enter":
			return m, m.applyConfig()
		case "tab":
			m.cfg.focus++
		}
	}
	return m, nil
}

func (m *Model) viewConfig() string {
	cs := m.cfg
	var b strings.Builder
	b.WriteString(fmt.Sprintf("Configure ch%d   (tab field · enter apply · esc cancel)\n\n", m.sel))
	b.WriteString("Name:    " + cs.name + "\n")
	b.WriteString("Mode:    " + cs.modeLabel + paramSummary(cs) + "\n")
	b.WriteString("RX dev:  " + orNone(cs.rxDev) + "\n")
	b.WriteString("TX dev:  " + orSame(cs.txDev) + "\n")
	b.WriteString("PTT dev: " + orNone(cs.pttDev) + "  method " + cs.pttMethod.String() + "\n\n")
	b.WriteString("Capture devices:\n")
	for _, d := range cs.devices {
		if d.GetHasCapture() {
			b.WriteString("  " + d.GetDeviceId() + "  " + d.GetLabel() + "\n")
		}
	}
	if cs.udev != "" {
		b.WriteString("\nudev rule:\n" + cs.udev + "\n")
	}
	return b.String()
}

func paramSummary(cs *configState) string {
	mi := modeByLabel(cs.modeLabel)
	if mi == nil || len(mi.params) == 0 {
		return ""
	}
	parts := make([]string, 0, len(mi.params))
	for _, p := range mi.params {
		v := p.def
		if cs.params != nil {
			if got, ok := cs.params[p.key]; ok {
				v = got
			}
		}
		parts = append(parts, fmt.Sprintf("%s=%g", p.key, v))
	}
	return " (" + strings.Join(parts, ", ") + ")"
}

func orSame(s string) string {
	if s == "" {
		return "(same as RX)"
	}
	return s
}
