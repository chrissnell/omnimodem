package app

import (
	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// --- typed messages for the SDR control RPCs ---
type sdrTuneMsg struct{ resp *pb.SetSdrTuneResponse }
type sdrGainMsg struct{ resp *pb.SetSdrGainResponse }
type sdrConfigMsg struct{ resp *pb.ConfigureSdrResponse }
type sdrCapsMsg struct{ resp *pb.GetSdrCapsResponse }

// setSdrTuneCmd asks the daemon to tune to an absolute demod frequency. The
// daemon splits it into hardware center + NCO offset and echoes the result; an
// SdrState event follows to keep every client in sync.
func setSdrTuneCmd(c client.ModemClient, ch uint32, freqHz float64) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.SetSdrTune(ctx, &pb.SetSdrTuneRequest{Channel: ch, FreqHz: freqHz})
		if err != nil {
			return rpcErrMsg{err}
		}
		return sdrTuneMsg{resp}
	}
}

// setSdrGainCmd sets AGC (auto) or a manual tuner gain (snapped to the table).
func setSdrGainCmd(c client.ModemClient, ch uint32, auto bool, gainDb float32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.SetSdrGain(ctx, &pb.SetSdrGainRequest{Channel: ch, Auto: auto, GainDb: gainDb})
		if err != nil {
			return rpcErrMsg{err}
		}
		return sdrGainMsg{resp}
	}
}

// configureSdrCmd changes demod mode, squelch, or ppm. Sentinels leave the other
// source-wide fields unchanged (capture_rate 0, bias-tee/direct-sampling off).
func configureSdrCmd(c client.ModemClient, ch uint32, demod pb.DemodMode, squelchDb float32, ppm int32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.ConfigureSdr(ctx, &pb.ConfigureSdrRequest{
			Channel: ch, DemodMode: demod, SquelchDb: squelchDb, Ppm: ppm,
		})
		if err != nil {
			return rpcErrMsg{err}
		}
		return sdrConfigMsg{resp}
	}
}

// getSdrCapsCmd fetches the tuner's capabilities (freq range, sample rates, gain
// table) so the view can validate tuning and step gain through real values.
func getSdrCapsCmd(c client.ModemClient, ch uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.GetSdrCaps(ctx, &pb.GetSdrCapsRequest{Channel: ch})
		if err != nil {
			return rpcErrMsg{err}
		}
		return sdrCapsMsg{resp}
	}
}

// isSDRDevice reports whether a channel's bound capture device is an rtl_tcp SDR
// endpoint, whose canonical id is "rtltcp:<host>:<port>" (see crates/omnimodemd
// ids.rs). SDR channels route to the tuning view instead of the operate view.
func isSDRDevice(id string) bool {
	const prefix = "rtltcp:"
	return len(id) >= len(prefix) && id[:len(prefix)] == prefix
}

// sdrSteps are the tuning step sizes (Hz) the operator cycles with `s`: 1 k, 5 k,
// 12.5 k, 25 k — the common amateur channel spacings.
var sdrSteps = []float64{1000, 5000, 12500, 25000}

// stepLabel renders a tuning step in a compact human form (1k, 12.5k, 25k). The
// fractional branch keeps a single decimal, which is exact for every value in
// sdrSteps; a step with sub-100-Hz precision would be rounded in the label.
func stepLabel(hz float64) string {
	switch {
	case hz >= 1000 && hz == float64(int64(hz/1000))*1000:
		return itoa(int64(hz/1000)) + "k"
	case hz >= 1000:
		// non-integer kHz (e.g. 12.5k): one decimal place
		whole := int64(hz / 1000)
		frac := int64((hz - float64(whole)*1000) / 100)
		return itoa(whole) + "." + itoa(frac) + "k"
	default:
		return itoa(int64(hz)) + "Hz"
	}
}

// itoa is a tiny base-10 formatter for non-negative ints, avoiding a strconv
// import for the two callers here.
func itoa(n int64) string {
	if n == 0 {
		return "0"
	}
	var buf [20]byte
	i := len(buf)
	for n > 0 {
		i--
		buf[i] = byte('0' + n%10)
		n /= 10
	}
	return string(buf[i:])
}

// clampFreq keeps a target frequency within the tuner's [min, max] range when
// the range is known (both > 0); otherwise it passes the value through.
func clampFreq(freq, min, max float64) float64 {
	if min > 0 && freq < min {
		return min
	}
	if max > 0 && freq > max {
		return max
	}
	return freq
}

// cycleIdx advances an index by dir within [0, n), clamping at the ends (no
// wrap) so stepping gain/step size never jumps across the table.
func cycleIdx(idx, dir, n int) int {
	if n <= 0 {
		return 0
	}
	idx += dir
	if idx < 0 {
		idx = 0
	}
	if idx >= n {
		idx = n - 1
	}
	return idx
}

// nearestGainIdx returns the index in a discrete gain table closest to `db`, or
// -1 for an empty table. Used to seat the manual-gain cursor on the value the
// daemon reports before the operator steps it.
func nearestGainIdx(gains []float32, db float32) int {
	if len(gains) == 0 {
		return -1
	}
	best, bestErr := 0, absF32(gains[0]-db)
	for i, g := range gains[1:] {
		if e := absF32(g - db); e < bestErr {
			best, bestErr = i+1, e
		}
	}
	return best
}

func absF32(x float32) float32 {
	if x < 0 {
		return -x
	}
	return x
}

// demodLabel is the short display name for a demod mode.
func demodLabel(m pb.DemodMode) string {
	switch m {
	case pb.DemodMode_DEMOD_NBFM:
		return "NBFM"
	case pb.DemodMode_DEMOD_AM:
		return "AM"
	case pb.DemodMode_DEMOD_WFM:
		return "WFM"
	case pb.DemodMode_DEMOD_USB:
		return "USB"
	case pb.DemodMode_DEMOD_LSB:
		return "LSB"
	default:
		return "?"
	}
}

// demodModes is the cycle order for the `m` picker. The enum ships complete in
// Phase A even though only NBFM is implemented daemon-side; selecting another
// mode makes ConfigureSdr fail with UNIMPLEMENTED, which the event loop shows as
// an error toast (model.go's rpcErrMsg case). The picker's displayed mode reads
// from the SdrState-fed chanLive, so a rejected mode never sticks — the label
// stays on the mode actually in effect.
var demodModes = []pb.DemodMode{
	pb.DemodMode_DEMOD_NBFM,
	pb.DemodMode_DEMOD_AM,
	pb.DemodMode_DEMOD_WFM,
	pb.DemodMode_DEMOD_USB,
	pb.DemodMode_DEMOD_LSB,
}

// nextDemod returns the mode after m in the picker cycle (wrapping).
func nextDemod(m pb.DemodMode) pb.DemodMode {
	for i, d := range demodModes {
		if d == m {
			return demodModes[(i+1)%len(demodModes)]
		}
	}
	return demodModes[0]
}
