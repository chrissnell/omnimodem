// F0 SSTV harness — minimal shim for MMSSTV's ComLib.h.
// The real ComLib.h drags in the whole GUI (ComCtrls/Grids/main.h); the DSP core
// (sstv.cpp, fir.cpp) only needs a handful of types + the global `sys` config.
// Field types are copied verbatim from the upstream ComLib.h (commit 8060b5f).
#ifndef F0_COMLIB_SHIM_H
#define F0_COMLIB_SHIM_H

// Suppress the real ComLib.h: fir.h does a quote-include that resolves next to
// itself (the upstream tree), so we pre-define its include guard here and
// force-include this shim first (-include). The upstream ComLib.h then self-skips.
#define ComLibH

#include <vcl.h>
#include <cmath>

// COLD: packed colour union (ref: ComLib.h:120-131). Used by the transcribed
// TX line generators in the harness driver, not by the linked DSP.
#pragma pack(push, 1)
typedef union {
    struct { BYTE r, g, b, d; } b;
    TColor c;
    DWORD  d;
} COLD;
#pragma pack(pop)

// SYSSET: only the fields the linked DSP (sstv.cpp) reads. Types verbatim from
// the upstream ComLib.h SYSSET declaration.
struct SYSSET {
    double m_SampFreq;
    double m_TxSampOff;
    int    m_Repeater;
    int    m_TestDem;
    int    m_UseRxBuff;
    int    m_bCQ100;
    int    m_CWIDFreq;
    int    m_CWIDSpeed;
    int    m_RepSenseLvl;
    int    m_RepTimeA;
    int    m_RepTimeB;
    int    m_RepTimeC;
    int    m_RepTimeD;
};

extern SYSSET sys;

// Colour clamp (ref: ComLib.cpp Limit256). Provided inline so any incidental
// reference resolves; the DSP path does not call it.
inline int Limit256(int d) { return d < 0 ? 0 : (d > 255 ? 255 : d); }

// Small ComLib string/math helpers used only in the out-of-scope FSK-ID /
// repeater code paths of sstv.cpp — present so the TU links.
#ifndef ABS
#define ABS(x) ((x) < 0 ? -(x) : (x))
#endif
inline char* StrCopy(char* d, const char* s, int n){ int i=0; for(; s && s[i] && i<n; ++i) d[i]=s[i]; d[i]=0; return d; }
inline const char* SkipSpace(const char* s){ while(*s==' '||*s=='\t') ++s; return s; }
inline void clipsp(char* s){ int n=(int)std::strlen(s); while(n>0 && (s[n-1]==' '||s[n-1]=='\t')) s[--n]=0; }

#endif
