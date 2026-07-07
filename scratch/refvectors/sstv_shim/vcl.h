// F0 SSTV golden-vector harness — fake <vcl.h> for GRA-289.
// Lets the *unmodified* MMSSTV DSP core (sstv.cpp, fir.cpp) compile on Linux g++
// by supplying the Borland/VCL types + macros those files reference. No VCL, no GUI.
// This is scratch tooling; it is NEVER shipped and does not affect the Rust port.
#ifndef F0_VCL_SHIM_H
#define F0_VCL_SHIM_H

#include <cstdint>
#include <cstring>
#include <cstdlib>
#include <cmath>
#include <cstdio>
#include <cctype>

// --- Borland fundamental types ---
typedef uint8_t   BYTE;
typedef uint16_t  WORD;
typedef uint16_t  USHORT;
typedef uint32_t  DWORD;
typedef int       BOOL;
typedef unsigned  UINT;
typedef const char* LPCSTR;
typedef char*     LPSTR;

#ifndef TRUE
#define TRUE  1
#endif
#ifndef FALSE
#define FALSE 0
#endif

// Borland calling-convention / keyword no-ops (functional behavior is identical).
#define __fastcall
#define __closure

// TColor is a packed 0x00BBGGRR int in the VCL.
typedef int TColor;

// The DSP never draws. fir.cpp still *defines* unused DrawGraph* functions that
// touch the VCL Canvas, so provide a no-op GUI surface just rich enough to compile
// them. None of this is ever executed.
enum { psSolid, psDot };
enum { bsSolid, bsClear };
enum { clBlack = 0, clWhite = 0xFFFFFF, clRed = 0xFF, clBlue = 0xFF0000, clGray = 0x808080 };
enum { TRANSPARENT = 1 };
struct TRect { int Left, Top, Right, Bottom; TRect(){} TRect(int l,int t,int r,int b):Left(l),Top(t),Right(r),Bottom(b){} };
inline TRect Rect(int l,int t,int r,int b){ return TRect(l,t,r,b); }
struct TPen   { TColor Color; int Width; int Style; };
struct TBrush { TColor Color; int Style; };
struct TFont  { int Size; TColor Color; int Style; };
struct TCanvas {
    TPen   _pen;  TPen*   Pen;
    TBrush _brush;TBrush* Brush;
    TFont  _font; TFont*  Font;
    int Handle;
    TCanvas():Pen(&_pen),Brush(&_brush),Font(&_font),Handle(0){}
    void MoveTo(int,int){}
    void LineTo(int,int){}
    void FillRect(const TRect&){}
    void TextOut(int,int,const char*){}
    int  TextWidth(const char*){ return 0; }
    int  TextHeight(const char*){ return 0; }
};
inline void SetBkMode(int,int){}
namespace Graphics {
    struct TBitmap { TCanvas _c; TCanvas* Canvas; int Width, Height; TBitmap():Canvas(&_c),Width(0),Height(0){} };
}

// The DSP globals fir.cpp/sstv.cpp read; defined once in the harness driver.
extern double SampFreq;   // active sample rate
extern double SampBase;   // reference rate for tap scaling (11025)

// Win32 page-locking used by CVCO's sine table — irrelevant off Windows.
inline BOOL VirtualLock(void*, unsigned long){ return TRUE; }
inline BOOL VirtualUnlock(void*, unsigned long){ return TRUE; }

// CLOCKMAX (ref: ComLib.h:52) — max supported sample rate, for table sizing.
#ifndef CLOCKMAX
#define CLOCKMAX 48500
#endif

// Borland min/max intrinsics used by some numeric code.
#ifndef __min
#define __min(a,b) ((a) < (b) ? (a) : (b))
#endif
#ifndef __max
#define __max(a,b) ((a) > (b) ? (a) : (b))
#endif

#endif
