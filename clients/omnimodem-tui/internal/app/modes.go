package app

import pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"

// modeParam describes one editable parameter for a mode (label + default).
type modeParam struct {
	key string
	def float64
}

// modeInfo: the modes the operate screen offers, their interaction shape, and
// their editable params. shape "chat" → ragchew surface; "sequencer" → the
// structured-QSO auto-sequence ladder (FT8/FT4/JT65/JT9); "beacon" → the
// receive-only spot monitor (WSPR). slotSecs is the T/R window length for the
// windowed sequencer/beacon modes (0 for the streaming "chat" modes).
type modeInfo struct {
	label    string
	shape    string // "chat" | "sequencer" | "beacon"
	slotSecs float64
	params   []modeParam
}

var modes = []modeInfo{
	{"psk31", "chat", 0, []modeParam{{"center", 1000}}},
	{"rtty", "chat", 0, []modeParam{{"baud", 45.45}, {"shift", 170}}},
	{"cw", "chat", 0, []modeParam{{"wpm", 20}, {"tone", 700}}},
	{"afsk1200", "chat", 0, nil},
	{"olivia", "chat", 0, []modeParam{{"tones", 32}, {"bw", 1000}}},
	{"ft8", "sequencer", 15, nil},
	{"ft4", "sequencer", 7.5, nil},
	{"jt65", "sequencer", 60, nil},
	{"jt9", "sequencer", 60, nil},
	{"wspr", "beacon", 120, nil},
}

func modeByLabel(label string) *modeInfo {
	for i := range modes {
		if modes[i].label == label {
			return &modes[i]
		}
	}
	return nil
}

// modeParamsFor builds the typed ModeParams oneof for a mode, or nil for modes
// without params (the daemon then uses the bare-label defaults).
func modeParamsFor(label string, vals map[string]float64) *pb.ModeParams {
	get := func(k string, d float64) float64 {
		if vals != nil {
			if v, ok := vals[k]; ok {
				return v
			}
		}
		return d
	}
	switch label {
	case "cw":
		return &pb.ModeParams{Params: &pb.ModeParams_Cw{Cw: &pb.CwParams{
			Wpm: uint32(get("wpm", 20)), ToneHz: float32(get("tone", 700)),
		}}}
	case "rtty":
		return &pb.ModeParams{Params: &pb.ModeParams_Rtty{Rtty: &pb.RttyParams{
			Baud: float32(get("baud", 45.45)), ShiftHz: float32(get("shift", 170)),
		}}}
	case "psk31":
		return &pb.ModeParams{Params: &pb.ModeParams_Psk31{Psk31: &pb.Psk31Params{
			CenterHz: float32(get("center", 1000)),
		}}}
	case "afsk1200":
		return &pb.ModeParams{Params: &pb.ModeParams_Afsk1200{Afsk1200: &pb.Afsk1200Params{Tx: true}}}
	case "olivia":
		return &pb.ModeParams{Params: &pb.ModeParams_Olivia{Olivia: &pb.OliviaParams{
			Tones: uint32(get("tones", 32)), BandwidthHz: uint32(get("bw", 1000)),
		}}}
	default:
		return nil // ft8/ft4/jt65/jt9/wspr: no params
	}
}
