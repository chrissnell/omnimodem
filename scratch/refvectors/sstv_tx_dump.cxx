// F0 SSTV golden-vector harness — TX dump driver (GRA-289, Phase 17).
//
// Links the *unmodified* MMSSTV DSP core (sstv.cpp + fir.cpp, upstream n5ac/mmsstv
// @ 8060b5f) via the fake-<vcl.h> shim in scratch/refvectors/sstv_shim/, drives one
// SSTV mode's transmit chain over a deterministic test image, and dumps to stdout:
//   * the resolved per-mode line/channel timing params (from the linked CSSTVSET),
//   * the VIS + per-line SYMBOL sequence as (freq_hz, ms) — the bit-exact TX domain,
//   * PCM statistics from the linked CSSTVMOD tone renderer (VCO + BPF) — FP-tolerance.
//
// AUTHENTICITY / PROVENANCE:
//   - CSSTVSET (timing tables), CSSTVMOD (tone renderer/VCO/BPF): UNMODIFIED reference.
//   - The VIS header + TMmsstv::LineSCT line layout are TRANSCRIBED here (they are
//     methods of the VCL form class TMmsstv in Main.cpp and cannot be linked without
//     the whole GUI), with exact `// ref: Main.cpp:NNNN` cites and the bitmap pixel
//     source (pBitmapTX->Canvas->Pixels[x][y]) replaced by testPixel(x,y).
//   - OutHEAD()/CW-ID/repeater/FSK-ID are operator features, deliberately omitted:
//     the canonical on-air picture signal is VIS + line scan only.
//
// Build: scratch/refvectors/build_sstv_tx.sh (run from the omnimodem repo root).

#include <vcl.h>
#include "sstv.h"
#include <vector>
#include <string>
#include <cstdint>
#include <cstdio>

// ---- DSP globals the linked reference reads (declared extern in the shim) ----
// SSTVSET and g_dblToneOffset are DEFINED inside sstv.cpp; we only supply the
// ones the reference expects the app (ComLib.cpp/Main.cpp) to provide.
double   SampFreq = 11025.0;   // MMSSTV native rate (ref: Main.cpp:212)
double   SampBase = 11025.0;
SYSSET   sys;
extern CSSTVSET SSTVSET;       // defined in sstv.cpp

// ---- colour helper, verbatim (ref: ComLib.cpp:3491) ----
static int ColorToFreq(int d) {
    d = d * (2300 - 1500) / 256;
    return d + 1500;
}

// ---- deterministic test image: 8 vertical colour bars, 320 wide ----
// Packed 0x00BBGGRR to match VCL TColor byte order (COLD.b.r is the low byte).
static const uint32_t kBars[8] = {
    0x000000, 0x0000FF, 0x00FF00, 0x00FFFF, // black, red, green, yellow
    0xFF0000, 0xFF00FF, 0xFFFF00, 0xFFFFFF, // blue, magenta, cyan, white
};
static uint32_t testPixel(int x, int /*y*/) { return kBars[(x * 8) / 320]; }

// ---- symbol log: every (freq, ms) write, the bit-exact TX vector ----
struct Sym { int fq; double ms; };
static std::vector<Sym> g_syms;
static void W(CSSTVMOD* mp, int fq, double ms) { g_syms.push_back({fq, ms}); mp->Write(short(fq), ms); }

// ---- transcribed VIS header for the 8-bit standard modes (ref: Main.cpp:6940-7107) ----
static void emitVIS(CSSTVMOD* mp, int visCode) {
    W(mp, 1900, 300);
    W(mp, 1200, 10);
    W(mp, 1900, 300);
    W(mp, 1200, 30);
    int d = visCode;
    for (int i = 0; i < 8; i++) { W(mp, d & 1 ? 1100 : 1300, 30); d >>= 1; }
    W(mp, 1200, 30);
}

// ---- transcribed Scottie line (ref: Main.cpp:6173 TMmsstv::LineSCT) ----
// Channel order G,B,(sync),R with 1.5 ms 1500 Hz separators; flag bits in the
// upper nibble select the per-channel TX gain in CSSTVMOD::Do (VariOut path).
static void lineSCT(CSSTVMOD* mp, double tw) {
    uint32_t col[320];
    W(mp, 1500 + 0x2000, 1.5);
    tw /= 320.0;
    for (int x = 0; x < 320; x++) {                 // G
        col[x] = testPixel(x, mp->m_wLine);
        int g = (col[x] >> 8) & 0xFF;
        W(mp, ColorToFreq(g) + 0x2000, tw);
    }
    W(mp, 1500 + 0x3000, 1.5);
    for (int x = 0; x < 320; x++) {                 // B
        int b = (col[x] >> 16) & 0xFF;
        W(mp, ColorToFreq(b) + 0x3000, tw);
    }
    W(mp, 1200, 9);
    W(mp, 1500 + 0x1000, 1.5);
    for (int x = 0; x < 320; x++) {                 // R
        int r = col[x] & 0xFF;
        W(mp, ColorToFreq(r) + 0x1000, tw);
    }
}

static uint64_t fnv1a(const std::vector<short>& v) {
    uint64_t h = 1469598103934665603ULL;
    for (short s : v) { uint16_t u = (uint16_t)s; h ^= (u & 0xFF); h *= 1099511628211ULL; h ^= (u >> 8); h *= 1099511628211ULL; }
    return h;
}

// FNV-1a over the FULL symbol sequence: each Sym as freq(int32 LE) ++ ms(f64 LE bits).
// The Rust modulator hashes its symbol stream identically for a bit-exact TX gate.
static void fnv_byte(uint64_t& h, uint8_t b) { h ^= b; h *= 1099511628211ULL; }
static uint64_t symbol_digest(const std::vector<Sym>& s) {
    uint64_t h = 1469598103934665603ULL;
    for (const Sym& x : s) {
        int32_t f = x.fq; uint8_t fb[4]; std::memcpy(fb, &f, 4);
        for (int i = 0; i < 4; i++) fnv_byte(h, fb[i]);
        double m = x.ms; uint8_t mb[8]; std::memcpy(mb, &m, 8);
        for (int i = 0; i < 8; i++) fnv_byte(h, mb[i]);
    }
    return h;
}

int main() {
    const int mode = smSCT1;      // Scottie 1
    const int visCode = 0x3c;     // ref: Main.cpp:6997
    const double tw = 138.24;     // ref: Main.cpp:6620 LineSCT(mp, 138.24)
    const int nLines = 4;         // dump a few lines; full image is m_L=256

    sys.m_SampFreq = 11025.0; sys.m_TxSampOff = 0.0; sys.m_TestDem = 0;
    SSTVSET.SetMode(mode);
    SSTVSET.SetTxMode(mode);

    CSSTVMOD mod;
    mod.OpenTXBuf(10);
    mod.InitTXBuf();

    emitVIS(&mod, visCode);
    for (int ln = 0; ln < nLines; ln++) { lineSCT(&mod, tw); mod.m_wLine++; }
    if (mode == smSCT1) W(&mod, 1200, 9.0);   // ref: Main.cpp:7124 Scottie leading sync

    // Drain the linked tone renderer.
    std::vector<short> pcm;
    while (mod.GetBufCnt() > 0) {
        double d = mod.Do();
        int v = (int)(d < 0 ? d - 0.5 : d + 0.5);
        if (v > 32767) v = 32767; if (v < -32768) v = -32768;
        pcm.push_back((short)v);
    }

    short mn = 32767, mx = -32768;
    for (short s : pcm) { if (s < mn) mn = s; if (s > mx) mx = s; }

    // ---- JSON out ----
    printf("{\n");
    printf("  \"_meta\": {\n");
    printf("    \"upstream\": \"n5ac/mmsstv @ 8060b5f\",\n");
    printf("    \"mode\": \"Scottie 1 (smSCT1)\", \"vis\": \"0x3c\", \"samp_freq\": 11025,\n");
    printf("    \"linked_unmodified\": [\"sstv.cpp (CSSTVSET,CSSTVMOD)\", \"fir.cpp\"],\n");
    printf("    \"transcribed\": [\"Main.cpp:6940-7107 VIS\", \"Main.cpp:6173 LineSCT\", \"ComLib.cpp:3491 ColorToFreq\"],\n");
    printf("    \"driver\": \"scratch/refvectors/sstv_tx_dump.cxx via build_sstv_tx.sh\",\n");
    printf("    \"note\": \"symbols (freq,ms) are bit-exact; pcm is FP-tolerance (VCO+BPF)\"\n");
    printf("  },\n");
    printf("  \"timing_samples\": { \"m_KS\": %.4f, \"m_OF\": %.4f, \"m_OFP\": %.4f, \"m_SG\": %.4f, \"m_CG\": %.4f, \"m_SB\": %.4f, \"m_CB\": %.4f, \"m_L\": %d, \"m_TxSampFreq\": %.4f },\n",
           SSTVSET.m_KS, SSTVSET.m_OF, SSTVSET.m_OFP, SSTVSET.m_SG, SSTVSET.m_CG, SSTVSET.m_SB, SSTVSET.m_CB, SSTVSET.m_L, SSTVSET.m_TxSampFreq);
    printf("  \"symbol_count\": %zu, \"nlines\": %d, \"symbol_fnv1a\": \"0x%016llx\",\n",
           g_syms.size(), nLines, (unsigned long long)symbol_digest(g_syms));
    printf("  \"vis_symbols\": [");
    for (size_t i = 0; i < 22 && i < g_syms.size(); i++) printf("%s[%d,%.4f]", i?",":"", g_syms[i].fq, g_syms[i].ms);
    printf("],\n");
    printf("  \"pcm\": { \"count\": %zu, \"min\": %d, \"max\": %d, \"fnv1a\": \"0x%016llx\",\n",
           pcm.size(), mn, mx, (unsigned long long)fnv1a(pcm));
    printf("    \"first16\": [");
    for (size_t i = 0; i < 16 && i < pcm.size(); i++) printf("%s%d", i?",":"", pcm[i]);
    printf("] }\n}\n");
    return 0;
}
