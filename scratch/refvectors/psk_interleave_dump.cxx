// psk_interleave_dump.cxx — fldigi PSK-R (de)interleaver golden-vector extractor.
//
// Links the *unmodified* fldigi diagonal (de)interleaver (src/mfsk/interleave.cxx,
// `interleave` with size=2) and emits, for a fixed 2-bit input sequence and a
// given depth, the forward-interleaved output stream fldigi's PSK-R TX produces
// (`Txinlv->bits(&bitshreg)` per code pair; ref psk.cxx:2337) and the round-trip
// back through the reverse interleaver (`Rxinlv->symbols`), which reproduces the
// input delayed by the interleaver's fill latency.
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   source:   src/mfsk/interleave.cxx, src/include/interleave.h
//   build:    scratch/refvectors/build_psk_interleave.sh
//
// Output: JSON: {"depth":D,"in":"s s ..","fwd":"s s ..","rev":"s s .."}

#include <cstdio>
#include "interleave.h"

int main() {
    const int depth = 40; // PSK125R idepth
    // A deterministic 2-bit (0..3) input sequence.
    const int N = 60;
    interleave fwd(2, depth, INTERLEAVE_FWD);
    interleave rev(2, depth, INTERLEAVE_REV);

    unsigned int in[N], fout[N], rout[N];
    for (int i = 0; i < N; i++) in[i] = (i * 7 + 1) & 3;

    for (int i = 0; i < N; i++) {
        unsigned int b = in[i];
        fwd.bits(&b);
        fout[i] = b;
        // Deinterleave the forward output to show the compensating round-trip.
        unsigned int r = fout[i];
        rev.bits(&r);
        rout[i] = r;
    }

    printf("{\"depth\":%d,\"in\":\"", depth);
    for (int i = 0; i < N; i++) printf("%s%u", i ? " " : "", in[i]);
    printf("\",\"fwd\":\"");
    for (int i = 0; i < N; i++) printf("%s%u", i ? " " : "", fout[i]);
    printf("\",\"rev\":\"");
    for (int i = 0; i < N; i++) printf("%s%u", i ? " " : "", rout[i]);
    printf("\"}\n");
    return 0;
}
