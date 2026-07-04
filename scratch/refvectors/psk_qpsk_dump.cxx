// psk_qpsk_dump.cxx — fldigi QPSK FEC golden-vector extractor.
//
// Links the *unmodified* fldigi convolutional encoder (src/filters/viterbi.cxx,
// `encoder` class) and emits, for the PSK31-Varicode bitstream of a fixed
// message, the exact QPSK code-symbol sequence fldigi feeds its differential
// modulator: for each varicode bit, `enc.encode(bit)` returns a 2-bit symbol
// s = parity(poly1 & shreg) | (parity(poly2 & shreg) << 1), s in 0..3. That
// symbol stream (before the (4-s)&3 constellation remap) is the bit-exact FEC
// domain omnimodem's ConvCode(k=5, [0x17,0x19]).encode must reproduce.
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   source:   src/filters/viterbi.cxx:218-256 (encoder), src/psk/pskvaricode.cxx,
//             FEC constants src/psk/psk.cxx:66-68 (K=5, POLY1=0x17, POLY2=0x19)
//   build:    scratch/refvectors/build_psk_qpsk.sh
//
// Output: JSON lines: {"msg":"...","varicode_bits":"01..","qpsk_symbols":"s s .."}

#include <cstdio>
#include "viterbi.h"
#include "pskvaricode.h"

// fldigi misc.cxx: parity(w) = hweight32(w) & 1 (even parity / popcount LSB).
int parity(unsigned long w) { return __builtin_popcountl(w) & 1; }

int main() {
    const char *msg = "CQ DE K1ABC";

    // Varicode payload bits (codeword + "00" separators), MSB-first.
    printf("{\"msg\":\"%s\",\"varicode_bits\":\"", msg);
    for (const char *p = msg; *p; ++p) {
        fputs(psk_varicode_encode((unsigned char)*p), stdout);
        fputs("00", stdout);
    }
    printf("\",\"qpsk_symbols\":\"");

    // K=5, POLY1=0x17, POLY2=0x19 (psk.cxx:66-68). One symbol per varicode bit.
    encoder enc(5, 0x17, 0x19);
    bool first = true;
    for (const char *p = msg; *p; ++p) {
        const char *cw = psk_varicode_encode((unsigned char)*p);
        // codeword bits then the two "00" separator bits
        char bits[64];
        int n = 0;
        for (const char *b = cw; *b; ++b) bits[n++] = *b - '0';
        bits[n++] = 0;
        bits[n++] = 0;
        for (int i = 0; i < n; ++i) {
            int s = enc.encode(bits[i]) & 3;
            if (!first) fputc(' ', stdout);
            printf("%d", s);
            first = false;
        }
    }
    printf("\"}\n");
    return 0;
}
