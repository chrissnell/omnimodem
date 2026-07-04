// psk_robust_dump.cxx — fldigi PSK-R (robust) FEC golden-vector extractor.
//
// Links fldigi's real convolutional `encoder` (src/filters/viterbi.cxx) and MFSK
// varicode (src/mfsk/mfskvaricode.cxx) and emits, for a message, the exact K=7
// code-symbol sequence the PSK-R / +F path feeds its differential BPSK modulator:
// MFSK-varicode bits -> `enc.encode(bit)` (PSKR_K=7, PSKR_POLY1=0x6d,
// PSKR_POLY2=0x4f) -> a 2-bit `bitshreg` value (before the interleaver, which
// PSK63F skips). ref: psk.cxx:70-74 (constants), 983-992 (enc init), 2335-2342
// (tx_bit pskr). The two bits are sent low-bit-first as two BPSK symbols.
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   build:    scratch/refvectors/build_psk_robust.sh
//
// Output: JSON: {"msg":"...","mfsk_bits":"01..","pskr_symbols":"s s .."}

#include <cstdio>
#include <string>
#include "viterbi.h"
#include "mfskvaricode.h"

int parity(unsigned long w) { return __builtin_popcountl(w) & 1; }

int main() {
    const char *msg = "CQ DE K1ABC";

    std::string bits;
    for (const char *p = msg; *p; ++p)
        for (const char *code = varienc((unsigned char)*p); *code; ++code)
            bits.push_back(*code);

    printf("{\"msg\":\"%s\",\"mfsk_bits\":\"%s\",\"pskr_symbols\":\"", msg, bits.c_str());
    encoder enc(7, 0x6d, 0x4f); // PSKR_K, PSKR_POLY1, PSKR_POLY2
    bool first = true;
    for (char b : bits) {
        int s = enc.encode(b - '0') & 3;
        if (!first) fputc(' ', stdout);
        printf("%d", s);
        first = false;
    }
    printf("\"}\n");
    return 0;
}
