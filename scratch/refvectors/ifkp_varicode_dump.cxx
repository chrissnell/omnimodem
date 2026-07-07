// ifkp_varicode_dump.cxx — fldigi IFKP Varicode + IFK golden extractor.
//
// Links the *unmodified* fldigi IFKP varicode tables
// (src/ifkp/ifkp_varicode.cxx: ifkp_varicode[256][2] / ifkp_varidecode[]) and
// emits, for a fixed message and for the whole 256-char alphabet:
//   1. the per-character varicode nibble sequence ifkp::send_char feeds
//      ifkp::send_symbol: sym1 = ifkp_varicode[ch][0], and sym2 =
//      ifkp_varicode[ch][1] only when sym2 > 28;
//   2. the IFK tone-index sequence send_symbol produces
//      (tone = (prevtone + sym + IFKP_OFFSET) % 33, prevtone := tone, start 0);
//   3. the streaming decode: ifkp::process_symbol's prev_nibble state machine
//      run over the nibble stream, and the ASCII it produces — proving both
//      the encode table, the tone framing, and the varidecode table.
//
// The varicode tables are the bit-exact reference; the ~10 lines of
// send_char / send_symbol / process_symbol framing glue are transcribed here
// verbatim with cites, since that glue lives in the modem class (ifkp.cxx)
// which cannot link standalone (FLTK / modem runtime).
//
// Provenance:
//   upstream: fldigi 4.2.x, checked out at ../fldigi
//   source:   src/ifkp/ifkp_varicode.cxx (ifkp_varicode / ifkp_varidecode)
//             src/ifkp/ifkp.cxx:706-728 (send_symbol/send_char),
//                                423-480 (process_symbol streaming decode)
//             src/include/ifkp.h:45-46 (IFKP_SPACING, IFKP_OFFSET)
//   build:    scratch/refvectors/build_ifkp_varicode.sh
//
// Output: JSON lines to stdout.

#include <cstdio>

// ref: src/ifkp/ifkp_varicode.cxx (tables, unmodified).
extern int ifkp_varicode[][2];
extern int ifkp_varidecode[];

#define IFKP_OFFSET 1 // ref: ifkp.h:46

// ref: ifkp.cxx:706-728 send_symbol/send_char — the nibbles actually sent.
static int char_nibbles(int ch, int out[2]) {
    int n = 0;
    int sym1 = ifkp_varicode[ch][0];
    int sym2 = ifkp_varicode[ch][1];
    out[n++] = sym1;
    if (sym2 > 28) out[n++] = sym2;
    return n;
}

// ref: ifkp.cxx:706-710 send_symbol — IFK tone advance, 33 tones, +OFFSET.
static int ifk_tone(int *prevtone, int sym) {
    int tone = (*prevtone + sym + IFKP_OFFSET) % 33;
    *prevtone = tone;
    return tone;
}

// ref: ifkp.cxx:423-480 process_symbol — streaming prev_nibble decode. At the
// integer/tone level the received nibble equals the transmitted sym (the
// nibbles[] table inverts the +OFFSET tone advance), so we decode straight
// from the nibble stream. Returns the ASCII char, or -1 when none completes.
static int prev_nibble = 0;
static int decode_nibble(int curr_nibble) {
    int curr_ch = -1;
    if (prev_nibble < 29 && curr_nibble < 29)
        curr_ch = ifkp_varidecode[prev_nibble];
    else if (prev_nibble < 29 && curr_nibble > 28 && curr_nibble < 32)
        curr_ch = ifkp_varidecode[prev_nibble * 32 + curr_nibble];
    prev_nibble = curr_nibble;
    return curr_ch;
}

static void dump_msg(const char *msg) {
    // Encode: symbol (nibble) stream + IFK tone stream. Symbols range 0..32, so
    // both are emitted as integer arrays (not a hex string).
    int prevtone = 0;
    int tones[1024];
    int nt = 0;
    int nibs[1024];
    int nn = 0;
    for (const char *p = msg; *p; ++p) {
        int nib[2];
        int n = char_nibbles((unsigned char)*p, nib);
        for (int i = 0; i < n; i++) {
            nibs[nn++] = nib[i];
            tones[nt++] = ifk_tone(&prevtone, nib[i]);
        }
    }
    printf("{\"msg\":\"%s\",\"syms\":[", msg);
    for (int i = 0; i < nn; i++) printf(i ? ",%d" : "%d", nibs[i]);
    printf("],\"tones\":[");
    for (int i = 0; i < nt; i++) printf(i ? ",%d" : "%d", tones[i]);
    printf("],\"decode\":\"");
    // Decode: stream the nibbles through the state machine. Append two idle
    // symbols (sym 0) at end to flush the trailing single-nibble character,
    // exactly as fldigi's continuous idle does.
    prev_nibble = 0;
    for (int i = 0; i < nn; i++) {
        int c = decode_nibble(nibs[i]);
        if (c > 0) printf("%c", c);
    }
    for (int i = 0; i < 2; i++) {
        int c = decode_nibble(0);
        if (c > 0) printf("%c", c);
    }
    printf("\"}\n");
}

int main() {
    // Full alphabet: raw table sym1/sym2 for every character 0..255.
    printf("{\"table\":\"ifkp_varicode\",\"rows\":[");
    for (int c = 0; c < 256; c++) {
        printf("%s[%d,%d]", c ? "," : "", ifkp_varicode[c][0], ifkp_varicode[c][1]);
    }
    printf("]}\n");

    // varidecode table (896 entries: prev in 0..28, curr in 0..32).
    printf("{\"table\":\"ifkp_varidecode\",\"rows\":[");
    for (int i = 0; i < 29 * 32; i++) {
        printf("%s%d", i ? "," : "", ifkp_varidecode[i]);
    }
    printf("]}\n");

    dump_msg("CQ CQ CQ de K1ABC");
    dump_msg("The quick brown fox 0123456789!");
    dump_msg("hello world");
    return 0;
}
