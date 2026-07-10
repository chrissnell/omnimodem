// dominoex_varicode_dump.cxx — fldigi DominoEX Varicode + IFK+ golden extractor.
//
// Links the *unmodified* fldigi DominoEX varicode table
// (src/dominoex/dominovar.cxx) and emits, for a fixed message and for the whole
// primary alphabet:
//   1. the per-character varicode nibble sequence that dominoex::sendchar feeds
//      dominoex::sendsymbol (code[0], then code[1..2] while the MSB (0x8) is set);
//   2. the IFK+ tone-index sequence sendsymbol produces
//      (tone = (txprevtone + 2 + sym) % NUMTONES, txprevtone := tone, start 0);
//   3. the decode round-trip: the varidec index that dominoex::decodeDomino
//      accumulates for each character (newest nibble in bits 3:0), and the ASCII
//      it decodes to — proving both tables and the framing index formula.
//
// The varicode tables (dominoex_varienc/dominoex_varidec) are the bit-exact
// reference; the ~6 lines of sendchar/sendsymbol/decodeDomino framing glue are
// transcribed here verbatim with cites, since that glue lives in the modem class
// (dominoex.cxx) which cannot link standalone (FLTK/modem runtime).
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   source:   src/dominoex/dominovar.cxx (varicode/varidecode + enc/dec)
//             src/dominoex/dominoex.cxx:651-681 (sendsymbol/sendchar),
//                                      368-395 (decodeDomino framing)
//   build:    scratch/refvectors/build_dominoex_varicode.sh
//
// Output: JSON lines to stdout.

#include <cstdio>
#include <cstring>
#include "dominovar.h"

#define NUMTONES 18 // ref: dominoex.h:46

// ref: dominoex.cxx:664-681 sendchar (non-FEC path) — the nibbles actually sent.
static int char_nibbles(unsigned char c, int secondary, unsigned char out[3]) {
    unsigned char *code = dominoex_varienc(c, secondary);
    int n = 0;
    out[n++] = code[0];
    for (int sym = 1; sym < 3; sym++) {
        if (code[sym] & 0x8) out[n++] = code[sym];
        else break;
    }
    return n;
}

// ref: dominoex.cxx:651-661 sendsymbol — IFK+ tone advance, reverse=false.
static int ifk_tone(int *txprevtone, int sym) {
    int tone = (*txprevtone + 2 + sym) % NUMTONES;
    *txprevtone = tone;
    return tone;
}

// ref: dominoex.cxx:372-395 decodeDomino — the accumulated varidec index for a
// character whose nibbles (in send order) are nib[0..n). symbolbuf[0] is newest.
static unsigned int decode_index(const unsigned char *nib, int n) {
    unsigned int sym = 0;
    for (int i = 0; i < n; i++)
        sym |= (unsigned int)nib[n - 1 - i] << (4 * i);
    return sym;
}

static void dump_msg(const char *msg) {
    printf("{\"msg\":\"%s\",\"nibbles\":\"", msg);
    int txprev = 0;
    // collect tones separately after printing nibbles
    unsigned char tones[512];
    int nt = 0;
    for (const char *p = msg; *p; ++p) {
        unsigned char nib[3];
        int n = char_nibbles((unsigned char)*p, 0, nib);
        for (int i = 0; i < n; i++) {
            printf("%x", nib[i]);
            tones[nt++] = (unsigned char)ifk_tone(&txprev, nib[i]);
        }
    }
    printf("\",\"tones\":[");
    for (int i = 0; i < nt; i++) printf(i ? ",%d" : "%d", tones[i]);
    printf("]}\n");
}

int main() {
    // Full primary alphabet: enc nibbles + decode round-trip.
    printf("{\"table\":\"primary\",\"rows\":[");
    for (int c = 0; c < 256; c++) {
        unsigned char nib[3];
        int n = char_nibbles((unsigned char)c, 0, nib);
        unsigned int idx = decode_index(nib, n);
        int dec = dominoex_varidec(idx);
        printf("%s{\"c\":%d,\"nib\":\"", c ? "," : "", c);
        for (int i = 0; i < n; i++) printf("%x", nib[i]);
        printf("\",\"idx\":%u,\"dec\":%d}", idx, dec);
    }
    printf("]}\n");

    dump_msg("CQ DE K1ABC");
    dump_msg("The quick brown fox 0123456789");
    return 0;
}
