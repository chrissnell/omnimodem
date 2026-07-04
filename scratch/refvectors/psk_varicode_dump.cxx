// psk_varicode_dump.cxx — fldigi PSK31 Varicode golden-vector extractor.
//
// Links the *unmodified* fldigi PSK31 varicode table (src/psk/pskvaricode.cxx)
// and emits, for a fixed message, the exact BPSK payload bitstream fldigi feeds
// its differential encoder: each character's varicode codeword (MSB-first, as
// written in varicodetab1[]) followed by the "00" inter-character separator.
// That bitstream is the bit-exact domain omnimodem's
// framing::varicode::encode(&PSK31, ...) must reproduce byte-for-byte.
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   source:   src/psk/pskvaricode.cxx:31-334 (varicodetab1 + psk_varicode_encode)
//   build:    scratch/refvectors/build_psk_varicode.sh
//
// Output: JSON lines, one per message, to stdout:
//   {"msg":"...","varicode_bits":"0101..."}

#include <cstdio>
#include <cstring>
#include "pskvaricode.h"

static void dump(const char *msg) {
    printf("{\"msg\":\"%s\",\"varicode_bits\":\"", msg);
    for (const char *p = msg; *p; ++p) {
        const char *cw = psk_varicode_encode((unsigned char)*p);
        fputs(cw, stdout);      // codeword bits, MSB-first
        fputs("00", stdout);    // inter-character separator
    }
    printf("\"}\n");
}

int main() {
    dump("CQ DE K1ABC");
    dump("The quick brown fox 0123456789");
    return 0;
}
