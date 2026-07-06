// F0 SSTV golden-vector harness — RX dump driver (GRA-289, Phase 17).
//
// Renders one mode's TX audio with the *unmodified* CSSTVMOD, then feeds it back
// through the *unmodified* CSSTVDEM demodulator + VIS detector and reports what the
// reference decodes: the detected VIS -> mode, and the sync-state progression.
// This is the "reference decodes our TX" substitute (MMSSTV has no Linux CLI decoder).
//
// Linked unmodified: sstv.cpp (CSSTVSET, CSSTVMOD, CSSTVDEM) + fir.cpp.
// Transcribed (as in sstv_tx_dump.cxx): the VIS header + LineSCT layout + ColorToFreq.
//
// Build: scratch/refvectors/build_sstv_rx.sh (from the omnimodem repo root).

#include <vcl.h>
#include "sstv.h"
#include <vector>
#include <cstdint>
#include <cstdio>

double   SampFreq = 11025.0;
double   SampBase = 11025.0;
SYSSET   sys;
extern CSSTVSET SSTVSET;   // defined in sstv.cpp

static int ColorToFreq(int d) { d = d * (2300 - 1500) / 256; return d + 1500; }

static const uint32_t kBars[8] = {
    0x000000, 0x0000FF, 0x00FF00, 0x00FFFF, 0xFF0000, 0xFF00FF, 0xFFFF00, 0xFFFFFF,
};
static uint32_t testPixel(int x, int) { return kBars[(x * 8) / 320]; }

static void emitVIS(CSSTVMOD* mp, int vis) {   // ref: Main.cpp:6940-7107
    mp->Write(1900, 300); mp->Write(1200, 10); mp->Write(1900, 300); mp->Write(1200, 30);
    int d = vis;
    for (int i = 0; i < 8; i++) { mp->Write(short(d & 1 ? 1100 : 1300), 30); d >>= 1; }
    mp->Write(1200, 30);
}
static void lineSCT(CSSTVMOD* mp, double tw) { // ref: Main.cpp:6173
    uint32_t col[320];
    mp->Write(short(1500 + 0x2000), 1.5); tw /= 320.0;
    for (int x = 0; x < 320; x++) { col[x] = testPixel(x, mp->m_wLine); mp->Write(short(ColorToFreq((col[x] >> 8) & 0xFF) + 0x2000), tw); }
    mp->Write(short(1500 + 0x3000), 1.5);
    for (int x = 0; x < 320; x++) mp->Write(short(ColorToFreq((col[x] >> 16) & 0xFF) + 0x3000), tw);
    mp->Write(1200, 9);
    mp->Write(short(1500 + 0x1000), 1.5);
    for (int x = 0; x < 320; x++) mp->Write(short(ColorToFreq(col[x] & 0xFF) + 0x1000), tw);
}

int main() {
    const int mode = smSCT1, vis = 0x3c;
    const double tw = 138.24;
    sys.m_SampFreq = 11025.0; sys.m_TxSampOff = 0.0; sys.m_TestDem = 0; sys.m_UseRxBuff = 0;

    // ---- render TX audio (unmodified CSSTVMOD) ----
    SSTVSET.SetMode(mode); SSTVSET.SetTxMode(mode);
    CSSTVMOD mod; mod.OpenTXBuf(10); mod.InitTXBuf();
    emitVIS(&mod, vis);
    for (int ln = 0; ln < 20; ln++) { lineSCT(&mod, tw); mod.m_wLine++; }
    mod.Write(1200, 9.0);
    std::vector<short> pcm;
    while (mod.GetBufCnt() > 0) { double d = mod.Do(); int v = (int)(d < 0 ? d - 0.5 : d + 0.5); if (v > 32767) v = 32767; if (v < -32768) v = -32768; pcm.push_back((short)v); }

    // ---- feed the unmodified demodulator ----
    SSTVSET.SetMode(smR36);           // deliberately WRONG initial mode; VIS detect must override to smSCT1
    CSSTVDEM dem;

    int startMode = SSTVSET.m_Mode;
    int visLockMode = -1; long visLockSample = -1;  // mode when VIS first validates (SyncMode 3 or picture RX 256)
    int prevSyncMode = dem.m_SyncMode, prevMode = SSTVSET.m_Mode;
    int detectedMode = -1; long detectSample = -1;
    int transitions = 0;
    printf("{\n  \"_meta\": { \"upstream\": \"n5ac/mmsstv @ 8060b5f\", \"test\": \"TX Scottie1 -> unmodified CSSTVDEM\",\n");
    printf("    \"linked_unmodified\": [\"CSSTVMOD\", \"CSSTVDEM\", \"CSSTVSET\", \"fir.cpp\"] },\n");
    printf("  \"pcm_samples\": %zu,\n", pcm.size());
    printf("  \"sync_trace\": [");
    for (size_t i = 0; i < pcm.size(); i++) {
        dem.Do((double)pcm[i]);
        // The picture-RX state (>=256) is only entered once VIS has validated the mode.
        if (visLockMode < 0 && dem.m_SyncMode >= 256) { visLockMode = SSTVSET.m_Mode; visLockSample = (long)i; }
        if (dem.m_SyncMode != prevSyncMode || SSTVSET.m_Mode != prevMode) {
            printf("%s\n    {\"sample\":%zu,\"syncMode\":%d,\"setMode\":%d}", transitions?",":"", i, dem.m_SyncMode, SSTVSET.m_Mode);
            if (SSTVSET.m_Mode != prevMode && SSTVSET.m_Mode != startMode && detectedMode < 0) { detectedMode = SSTVSET.m_Mode; detectSample = (long)i; }
            prevSyncMode = dem.m_SyncMode; prevMode = SSTVSET.m_Mode; transitions++;
            if (transitions > 40) break;
        }
    }
    printf("\n  ],\n");
    printf("  \"start_mode\": %d, \"detected_mode\": %d, \"detected_at_sample\": %ld,\n", startMode, detectedMode, detectSample);
    printf("  \"picture_rx_mode\": %d, \"picture_rx_at_sample\": %ld, \"expected_smSCT1\": %d,\n", visLockMode, visLockSample, (int)smSCT1);
    printf("  \"vis_ok\": %s\n}\n", (visLockMode == (int)smSCT1) ? "true" : "false");
    return 0;
}
