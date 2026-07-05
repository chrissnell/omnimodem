// thor_dump.cxx — fldigi THOR golden-vector extractor.
//
// Reproduces the *unmodified* fldigi THOR TX pipeline (thor::sendchar +
// thor::sendsymbol, reverse=false) by wiring the real fldigi leaf components —
// the K=7/K=15 convolutional encoder (src/filters/viterbi.cxx), the size-4 MFSK
// diagonal interleaver (src/mfsk/interleave.cxx), the IZ8BLY MFSK varicode
// (src/mfsk/mfskvaricode.cxx) and the THOR secondary varicode
// (src/thor/thorvaricode.cxx) — exactly as thor.cxx does. Nothing here is
// modified reference code; the driver only sequences the leaf objects the way
// the modem class would, minus the FLTK/modem runtime.
//
// For a fixed message it dumps every bit/integer-domain stage so a break
// localises:
//   varicode  — concatenated MFSK-varicode '0'/'1' bits of the message
//   codepairs — the 2-bit convolutional-encoder outputs (poly1 in LSB, poly2
//               in bit1), one pair per varicode bit, encoder state carried
//               across the whole message (streaming, no per-char flush)
//   nibbles   — 4-bit words assembled from the code pairs (MSB-first shift)
//   inlv      — nibbles after the size-4 forward interleaver
//   tones     — IFK+ tone indices: tone = (prevtone + 2 + inlv) % 18
//
// THOR16 (K=7, idepth=10) is the default vector; THOR100 (K=15, idepth=50)
// locks the long-constraint-length encoder path.
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   sources:  src/filters/viterbi.cxx, src/mfsk/interleave.cxx,
//             src/mfsk/mfskvaricode.cxx, src/thor/thorvaricode.cxx
//   build:    scratch/refvectors/build_thor.sh
//
// Output: one JSON object per line, consumed by
//   crates/dsp/tests/vectors/thor_varicode.json

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "interleave.h"
#include "viterbi.h"
#include "thorvaricode.h"

// viterbi.cxx references the global parity(); provide it standalone so we do not
// have to link the whole src/misc/misc.cxx (identical definition).
int parity(unsigned long w)
{
    int p = 0;
    while (w) { p ^= (w & 1); w >>= 1; }
    return p;
}

#define THORNUMTONES 18
#define THOR_K   7
#define THOR_POLY1 0x6d
#define THOR_POLY2 0x4f
#define THOR_K15  15
#define K15_POLY1 044735
#define K15_POLY2 063057

struct Stages {
    std::string varicode;
    std::vector<int> codepairs;
    std::vector<int> nibbles;
    std::vector<int> inlv;
    std::vector<int> tones;
};

// Reproduce thor::sendchar / sendsymbol for a whole message.
static Stages run(const std::string& msg, int k, int poly1, int poly2, int idepth)
{
    Stages st;
    encoder Enc(k, poly1, poly2);
    interleave Txinlv(4, idepth, INTERLEAVE_FWD);

    int txprevtone = 0, bitstate = 0;
    unsigned int bitshreg = 0;

    for (unsigned char ch : msg) {
        const char* code = thorvarienc(ch, 0); // primary set → MFSK varicode
        while (*code) {
            int bit = *code++ - '0';
            st.varicode.push_back('0' + bit);
            int data = Enc.encode(bit);
            st.codepairs.push_back(data & 3);
            for (int i = 0; i < 2; i++) {
                bitshreg = (bitshreg << 1) | ((data >> i) & 1);
                bitstate++;
                if (bitstate == 4) {
                    st.nibbles.push_back(bitshreg);
                    Txinlv.bits(&bitshreg);
                    st.inlv.push_back(bitshreg);
                    int tone = (txprevtone + 2 + bitshreg) % THORNUMTONES;
                    txprevtone = tone;
                    st.tones.push_back(tone);
                    bitstate = 0;
                    bitshreg = 0;
                }
            }
        }
    }
    return st;
}

static void emit(const char* name, const std::string& msg, const Stages& st, int idepth, int k)
{
    printf("{\"mode\":\"%s\",\"msg\":\"%s\",\"k\":%d,\"idepth\":%d,",
           name, msg.c_str(), k, idepth);
    printf("\"varicode\":\"%s\",", st.varicode.c_str());
    printf("\"codepairs\":\"");
    for (size_t i = 0; i < st.codepairs.size(); i++) printf("%s%d", i ? " " : "", st.codepairs[i]);
    printf("\",\"nibbles\":\"");
    for (size_t i = 0; i < st.nibbles.size(); i++) printf("%s%d", i ? " " : "", st.nibbles[i]);
    printf("\",\"inlv\":\"");
    for (size_t i = 0; i < st.inlv.size(); i++) printf("%s%d", i ? " " : "", st.inlv[i]);
    printf("\",\"tones\":\"");
    for (size_t i = 0; i < st.tones.size(); i++) printf("%s%d", i ? " " : "", st.tones[i]);
    printf("\"}\n");
}

// Dump the THOR secondary-varicode encode table (' '..'z') and, for each, the
// varidec index the RX shift register accumulates and decodes back. 'code' is
// the '0'/'1' codeword thorvarienc(c,1) returns; 'val' is its integer value
// (== the shreg>>1 the framer decodes); 'dec' = thorvaridec(val) (== c|0x100).
static void emit_secondary()
{
    printf("{\"secondary\":[");
    bool first = true;
    for (int c = ' '; c <= 'z'; c++) {
        const char* code = thorvarienc(c, 1);
        unsigned int val = 0;
        for (const char* p = code; *p; p++) val = (val << 1) | (*p - '0');
        int dec = thorvaridec(val);
        printf("%s{\"c\":%d,\"code\":\"%s\",\"val\":%u,\"dec\":%d}",
               first ? "" : ",", c, code, val, dec);
        first = false;
    }
    printf("]}\n");
}

int main()
{
    const std::string msg = "CQ DE K1ABC";
    emit("thor16", msg, run(msg, THOR_K, THOR_POLY1, THOR_POLY2, 10), 10, THOR_K);
    emit("thor100", msg, run(msg, THOR_K15, K15_POLY1, K15_POLY2, 50), 50, THOR_K15);
    emit_secondary();
    return 0;
}
