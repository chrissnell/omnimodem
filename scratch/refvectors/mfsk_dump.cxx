// mfsk_dump.cxx — fldigi MFSK TX-chain golden-vector extractor.
//
// Composes the *unmodified* fldigi MFSK data-path primitives in the exact order
// mfsk.cxx's sendchar/sendbit/sendsymbol use them, and dumps every stage
// intermediate for a fixed message across the representative submodes:
//   varicode(text)          — src/mfsk/mfskvaricode.cxx  varienc()
//   → streaming K=7 conv     — src/filters/viterbi.cxx     encoder(7,0x6d,0x4f)
//   → size×depth interleave  — src/mfsk/interleave.cxx     interleave(symbits,depth,FWD)
//   → 8-bit grayencode       — src/misc/misc.cxx           grayencode()  (tone index)
//
// This is the "data portion" of the wire (no STX/EOT/preamble framing envelope),
// matching the deferred-framing precedent the Phase-9 DominoEX port established:
// the wire-determining arithmetic (varicode/FEC/interleave/gray) is asserted
// bit-exact; the framing envelope + AFC/sync are gated on loopback + the
// #[ignore] cross-decode, not on this vector.
//
// `parity` and `grayencode` are inlined verbatim from the cited fldigi sources
// (misc.cxx) so we need not drag misc.cxx's FLTK/config deps into the link —
// the same technique psk_robust_dump.cxx uses for `parity`.
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   sources:  src/mfsk/mfskvaricode.cxx, src/filters/viterbi.cxx,
//             src/mfsk/interleave.cxx, src/misc/misc.cxx (grayencode/parity)
//   build:    scratch/refvectors/build_mfsk.sh
//
// Output: one JSON object per line:
//   {"mode":"mfsk16","symbits":4,"depth":10,"numtones":16,"msg":"...",
//    "varicode":"0101..","coded":"0110..","symbols":"s s ..","tones":"t t .."}

#include <cstdio>
#include <cstring>
#include "viterbi.h"
#include "interleave.h"

// ref: src/misc/misc.cxx:67 parity()
int parity(unsigned long w) { return __builtin_popcountl(w) & 1; }

// ref: src/mfsk/mfskvaricode.cxx:varienc (declared in mfskvaricode.h)
extern const char *varienc(int c);

// ref: src/misc/misc.cxx:123 grayencode() — note fldigi's grayencode is the
// full XOR-cascade (gray→binary), the inverse of the conventional n^(n>>1).
static unsigned char grayencode(unsigned char data) {
    unsigned char bits = data;
    bits ^= data >> 1;
    bits ^= data >> 2;
    bits ^= data >> 3;
    bits ^= data >> 4;
    bits ^= data >> 5;
    bits ^= data >> 6;
    bits ^= data >> 7;
    return bits;
}

struct Sub { const char *name; int symbits; int depth; };

static void dump(const Sub &s, const char *msg) {
    const int numtones = 1 << s.symbits; // fldigi: numtones == 2^symbits
    encoder enc(7, 0x6d, 0x4f);          // NASA_K, POLY1, POLY2 (mfsk.h:54-56)
    interleave txinlv(s.symbits, s.depth, INTERLEAVE_FWD);

    // 1) varicode bits (self-framed, no inter-char separator) — ref sendchar.
    char varibits[8192];
    int nvar = 0;
    for (const char *p = msg; *p; ++p) {
        const char *code = varienc((unsigned char)*p);
        while (*code) varibits[nvar++] = *code++;
    }

    // 2) streaming conv-encode + 3) interleave + 4) grayencode → tones.
    //    ref: sendbit (data=enc.encode(bit); for i in 0..2 push (data>>i)&1),
    //    sendsymbol (grayencode(sym & (numtones-1))).
    char coded[16384];
    int ncode = 0;
    unsigned int tones[16384];
    unsigned int symbols[16384];
    int ntone = 0;
    unsigned int bitshreg = 0;
    int bitstate = 0;
    for (int k = 0; k < nvar; ++k) {
        int data = enc.encode(varibits[k] - '0');
        for (int i = 0; i < 2; ++i) {
            int cb = (data >> i) & 1;
            coded[ncode++] = '0' + cb;
            bitshreg = (bitshreg << 1) | cb;
            bitstate++;
            if (bitstate == s.symbits) {
                txinlv.bits(&bitshreg);
                symbols[ntone] = bitshreg;
                tones[ntone] = grayencode(bitshreg & (numtones - 1));
                ntone++;
                bitstate = 0;
                bitshreg = 0;
            }
        }
    }

    printf("{\"mode\":\"%s\",\"symbits\":%d,\"depth\":%d,\"numtones\":%d,\"msg\":\"%s\",\"varicode\":\"",
           s.name, s.symbits, s.depth, numtones, msg);
    for (int i = 0; i < nvar; ++i) putchar(varibits[i]);
    printf("\",\"coded\":\"");
    for (int i = 0; i < ncode; ++i) putchar(coded[i]);
    printf("\",\"symbols\":\"");
    for (int i = 0; i < ntone; ++i) printf("%s%u", i ? " " : "", symbols[i]);
    printf("\",\"tones\":\"");
    for (int i = 0; i < ntone; ++i) printf("%s%u", i ? " " : "", tones[i]);
    printf("\"}\n");
}

int main() {
    const char *msg = "CQ DE K1ABC";
    // Representative submodes spanning every (symbits, depth) combination the
    // family uses: symbits 3/4/5, depth 5/10/20 and the deep-interleave L modes.
    const Sub subs[] = {
        {"mfsk4", 5, 5},   {"mfsk8", 5, 5},
        {"mfsk16", 4, 10}, {"mfsk32", 4, 10}, {"mfsk64", 4, 10}, {"mfsk11", 4, 10}, {"mfsk22", 4, 10},
        {"mfsk128", 4, 20},
        {"mfsk31", 3, 10},
        {"mfsk64l", 4, 400}, {"mfsk128l", 4, 800},
    };
    for (const Sub &s : subs) dump(s, msg);
    return 0;
}
