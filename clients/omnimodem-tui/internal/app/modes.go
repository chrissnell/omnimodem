package app

import pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"

// modeParam describes one editable parameter for a mode (label + default).
type modeParam struct {
	key string
	def float64
}

// modeInfo: the modes the operate screen offers, their interaction shape, and
// their editable params. shape "chat" → ragchew surface; "sequencer" → the
// structured-QSO auto-sequence ladder (FT8/FT4/JT65/JT9); "beacon" → the spot
// monitor (WSPR), which decodes spots and keys a single call/grid/power beacon on
// enter; "image" → the facsimile raster surface (Hell), which scrolls the received
// image columns and composes text that the mode paints as pixels on TX. slotSecs
// is the T/R window length for the windowed sequencer/beacon modes (0 for the
// streaming "chat"/"image" modes).
type modeInfo struct {
	label    string
	shape    string // "chat" | "sequencer" | "beacon" | "image"
	slotSecs float64
	params   []modeParam
}

var modes = []modeInfo{
	{"psk31", "chat", 0, []modeParam{{"center", 1000}}},
	{"psk63", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk125", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk250", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk500", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk1000", "chat", 0, []modeParam{{"center", 1500}}},
	{"qpsk31", "chat", 0, []modeParam{{"center", 1500}}},
	{"qpsk63", "chat", 0, []modeParam{{"center", 1500}}},
	{"qpsk125", "chat", 0, []modeParam{{"center", 1500}}},
	{"qpsk250", "chat", 0, []modeParam{{"center", 1500}}},
	{"qpsk500", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk63f", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk125r", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk250r", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk500r", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk1000r", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk63rc4", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk63rc5", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk63rc10", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk63rc20", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk63rc32", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk125rc4", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk125rc5", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk125rc10", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk125rc12", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk125rc16", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk250rc2", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk250rc3", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk250rc5", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk250rc6", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk250rc7", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk500rc2", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk500rc3", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk500rc4", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk125c12", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk250c6", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk500c2", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk500c4", "chat", 0, []modeParam{{"center", 1500}}},
	{"psk1000c2", "chat", 0, []modeParam{{"center", 1500}}},
	{"dominoexmicro", "chat", 0, []modeParam{{"center", 1500}}},
	{"dominoex4", "chat", 0, []modeParam{{"center", 1500}}},
	{"dominoex5", "chat", 0, []modeParam{{"center", 1500}}},
	{"dominoex8", "chat", 0, []modeParam{{"center", 1500}}},
	{"dominoex11", "chat", 0, []modeParam{{"center", 1500}}},
	{"dominoex16", "chat", 0, []modeParam{{"center", 1500}}},
	{"dominoex22", "chat", 0, []modeParam{{"center", 1500}}},
	{"dominoex44", "chat", 0, []modeParam{{"center", 1500}}},
	{"dominoex88", "chat", 0, []modeParam{{"center", 1500}}},
	{"thormicro", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor4", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor5", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor8", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor11", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor16", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor22", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor25x4", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor50x1", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor50x2", "chat", 0, []modeParam{{"center", 1500}}},
	{"thor100", "chat", 0, []modeParam{{"center", 1500}}},
	// The fldigi IFKP family: 33-tone IFK with the self-framing IFKP Varicode.
	{"ifkp", "chat", 0, []modeParam{{"center", 1500}}},
	{"ifkp-slow", "chat", 0, []modeParam{{"center", 1500}}},
	{"ifkp-fast", "chat", 0, []modeParam{{"center", 1500}}},
	// The fldigi FSQ / FSQCALL family: 33-tone IFK with a CRC8-keyed directed
	// protocol. `directed` (0/1) keys the selective-call header; the operator
	// callsign is taken from the station identity. Directed traffic surfaces in
	// the chat view.
	{"fsq", "chat", 0, []modeParam{{"center", 1500}, {"directed", 0}}},
	{"fsq-1.5", "chat", 0, []modeParam{{"center", 1500}, {"directed", 0}}},
	{"fsq-2", "chat", 0, []modeParam{{"center", 1500}, {"directed", 0}}},
	{"fsq-4.5", "chat", 0, []modeParam{{"center", 1500}, {"directed", 0}}},
	{"fsq-6", "chat", 0, []modeParam{{"center", 1500}, {"directed", 0}}},
	{"feldhell", "image", 0, []modeParam{{"center", 1500}}},
	{"slowhell", "image", 0, []modeParam{{"center", 1500}}},
	{"hellx5", "image", 0, []modeParam{{"center", 1500}}},
	{"hellx9", "image", 0, []modeParam{{"center", 1500}}},
	{"hell80", "image", 0, []modeParam{{"center", 1500}}},
	// The fldigi MFSK family: M-ary FSK + K=7 conv + interleave + MFSK Varicode.
	{"mfsk4", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk8", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk11", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk16", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk22", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk31", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk32", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk64", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk128", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk64l", "chat", 0, []modeParam{{"center", 1500}}},
	{"mfsk128l", "chat", 0, []modeParam{{"center", 1500}}},
	// The fldigi Contestia grid (Olivia's 32-chip-Walsh sibling): tones/bandwidth.
	{"contestia4_125", "chat", 0, []modeParam{{"tones", 4}, {"bw", 125}}},
	{"contestia4_250", "chat", 0, []modeParam{{"tones", 4}, {"bw", 250}}},
	{"contestia4_500", "chat", 0, []modeParam{{"tones", 4}, {"bw", 500}}},
	{"contestia4_1000", "chat", 0, []modeParam{{"tones", 4}, {"bw", 1000}}},
	{"contestia4_2000", "chat", 0, []modeParam{{"tones", 4}, {"bw", 2000}}},
	{"contestia8_125", "chat", 0, []modeParam{{"tones", 8}, {"bw", 125}}},
	{"contestia8_250", "chat", 0, []modeParam{{"tones", 8}, {"bw", 250}}},
	{"contestia8_500", "chat", 0, []modeParam{{"tones", 8}, {"bw", 500}}},
	{"contestia8_1000", "chat", 0, []modeParam{{"tones", 8}, {"bw", 1000}}},
	{"contestia8_2000", "chat", 0, []modeParam{{"tones", 8}, {"bw", 2000}}},
	{"contestia16_250", "chat", 0, []modeParam{{"tones", 16}, {"bw", 250}}},
	{"contestia16_500", "chat", 0, []modeParam{{"tones", 16}, {"bw", 500}}},
	{"contestia16_1000", "chat", 0, []modeParam{{"tones", 16}, {"bw", 1000}}},
	{"contestia16_2000", "chat", 0, []modeParam{{"tones", 16}, {"bw", 2000}}},
	{"contestia32_1000", "chat", 0, []modeParam{{"tones", 32}, {"bw", 1000}}},
	{"contestia32_2000", "chat", 0, []modeParam{{"tones", 32}, {"bw", 2000}}},
	{"contestia64_500", "chat", 0, []modeParam{{"tones", 64}, {"bw", 500}}},
	{"contestia64_1000", "chat", 0, []modeParam{{"tones", 64}, {"bw", 1000}}},
	{"contestia64_2000", "chat", 0, []modeParam{{"tones", 64}, {"bw", 2000}}},
	// The fldigi MT63 family: 64-carrier overlapping-Walsh OFDM + deep interleave.
	{"mt63_500s", "chat", 0, []modeParam{{"center", 1500}}},
	{"mt63_500l", "chat", 0, []modeParam{{"center", 1500}}},
	{"mt63_1000s", "chat", 0, []modeParam{{"center", 1500}}},
	{"mt63_1000l", "chat", 0, []modeParam{{"center", 1500}}},
	{"mt63_2000s", "chat", 0, []modeParam{{"center", 1500}}},
	{"mt63_2000l", "chat", 0, []modeParam{{"center", 1500}}},
	{"rtty", "chat", 0, []modeParam{{"baud", 45.45}, {"shift", 170}}},
	{"cw", "chat", 0, []modeParam{{"wpm", 20}, {"tone", 700}}},
	{"afsk1200", "chat", 0, nil},
	{"olivia", "chat", 0, []modeParam{{"tones", 32}, {"bw", 1000}}},
	{"ft8", "sequencer", 15, nil},
	{"ft4", "sequencer", 7.5, nil},
	{"jt65", "sequencer", 60, nil},
	{"jt9", "sequencer", 60, nil},
	{"fst4", "sequencer", 15, nil}, // LF/MF weak-signal QSO; default 15 s T/R
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
			CenterHz: float32(get("center", 0)), Reverse: get("reverse", 0) != 0,
		}}}
	case "psk31", "psk63", "psk125", "psk250", "psk500", "psk1000",
		"qpsk31", "qpsk63", "qpsk125", "qpsk250", "qpsk500",
		"psk63f", "psk125r", "psk250r", "psk500r", "psk1000r",
		"psk63rc4", "psk63rc5", "psk63rc10", "psk63rc20", "psk63rc32",
		"psk125rc4", "psk125rc5", "psk125rc10", "psk125rc12", "psk125rc16",
		"psk250rc2", "psk250rc3", "psk250rc5", "psk250rc6", "psk250rc7", "psk500rc2", "psk500rc3", "psk500rc4",
		"psk125c12", "psk250c6", "psk500c2", "psk500c4", "psk1000c2":
		// The whole fldigi PSK/QPSK rate family: submode label + audio center.
		// psk31 keeps its 1000 Hz default; the higher rates centre at 1500 Hz.
		def := 1500.0
		if label == "psk31" {
			def = 1000
		}
		return &pb.ModeParams{Params: &pb.ModeParams_Psk{Psk: &pb.PskParams{
			Submode: label, CenterHz: float32(get("center", def)),
		}}}
	case "dominoexmicro", "dominoex4", "dominoex5", "dominoex8", "dominoex11",
		"dominoex16", "dominoex22", "dominoex44", "dominoex88":
		// The fldigi DominoEX IFK+ family: submode label + audio center (1500 Hz).
		return &pb.ModeParams{Params: &pb.ModeParams_Dominoex{Dominoex: &pb.DominoParams{
			Submode: label, CenterHz: float32(get("center", 1500)),
		}}}
	case "thormicro", "thor4", "thor5", "thor8", "thor11", "thor16", "thor22",
		"thor25x4", "thor50x1", "thor50x2", "thor100":
		// The fldigi THOR family (IFK+ core + convolutional FEC + interleave):
		// submode label + audio center (1500 Hz).
		return &pb.ModeParams{Params: &pb.ModeParams_Thor{Thor: &pb.ThorParams{
			Submode: label, CenterHz: float32(get("center", 1500)),
		}}}
	case "feldhell", "slowhell", "hellx5", "hellx9", "hell80":
		// The fldigi Feld Hell facsimile family: submode label + audio center.
		return &pb.ModeParams{Params: &pb.ModeParams_Hell{Hell: &pb.HellParams{
			Submode: label, CenterHz: float32(get("center", 1500)),
		}}}
	case "ifkp", "ifkp-slow", "ifkp-fast":
		// The fldigi IFKP family: speed label + audio center (1500 Hz).
		return &pb.ModeParams{Params: &pb.ModeParams_Ifkp{Ifkp: &pb.IfkpParams{
			Speed: label, CenterHz: float32(get("center", 1500)),
		}}}
	case "fsq", "fsq-1.5", "fsq-2", "fsq-4.5", "fsq-6":
		// The fldigi FSQ / FSQCALL family: speed label + audio center + directed
		// flag. `mycall` is injected from the station identity at the call site
		// (persistAll), since it is not a numeric setting.
		return &pb.ModeParams{Params: &pb.ModeParams_Fsq{Fsq: &pb.FsqParams{
			Speed: label, CenterHz: float32(get("center", 1500)), Directed: get("directed", 0) != 0,
		}}}
	case "mfsk4", "mfsk8", "mfsk11", "mfsk16", "mfsk22", "mfsk31",
		"mfsk32", "mfsk64", "mfsk128", "mfsk64l", "mfsk128l":
		// The fldigi MFSK family: submode label + audio center (1500 Hz).
		return &pb.ModeParams{Params: &pb.ModeParams_Mfsk{Mfsk: &pb.MfskParams{
			Submode: label, CenterHz: float32(get("center", 1500)),
		}}}
	case "mt63_500s", "mt63_500l", "mt63_1000s", "mt63_1000l", "mt63_2000s", "mt63_2000l":
		// The fldigi MT63 family: submode label + audio center (1500 Hz).
		return &pb.ModeParams{Params: &pb.ModeParams_Mt63{Mt63: &pb.Mt63Params{
			Submode: label, CenterHz: float32(get("center", 1500)),
		}}}
	case "contestia4_125", "contestia4_250", "contestia4_500", "contestia4_1000", "contestia4_2000",
		"contestia8_125", "contestia8_250", "contestia8_500", "contestia8_1000", "contestia8_2000",
		"contestia16_250", "contestia16_500", "contestia16_1000", "contestia16_2000",
		"contestia32_1000", "contestia32_2000",
		"contestia64_500", "contestia64_1000", "contestia64_2000":
		// The fldigi Contestia grid: tones + bandwidth carried in typed params.
		mi := modeByLabel(label)
		t, bw := 8.0, 500.0
		if mi != nil {
			for _, p := range mi.params {
				if p.key == "tones" {
					t = p.def
				}
				if p.key == "bw" {
					bw = p.def
				}
			}
		}
		return &pb.ModeParams{Params: &pb.ModeParams_Contestia{Contestia: &pb.ContestiaParams{
			Tones: uint32(get("tones", t)), BandwidthHz: uint32(get("bw", bw)),
		}}}
	case "afsk1200":
		return &pb.ModeParams{Params: &pb.ModeParams_Afsk1200{Afsk1200: &pb.Afsk1200Params{Tx: get("tx", 1) != 0}}}
	case "olivia":
		return &pb.ModeParams{Params: &pb.ModeParams_Olivia{Olivia: &pb.OliviaParams{
			Tones: uint32(get("tones", 32)), BandwidthHz: uint32(get("bw", 1000)),
		}}}
	default:
		return nil // ft8/ft4/jt65/jt9/wspr: no params
	}
}
