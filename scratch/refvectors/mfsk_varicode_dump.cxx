// mfsk_varicode_dump.cxx — fldigi MFSK (IZ8BLY) Varicode golden-vector extractor.
//
// Links the *unmodified* fldigi MFSK varicode table (src/mfsk/mfskvaricode.cxx,
// `varienc`/`varidec`) and emits, for a fixed message, the exact self-framed bit
// stream fldigi's PSK-R/+F path feeds its FEC (`tx_char` -> `varienc(c)`, bits
// concatenated with NO separator — the codewords are self-delimiting), plus the
// round-trip decode through the same shreg framing fldigi's `rx_bit` uses
// (`shreg=(shreg<<1)|bit; if((shreg&7)==1){ varidec(shreg>>1); shreg=1; }`).
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   source:   src/mfsk/mfskvaricode.cxx:35-349 (varicode[]/varidecode[]),
//             framing src/psk/psk.cxx:1117-1123 (rx_bit MFSK path)
//   build:    scratch/refvectors/build_mfsk_varicode.sh
//
// Output: JSON: {"msg":"...","mfsk_bits":"01..","decoded":"..."}

#include <cstdio>
#include <string>
#include "mfskvaricode.h"

int main() {
    const char *msg = "CQ DE K1ABC";

    // Encode: concatenate varienc(c) codeword bits, no separator (tx_char pskr).
    std::string bits;
    for (const char *p = msg; *p; ++p) {
        for (const char *code = varienc((unsigned char)*p); *code; ++code) {
            bits.push_back(*code);
        }
    }

    // Decode: fldigi rx_bit MFSK framing.
    std::string decoded;
    unsigned int shreg = 0;
    for (char b : bits) {
        shreg = (shreg << 1) | (b - '0');
        if ((shreg & 7) == 1) {
            int c = varidec(shreg >> 1);
            if (c != -1 && c != 0) decoded.push_back((char)c);
            shreg = 1;
        }
    }

    printf("{\"msg\":\"%s\",\"mfsk_bits\":\"%s\",\"decoded\":\"%s\"}\n",
           msg, bits.c_str(), decoded.c_str());
    return 0;
}
