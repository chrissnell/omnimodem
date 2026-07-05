// feldhell_dump.cxx — fldigi Feld Hell font + on-air column-stream golden extractor.
//
// Links the *unmodified* fldigi Feld Hell bitmap font tables
// (src/feld/feldfonts.cxx, which #includes all fifteen Feld*-{12,14}.cxx data
// files) and emits, for the default transmit font FeldHell-12 (progdefaults
// .feldfontnbr default = 4, i.e. `feldhell_12`; ref: configuration.h:989-991,
// feld.cxx:537):
//   1. "glyphs": for every printable ASCII char ' '..'~', the trimmed sequence
//      of 14-bit column values get_font_data(c, col) returns until it signals
//      end-of-character (-1). Bit `row` (0..13) of a column value is on iff that
//      pixel is set — this is the bit-exact glyph raster.
//   2. "streams": for fixed messages, the full on-air column stream tx_char emits
//      with HellXmtWidth = 1 (default; ref: configuration.h:689-691): a leading
//      null column, the per-character glyph columns (space -> 4 null columns),
//      and a trailing null column. This is the bit-exact on-air raster.
//
// The font tables are the bit-exact reference and are linked verbatim. The two
// pure-integer functions get_font_data and the tx_char column loop are
// transcribed here with cites, since feld.cxx cannot link standalone (it drags
// in the fltk/modem/ModulateXmtr runtime). Only the wire-determining integer
// logic is reproduced; no audio.
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   source:   src/feld/feldfonts.cxx + Feld*-{12,14}.cxx (font tables, verbatim)
//             src/feld/feld.cxx:521-561 (get_font_data)
//             src/feld/feld.cxx:633-659 (tx_char column loop)
//             src/include/configuration.h:689 (HellXmtWidth=1), :989 (feldfontnbr=4)
//   build:    scratch/refvectors/feldhell_dump.cxx via build_feldhell.sh
//
// Output: one JSON object to stdout.

#include <cstdio>
#include "fontdef.h"

// ref: feld.cxx:521-561 get_font_data — column bits for char `c`, column `col`,
// using bitmap font `font`. Returns -1 when column `col` and all lesser columns
// are blank (character complete). Verbatim but parameterized on the font pointer
// (fldigi selects it from progdefaults.feldfontnbr).
static int get_font_data(fntchr *font, unsigned char c, int col) {
    int bits = 0;
    int mask;
    int bin;
    int ordbits = 0;

    if (col > 15 || c < ' ' || c > '~')
        return -1;
    mask = 1 << (15 - col);

    for (int i = 0; i < 14; i++) ordbits |= font[c - ' '].byte[i];

    for (int row = 0; row < 14; row++) {
        bin = font[c - ' '].byte[13 - row] & mask;
        if (bin != 0)
            bits |= 1 << row;
    }
    int testval = (1 << (15 - col)) - 1;
    if ((bits == 0) && ((ordbits & testval) == 0))
        return -1;
    return bits;
}

// ref: feld.cxx:633-659 tx_char — the *column* stream (send_symbol audio elided).
// Appends the 14-bit column values one character contributes: a leading null
// column, then either 3 more null columns (space) or the glyph's columns repeated
// HellXmtWidth times, then a trailing null column. `emit` collects columns.
static void tx_char_columns(fntchr *font, int HellXmtWidth, char c,
                            int *out, int *n) {
    out[(*n)++] = 0; // send_null_column (leading)
    if (c == ' ') {
        out[(*n)++] = 0;
        out[(*n)++] = 0;
        out[(*n)++] = 0;
    } else {
        int column = 0, bits;
        while ((bits = get_font_data(font, (unsigned char)c, column)) != -1) {
            for (int col = 0; col < HellXmtWidth; col++)
                out[(*n)++] = bits;
            column++;
        }
    }
    out[(*n)++] = 0; // send_null_column (trailing)
}

static void dump_stream(fntchr *font, int xmtwidth, const char *msg) {
    static int cols[65536];
    int n = 0;
    for (const char *p = msg; *p; ++p)
        tx_char_columns(font, xmtwidth, *p, cols, &n);
    printf("{\"kind\":\"stream\",\"msg\":\"%s\",\"cols\":[", msg);
    for (int i = 0; i < n; i++) printf("%s%d", i ? "," : "", cols[i]);
    printf("]}\n");
}

int main() {
    fntchr *font = feldhell_12; // feldfontnbr default = 4
    const int HellXmtWidth = 1; // default

    // JSON-lines: a leading _meta provenance record, then one line per glyph and
    // one line per on-air column stream (mirrors dominoex_varicode.json).
    printf("{\"_meta\":{"
           "\"upstream\":\"w1hkj/fldigi 4.1.23 @ 61b97f413\","
           "\"files\":["
           "\"src/feld/feldfonts.cxx + Feld*-{12,14}.cxx (bitmap font tables, verbatim)\","
           "\"src/feld/feld.cxx:521-561 (get_font_data column bits)\","
           "\"src/feld/feld.cxx:633-659 (tx_char on-air column loop)\"],"
           "\"font\":\"feldhell_12 (feldfontnbr default 4)\",\"xmtwidth\":%d,"
           "\"driver\":\"scratch/refvectors/build_feldhell.sh\","
           "\"note\":\"cols are 14-bit column values; bit r (0..13) set iff pixel row r is on. bit-exact.\""
           "}}\n", HellXmtWidth);

    for (int c = ' '; c <= '~'; c++) {
        printf("{\"kind\":\"glyph\",\"c\":%d,\"cols\":[", c);
        int col = 0, bits, k = 0;
        while ((bits = get_font_data(font, (unsigned char)c, col)) != -1) {
            printf("%s%d", k ? "," : "", bits);
            col++;
            k++;
        }
        printf("]}\n");
    }
    dump_stream(font, HellXmtWidth, "CQ DE K1ABC");
    dump_stream(font, HellXmtWidth, "The quick brown fox 0123456789");
    return 0;
}
