package app

import "strings"

type macro struct {
	key  string // function-key label, e.g. "F1"
	name string // "CQ", "Call", "RST", "73", "Brag"
	text string // template with {mycall}/{call}/{rst} placeholders
}

type macroCtx struct{ myCall, theirCall, rst string }

var defaultMacros = []macro{
	{"F1", "CQ", "CQ CQ de {mycall} {mycall} K"},
	{"F2", "Call", "{call} de {mycall} {mycall}"},
	{"F3", "RST", "{call} de {mycall} ur {rst} {rst}"},
	{"F4", "73", "{call} de {mycall} 73 e e"},
	{"F5", "Brag", "{call} de {mycall} rig is omnimodem pwr 50w"},
}

func expandMacro(tmpl string, ctx macroCtx) string {
	r := strings.NewReplacer(
		"{mycall}", ctx.myCall,
		"{call}", ctx.theirCall,
		"{rst}", ctx.rst,
	)
	return r.Replace(tmpl)
}

func macroForKey(k string) string {
	idx := map[string]int{"f1": 0, "f2": 1, "f3": 2, "f4": 3, "f5": 4}[k]
	return defaultMacros[idx].text
}

// macroBar renders the F-key strip plus the TX/Halt affordances.
func macroBar() string {
	var parts []string
	for _, mc := range defaultMacros {
		parts = append(parts, mc.key+" "+mc.name)
	}
	return strings.Join(parts, "  ") + "      [Esc] HALT TX"
}
