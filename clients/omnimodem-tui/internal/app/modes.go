package app

import pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"

// modeParam describes one editable parameter for a mode (label + default).
type modeParam struct {
	key string
	def float64
}

// modeInfo: the modes the operate screen offers, their interaction shape, and
// their editable params. shape "chat" → ragchew surface; "ft8" → sequencer.
type modeInfo struct {
	label  string
	shape  string // "chat" | "ft8"
	params []modeParam
}

var modes = []modeInfo{
	{"psk31", "chat", []modeParam{{"center", 1000}}},
	{"rtty", "chat", []modeParam{{"baud", 45.45}, {"shift", 170}}},
	{"cw", "chat", []modeParam{{"wpm", 20}, {"tone", 700}}},
	{"afsk1200", "chat", nil},
	{"ft8", "ft8", nil},
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
	default:
		return nil // ft8/ft4/jt65/jt9/wspr: no params
	}
}
