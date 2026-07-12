package app

import (
	"strconv"
	"strings"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

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
// image columns and composes text that the mode paints as pixels on TX; "adsb" →
// the receive-only live flights table (view_channels routes an ADS-B channel there
// instead of the operate screen). slotSecs is the T/R window length for the
// windowed sequencer/beacon modes (0 for the streaming "chat"/"image"/"adsb" modes).
type modeInfo struct {
	label    string
	shape    string // "chat" | "sequencer" | "beacon" | "image" | "adsb"
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
	// The MMSSTV SSTV colour line-scan family (RGB-sequential: Scottie/Martin/SC2
	// wired). No tunable params — the VIS + scan frequencies are fixed by the SSTV
	// standard. RX emits a colour raster (Image, channels=3); TX paints an RGB picture.
	{"scottie1", "image", 0, nil},
	{"scottie2", "image", 0, nil},
	{"scottiedx", "image", 0, nil},
	{"martin1", "image", 0, nil},
	{"martin2", "image", 0, nil},
	{"sc2-180", "image", 0, nil},
	{"sc2-120", "image", 0, nil},
	{"sc2-60", "image", 0, nil},
	{"robot72", "image", 0, nil},
	{"robot36", "image", 0, nil},
	{"robot24", "image", 0, nil},
	{"bw8", "image", 0, nil},
	{"bw12", "image", 0, nil},
	{"p3", "image", 0, nil},
	{"p5", "image", 0, nil},
	{"p7", "image", 0, nil},
	{"pd50", "image", 0, nil},
	{"pd90", "image", 0, nil},
	{"pd120", "image", 0, nil},
	{"pd160", "image", 0, nil},
	{"pd180", "image", 0, nil},
	{"pd240", "image", 0, nil},
	{"pd290", "image", 0, nil},
	{"mp73", "image", 0, nil},
	{"mp115", "image", 0, nil},
	{"mp140", "image", 0, nil},
	{"mp175", "image", 0, nil},
	{"mr73", "image", 0, nil},
	{"mr90", "image", 0, nil},
	{"mr115", "image", 0, nil},
	{"mr140", "image", 0, nil},
	{"mr175", "image", 0, nil},
	{"ml180", "image", 0, nil},
	{"ml240", "image", 0, nil},
	{"ml280", "image", 0, nil},
	{"ml320", "image", 0, nil},
	{"mp73-n", "image", 0, nil},
	{"mp110-n", "image", 0, nil},
	{"mp140-n", "image", 0, nil},
	{"mc110-n", "image", 0, nil},
	{"mc140-n", "image", 0, nil},
	{"mc180-n", "image", 0, nil},
	{"avt90", "image", 0, nil},
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
	// The fldigi Throb family: dual-tone MFSK at 8 kHz (Throb / ThrobX).
	{"throb1", "chat", 0, []modeParam{{"center", 1500}}},
	{"throb2", "chat", 0, []modeParam{{"center", 1500}}},
	{"throb4", "chat", 0, []modeParam{{"center", 1500}}},
	{"throbx1", "chat", 0, []modeParam{{"center", 1500}}},
	{"throbx2", "chat", 0, []modeParam{{"center", 1500}}},
	{"throbx4", "chat", 0, []modeParam{{"center", 1500}}},
	{"navtex", "chat", 0, []modeParam{{"center", 1000}}},
	{"sitorb", "chat", 0, []modeParam{{"center", 1000}}},
	{"wefax576", "image", 0, []modeParam{{"center", 1900}}},
	{"wefax288", "image", 0, []modeParam{{"center", 1900}}},
	{"rtty", "chat", 0, []modeParam{{"baud", 45.45}, {"shift", 170}}},
	{"cw", "chat", 0, []modeParam{{"wpm", 20}, {"tone", 700}}},
	{"afsk1200", "chat", 0, nil},
	{"olivia", "chat", 0, []modeParam{{"tones", 32}, {"bw", 1000}}},
	{"ft8", "sequencer", 15, nil},
	{"ft4", "sequencer", 7.5, nil},
	{"jt65", "sequencer", 60, nil},
	{"jt9", "sequencer", 60, nil},
	{"fst4", "sequencer", 15, nil}, // LF/MF weak-signal QSO; default 15 s T/R
	// The WSJT-X JT4 family (legacy EME): submodes A–G differ only in 4-FSK tone
	// spacing; 60 s on the minute, same auto-sequence ladder as JT65/JT9.
	{"jt4a", "sequencer", 60, nil},
	{"jt4b", "sequencer", 60, nil},
	{"jt4c", "sequencer", 60, nil},
	{"jt4d", "sequencer", 60, nil},
	{"jt4e", "sequencer", 60, nil},
	{"jt4f", "sequencer", 60, nil},
	{"jt4g", "sequencer", 60, nil},
	{"msk144", "sequencer", 0, nil}, // VHF meteor scatter; streaming short bursts (default 1500 Hz)
	{"wspr", "beacon", 120, nil},
	// W5 JS8Call JS8: 8-GFSK weak-signal keyboard mode on the FT8 core. Like FSQ
	// it is a free-text/directed surface (chat), windowed by the daemon at the
	// submode's T/R period. Registered as the Normal submode; the submode
	// selector and full directed-protocol view are follow-on work.
	{"js8", "chat", 0, nil},
	// ADS-B (Mode S) 1090 MHz surveillance. Receive-only: bound to an rtl_tcp SDR,
	// the daemon captures wideband and streams AircraftReport telemetry. No operator
	// params and no TX; selecting it on a channel opens the live flights table.
	{"adsb", "adsb", 0, nil},
}

// baseModeLabel strips the daemon's parameter suffix from a live mode string.
// The daemon reports a channel's mode as the descriptor it persists — e.g.
// "feldhell:center=1500" or "rtty:baud=45,shift=170,center=915,reverse=false"
// (mode/mod.rs to_mode_string) — while the modes table is keyed by the bare
// label. Everything from the first ':' is params; labels without one (e.g.
// "contestia8_500") pass through unchanged.
func baseModeLabel(mode string) string {
	base, _, _ := strings.Cut(mode, ":")
	return base
}

// modeDisplayNames maps the irregular bare labels to the mixed-case names their
// authors and operators actually use. Only labels whose conventional spelling
// isn't produced by displayMode's family rules or its all-caps fallback live
// here — plain acronyms (PSK31, MFSK16, FT8, RTTY, PD120, SC2-180 …) are handled
// by uppercasing and are deliberately absent.
var modeDisplayNames = map[string]string{
	// fldigi Feld Hell facsimile family.
	"feldhell": "Feld Hell",
	"slowhell": "Slow Hell",
	"hellx5":   "Hell X5",
	"hellx9":   "Hell X9",
	"hell80":   "Hell 80",
	// fldigi IFKP speeds (bare "ifkp" uppercases fine).
	"ifkp-slow": "IFKP Slow",
	"ifkp-fast": "IFKP Fast",
	// fldigi Throb / ThrobX.
	"throb1":   "Throb1",
	"throb2":   "Throb2",
	"throb4":   "Throb4",
	"throbx1":  "ThrobX1",
	"throbx2":  "ThrobX2",
	"throbx4":  "ThrobX4",
	"olivia":   "Olivia",
	"sitorb":   "SITOR-B",
	"wefax576": "WEFAX-576",
	"wefax288": "WEFAX-288",
	// ADS-B (Mode S) surveillance.
	"adsb": "ADS-B",
	// MMSSTV modes whose names carry a spelled-out word.
	"scottie1":  "Scottie 1",
	"scottie2":  "Scottie 2",
	"scottiedx": "Scottie DX",
	"martin1":   "Martin 1",
	"martin2":   "Martin 2",
	"robot72":   "Robot 72",
	"robot36":   "Robot 36",
	"robot24":   "Robot 24",
}

// displayMode returns the conventional operator-facing name for a mode string —
// the casing each mode is known by (fldigi / WSJT-X / MMSSTV). It is display
// only: the daemon wire label (baseModeLabel) is never altered. Irregular names
// come from modeDisplayNames; the big prefix families follow their own rule; and
// anything left (plain acronym + digits, e.g. FT8, MFSK16, PD120) uppercases.
func displayMode(mode string) string {
	label := baseModeLabel(mode)
	if label == "" {
		return ""
	}
	if d, ok := modeDisplayNames[label]; ok {
		return d
	}
	switch {
	case strings.HasPrefix(label, "psk"), strings.HasPrefix(label, "qpsk"),
		strings.HasPrefix(label, "mfsk"), strings.HasPrefix(label, "fsq"):
		return strings.ToUpper(label)
	case strings.HasPrefix(label, "dominoex"):
		return "DominoEX " + modeSizeSuffix(strings.TrimPrefix(label, "dominoex"))
	case strings.HasPrefix(label, "thor"):
		return "THOR " + modeSizeSuffix(strings.TrimPrefix(label, "thor"))
	case strings.HasPrefix(label, "contestia"):
		return "Contestia " + strings.Replace(strings.TrimPrefix(label, "contestia"), "_", "/", 1)
	case strings.HasPrefix(label, "mt63_"):
		return "MT63-" + strings.ToUpper(strings.TrimPrefix(label, "mt63_"))
	}
	return strings.ToUpper(label)
}

// modeSizeSuffix formats the DominoEX/THOR size suffix: the word "micro"
// capitalizes, numeric speeds (4, 88, 25x4 …) pass through unchanged.
func modeSizeSuffix(s string) string {
	if s == "micro" {
		return "Micro"
	}
	return s
}

func modeByLabel(label string) *modeInfo {
	label = baseModeLabel(label)
	for i := range modes {
		if modes[i].label == label {
			return &modes[i]
		}
	}
	return nil
}

// modeFamilyGroup is one row of the family selector: a display name and the
// indices (into `modes`) of every submode that belongs to it, in table order.
// The operate/config screen picks a family first, then a specific mode within
// it — turning one ~200-entry cycle into a family cycle plus a short submode
// cycle.
type modeFamilyGroup struct {
	name  string
	modes []int
}

// families is the ordered family list, computed once from the modes table so a
// family's membership can never drift from the source of truth. Order follows
// each family's first appearance in `modes`.
var families = buildFamilies()

func buildFamilies() []modeFamilyGroup {
	var out []modeFamilyGroup
	idx := make(map[string]int, len(modes))
	for i := range modes {
		name := familyName(modes[i].label)
		fi, ok := idx[name]
		if !ok {
			fi = len(out)
			idx[name] = fi
			out = append(out, modeFamilyGroup{name: name})
		}
		out[fi].modes = append(out[fi].modes, i)
	}
	return out
}

// familyName classifies a bare mode label into the display family it belongs to.
// The big prefix families (PSK/DominoEX/THOR/MFSK/…) collapse dozens of submodes
// into one selectable group; standalone modes (CW, RTTY, FT8, …) form a family
// of one so the cascading selector treats every mode the same way.
func familyName(label string) string {
	switch label {
	case "cw":
		return "CW"
	case "rtty":
		return "RTTY"
	case "olivia":
		return "Olivia"
	case "afsk1200":
		return "Packet"
	case "navtex", "sitorb":
		return "NAVTEX / SITOR-B"
	case "wefax576", "wefax288":
		return "WEFAX"
	case "ft8":
		return "FT8"
	case "ft4":
		return "FT4"
	case "jt65":
		return "JT65"
	case "jt9":
		return "JT9"
	case "fst4":
		return "FST4"
	case "msk144":
		return "MSK144"
	case "wspr":
		return "WSPR"
	case "js8":
		return "JS8"
	case "adsb":
		return "ADS-B"
	}
	switch {
	case strings.HasPrefix(label, "jt4"): // jt4a..jt4g
		return "JT4"
	case strings.HasPrefix(label, "qpsk"):
		return "QPSK"
	case strings.HasPrefix(label, "psk"):
		return pskFamily(label)
	case strings.HasPrefix(label, "dominoex"):
		return "DominoEX"
	case strings.HasPrefix(label, "thor"):
		return "THOR"
	case strings.HasPrefix(label, "ifkp"):
		return "IFKP"
	case strings.HasPrefix(label, "fsq"):
		return "FSQ"
	case strings.HasPrefix(label, "mfsk"):
		return "MFSK"
	case strings.HasPrefix(label, "contestia"):
		return "Contestia"
	case strings.HasPrefix(label, "mt63"):
		return "MT63"
	case strings.HasPrefix(label, "throb"): // throb + throbx
		return "Throb"
	case strings.Contains(label, "hell"): // feldhell/slowhell/hellx*/hell80
		return "Hell"
	}
	// Everything left is an SSTV colour/mono submode (all shape "image"); WEFAX
	// and Hell — the other image modes — are handled above.
	if mi := modeByLabel(label); mi != nil && mi.shape == "image" {
		return "SSTV"
	}
	return "Other"
}

// pskFamily splits the PSK label space into the fldigi-style sub-families: plain
// BPSK, robust (…r/…f), robust multi-carrier (…rc…), and plain multi-carrier
// (…c…). QPSK is handled by the caller before this runs.
func pskFamily(label string) string {
	rest := strings.TrimPrefix(label, "psk")
	switch {
	case strings.Contains(rest, "rc"):
		return "PSK-RC"
	case strings.HasSuffix(rest, "r"), strings.HasSuffix(rest, "f"):
		return "PSK-R"
	case strings.ContainsRune(rest, 'c'):
		return "PSK-C"
	default:
		return "PSK"
	}
}

// familyIdxOfMode returns the index into `families` of the family containing the
// mode at modeIdx (0 if somehow unclassified).
func familyIdxOfMode(modeIdx int) int {
	name := familyName(modes[modeIdx].label)
	for i := range families {
		if families[i].name == name {
			return i
		}
	}
	return 0
}

// familyModePos returns the position of modeIdx within a family's submode list
// (0 if not present, which shouldn't happen for a family's own modes).
func familyModePos(fam modeFamilyGroup, modeIdx int) int {
	for pos, mi := range fam.modes {
		if mi == modeIdx {
			return pos
		}
	}
	return 0
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
	case "throb1", "throb2", "throb4", "throbx1", "throbx2", "throbx4":
		// The fldigi Throb / ThrobX family: submode label + audio center (1500 Hz).
		return &pb.ModeParams{Params: &pb.ModeParams_Throb{Throb: &pb.ThrobParams{
			Submode: label, CenterHz: float32(get("center", 1500)),
		}}}
	case "navtex", "sitorb":
		// NAVTEX / SITOR-B: submode label + audio center (1000 Hz).
		return &pb.ModeParams{Params: &pb.ModeParams_Navtex{Navtex: &pb.NavtexParams{
			Submode: label, CenterHz: float32(get("center", 1000)),
		}}}
	case "wefax576", "wefax288":
		// WEFAX facsimile: submode label + audio carrier (1900 Hz).
		return &pb.ModeParams{Params: &pb.ModeParams_Wefax{Wefax: &pb.WefaxParams{
			Submode: label, CenterHz: float32(get("center", 1900)),
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

// modeStringFor builds the ConfigureChannel `mode` string for a mode. Most modes
// carry their settings in a typed ModeParams message and just need the bare
// label here; but FST4, JS8, and MSK144 have no typed proto message — the daemon
// reads their extra parameters from the mode string's `:key=value` tail
// (ModeConfig::parse), so this appends that tail from the settings-form values.
// vals is the form's raw string values (SettingsForm.Values()).
func modeStringFor(label string, vals map[string]string) string {
	pick := func(k, d string) string {
		if v, ok := vals[k]; ok && v != "" {
			return v
		}
		return d
	}
	switch label {
	case "fst4":
		return "fst4:tr=" + pick("tr", "15")
	case "js8":
		return "js8:sub=" + pick("sub", "normal")
	case "msk144":
		return "msk144:freq=" + pick("freq", "1500")
	default:
		return label
	}
}

// modeStringParam reads a numeric key from a mode string's `:key=value` tail
// (e.g. modeStringParam("fst4:tr=300", "tr", 15) == 300), returning def when the
// mode has no tail or the key is absent/unparseable.
func modeStringParam(mode, key string, def float64) float64 {
	_, tail, ok := strings.Cut(mode, ":")
	if !ok {
		return def
	}
	for _, kv := range strings.Split(tail, ",") {
		if k, val, ok := strings.Cut(kv, "="); ok && k == key {
			if f, err := strconv.ParseFloat(val, 64); err == nil {
				return f
			}
		}
	}
	return def
}
