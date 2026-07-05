// mt63_dump.cxx — fldigi MT63 encoder + modulator-phase golden extractor.
//
// Links the *unmodified* fldigi MT63 base modem (src/mt63/mt63base.cxx +
// src/mt63/dsp.cxx) and emits, for a fixed message and each (bandwidth,
// interleave) config, the two bit-exact integer intermediates that determine the
// wire:
//   1. "encoder": for every character, the 64-bit MT63encoder.Output vector
//      (1 = no phase flip, 0 = phase flip) — the Walsh inverse transform +
//      block-interleaver output. This is the sole novel bit-domain stage.
//   2. "txvect":  for every character, the 64 integer FFT-twiddle phase indices
//      TxVect[0..63] (0..511) after the differential phase accumulation in
//      MT63tx::SendChar — i.e. the exact per-carrier DBPSK constellation point
//      index placed on the IFFT before the (FP) window/overlap-add/comb audio
//      path. Bit-exact.
//
// To make the reference deterministic we re-Preset the transmitter's encoder
// with RandFill=0 (the live modem seeds the interleaver pipe with rand(); that
// randomness is an anti-strong-carrier startup measure and is irrelevant to the
// wire codec — the differential decoder recovers regardless). Everything dumped
// here is then a pure function of the message + config.
//
// MT63tx / MT63encoder are standalone DSP classes (no FLTK/modem runtime), so we
// read their internals directly via `#define private public` before including
// the header; the linked object files keep their normal layout (no virtuals).
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   source:   src/mt63/mt63base.cxx  (MT63encoder::Process, MT63tx::SendChar/
//                                      Preset — Walsh+interleave+phase accum,
//                                      verbatim)
//             src/mt63/dsp.cxx        (dspWalshInvTrans, verbatim)
//             src/mt63/symbol.dat     (SymbolLen/Separ, DataCarrSepar)
//             src/mt63/mt63intl.dat   (Short/Long interleave patterns)
//   build:    scratch/refvectors/mt63_dump.cxx via build_mt63.sh
//
// Output: one JSON object to stdout, consumed by crates/dsp/tests/vectors/mt63.json.

#include <cstdio>
#include <cstring>

#include "dsp.h"

// expose MT63tx / MT63encoder internals for the dump
#define private public
#include "mt63base.h"
#undef private

// the interleave pattern tables live in mt63intl.dat; symbol geometry in symbol.dat
extern int ShortIntlvPatt[64];
extern int LongIntlvPatt[64];

static const char *MSG = "CQ CQ DE K1ABC K1ABC/7 --.,?!";

struct Cfg { const char *name; int bw; int longintlv; double freq; };

static void dump_config(const Cfg &cfg, bool first) {
    MT63tx tx;
    // Preset builds TxVect/dspPhaseCorr and the (rand-filled) encoder.
    tx.Preset(cfg.freq, cfg.bw, cfg.longintlv);
    // Re-preset the encoder deterministically (RandFill = 0): identical Walsh +
    // interleaver, zeroed pipe. TxVect keeps its Preset-initialised values.
    int intlv = cfg.longintlv ? 64 : 32;
    int *patt = cfg.longintlv ? LongIntlvPatt : ShortIntlvPatt;
    tx.Encoder.Preset(64, intlv, patt, 0);

    printf("%s\n    \"%s\": {\n", first ? "" : ",", cfg.name);
    printf("      \"bandwidth\": %d, \"interleave\": %d, \"freq\": %g,\n",
           cfg.bw, intlv, cfg.freq);
    printf("      \"first_data_carr\": %d,\n", tx.FirstDataCarr);
    printf("      \"encoder\": [");
    // collect both streams in one pass over the message
    // encoder bits
    size_t n = strlen(MSG);
    // First pass: encoder Output per char
    for (size_t j = 0; j < n; j++) {
        tx.Encoder.Process(MSG[j]);
        printf("%s\"", j ? "," : "");
        for (int i = 0; i < 64; i++) printf("%c", '0' + tx.Encoder.Output[i]);
        printf("\"");
    }
    printf("],\n");

    // Second pass (fresh deterministic state): TxVect phase indices per char.
    // Re-Preset to reset TxVect + encoder pipe to the initial state.
    tx.Preset(cfg.freq, cfg.bw, cfg.longintlv);
    tx.Encoder.Preset(64, intlv, patt, 0);
    printf("      \"txvect\": [");
    int mask = tx.FFT.Size - 1;
    int flip = tx.FFT.Size / 2;
    for (size_t j = 0; j < n; j++) {
        tx.Encoder.Process(MSG[j]);
        // MT63tx::SendChar phase accumulation (ref: mt63base.cxx:262-271)
        for (int i = 0; i < 64; i++) {
            if (tx.Encoder.Output[i])
                tx.TxVect[i] = (tx.TxVect[i] + tx.dspPhaseCorr[i]) & mask;
            else
                tx.TxVect[i] = (tx.TxVect[i] + tx.dspPhaseCorr[i] + flip) & mask;
        }
        printf("%s[", j ? "," : "");
        for (int i = 0; i < 64; i++) printf("%s%d", i ? "," : "", tx.TxVect[i]);
        printf("]");
    }
    printf("]\n    }");
}

int main() {
    Cfg cfgs[] = {
        {"mt63_500s",  500,  0, 1500.0},
        {"mt63_1000s", 1000, 0, 1500.0},
        {"mt63_1000l", 1000, 1, 1500.0},
        {"mt63_2000s", 2000, 0, 1500.0},
    };
    printf("{\n");
    printf("  \"_provenance\": \"fldigi 4.1.23 @61b97f413; src/mt63/{mt63base,dsp}.cxx; "
           "scratch/refvectors/mt63_dump.cxx via build_mt63.sh\",\n");
    printf("  \"message\": \"%s\",\n", MSG);
    // symbol geometry (ref: src/mt63/symbol.dat) — const-int, internal linkage
    printf("  \"symbol_len\": %d, \"symbol_separ\": %d, \"data_carr_separ\": %d,\n",
           512, 200, 4);
    printf("  \"configs\": {");
    int nc = sizeof(cfgs) / sizeof(cfgs[0]);
    for (int i = 0; i < nc; i++) dump_config(cfgs[i], i == 0);
    printf("\n  }\n}\n");
    return 0;
}
