// MFSK in-band picture sub-protocol golden-vector extractor.
//
// upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
// source:   src/mfsk/mfsk-pic.cxx (pic_TxSendColor plane reorder :196-202;
//           pic_TxSendGrey luma :239; picheader builders :205-207, 246-248)
//           src/mfsk/mfsk.cxx    (check_picture_header RX parser :366-422)
//           src/include/mfsk.h   (PICHEADER 64)
//
// mfsk-pic.cxx / mfsk.cxx cannot link standalone (FLTK + the modem runtime), so
// the three pure integer/string functions are transcribed here VERBATIM with
// cites — same doctrine as scratch/refvectors/feldhell_dump.cxx. Compiled +
// run by an independent C++ toolchain, its output is the reference the Rust
// KATs in crates/dsp/src/modes/picture.rs assert against byte-for-byte.
//
// emits: JSON-lines consumed by crates/dsp/tests/vectors/mfskpic.json

#include <cstdio>
#include <cstring>
#include <cctype>

#define PICHEADER 64  // ref: mfsk.h:50

// ---- TX header builders. ref: mfsk-pic.cxx:205-207 (color), :246-248 (grey).
static void tx_header(char *picheader, int W, int H, bool color, int TXspp) {
    if (color) {
        if (TXspp == 8)
            snprintf(picheader, PICHEADER, "\nSending Pic:%dx%dC;", W, H);
        else
            snprintf(picheader, PICHEADER, "\nSending Pic:%dx%dCp%d;", W, H, TXspp);
    } else {
        if (TXspp == 8)
            snprintf(picheader, PICHEADER, "\nSending Pic:%dx%d;", W, H);
        else
            snprintf(picheader, PICHEADER, "\nSending Pic:%dx%dp%d;", W, H, TXspp);
    }
}

// ---- TX colour plane reorder. ref: mfsk-pic.cxx:196-202. inbuf is interleaved
// RGB (W*H*3); outbuf is per-row planar R…G…B (W*H*3).
static void tx_color(const unsigned char *inbuf, unsigned char *outbuf, int W, int H) {
    int iy, ix, rgb, rowstart;
    for (iy = 0; iy < H; iy++) {
        rowstart = iy * W * 3;
        for (rgb = 0; rgb < 3; rgb++)
            for (ix = 0; ix < W; ix++)
                outbuf[rowstart + rgb * W + ix] = inbuf[rowstart + rgb + ix * 3];
    }
}

// ---- TX grey luma. ref: mfsk-pic.cxx:239. integer reduction, one byte/pixel.
static void tx_grey(const unsigned char *inbuf, unsigned char *outbuf, int W, int H) {
    for (int i = 0; i < W * H; i++)
        outbuf[i] = (31 * inbuf[i * 3] + 61 * inbuf[i * 3 + 1] + 8 * inbuf[i * 3 + 2]) / 100;
}

// ---- RX header parser, VERBATIM. ref: mfsk.cxx:366-422. Operates on a rolling
// PICHEADER buffer fed one char at a time.
static char picheader[PICHEADER];
static int picW, picH, RXspp;
static bool pic_color;

static bool check_picture_header(char c) {
    char *p;
    if (c >= ' ' && c <= 'z') {
        memmove(picheader, picheader + 1, PICHEADER - 1);
        picheader[PICHEADER - 2] = c;
    }
    picW = 0;
    picH = 0;
    pic_color = false;
    p = strstr(picheader, "Pic:");
    if (p == NULL) return false;
    p += 4;
    if (*p == 0) return false;
    while (*p && isdigit(*p)) picW = (picW * 10) + (*p++ - '0');
    if (*p++ != 'x') return false;
    while (*p && isdigit(*p)) picH = (picH * 10) + (*p++ - '0');
    if (*p == 'C') { pic_color = true; p++; }
    if (*p == ';') {
        if (picW == 0 || picH == 0 || picW > 4095 || picH > 4095) return false;
        RXspp = 8;
        return true;
    }
    if (*p == 'p') p++; else return false;
    if (!*p) return false;
    RXspp = 8;
    if (*p == '4') RXspp = 4;
    if (*p == '2') RXspp = 2;
    p++;
    if (!*p) return false;
    if (*p != ';') return false;
    if (picW == 0 || picH == 0 || picW > 4095 || picH > 4095) return false;
    return true;
}

static void emit_bytes(const unsigned char *b, int n) {
    for (int i = 0; i < n; i++) printf("%s%d", i ? "," : "", b[i]);
}

// Feed a header string char-by-char through the rolling buffer, return the
// parse result at the char that completes it (mirrors the RX data path).
static bool run_parser(const char *s, int &w, int &h, bool &col, int &spp) {
    // fldigi's picheader is a rolling window continuously fed by the RX text
    // stream — printable chars, no embedded nulls before the content. Seed it
    // with spaces (not zeros) so strstr scans to the header, and keep index
    // PICHEADER-1 as the null terminator.
    memset(picheader, ' ', PICHEADER);
    picheader[PICHEADER - 1] = 0;
    bool ok = false;
    for (const char *c = s; *c; c++) {
        if (check_picture_header(*c)) { ok = true; break; }
    }
    w = picW; h = picH; col = pic_color; spp = RXspp;
    return ok;
}

int main() {
    printf("{\"_meta\":{\"upstream\":\"w1hkj/fldigi 4.1.23 @ 61b97f413\",");
    printf("\"files\":[\"src/mfsk/mfsk-pic.cxx:196-202,239,205-207,246-248\",");
    printf("\"src/mfsk/mfsk.cxx:366-422 (check_picture_header)\"],");
    printf("\"driver\":\"scratch/refvectors/build_mfskpic.sh\",");
    printf("\"note\":\"bit-exact: TX header strings, colour plane reorder, integer luma, RX header parse.\"}}\n");

    // TX headers for a representative size across colour/grey x speed.
    const int W = 3, H = 2;
    char hdr[PICHEADER];
    const bool cols[2] = {true, false};
    const int spps[3] = {8, 4, 2};
    for (int ci = 0; ci < 2; ci++) {
        for (int si = 0; si < 3; si++) {
            tx_header(hdr, W, H, cols[ci], spps[si]);
            // JSON-escape the leading \n.
            printf("{\"kind\":\"header\",\"w\":%d,\"h\":%d,\"color\":%s,\"txspp\":%d,\"s\":\"",
                   W, H, cols[ci] ? "true" : "false", spps[si]);
            for (const char *c = hdr; *c; c++) {
                if (*c == '\n') printf("\\n"); else putchar(*c);
            }
            printf("\"}\n");
        }
    }
    // Also a realistic size header (colour, spp8) to pin multi-digit formatting.
    tx_header(hdr, 320, 240, true, 8);
    printf("{\"kind\":\"header\",\"w\":320,\"h\":240,\"color\":true,\"txspp\":8,\"s\":\"");
    for (const char *c = hdr; *c; c++) { if (*c == '\n') printf("\\n"); else putchar(*c); }
    printf("\"}\n");

    // Test image: 3x2 interleaved RGB, distinct values so ordering is unambiguous.
    unsigned char in[3 * 2 * 3] = {
        10, 11, 12,  20, 21, 22,  30, 31, 32,   // row 0
        40, 41, 42,  50, 51, 52,  60, 61, 62,   // row 1
    };
    unsigned char cout[3 * 2 * 3];
    unsigned char gout[3 * 2];
    tx_color(in, cout, W, H);
    tx_grey(in, gout, W, H);
    printf("{\"kind\":\"color_raster\",\"w\":%d,\"h\":%d,\"in\":[", W, H);
    emit_bytes(in, W * H * 3);
    printf("],\"out\":[");
    emit_bytes(cout, W * H * 3);
    printf("]}\n");
    printf("{\"kind\":\"grey_raster\",\"w\":%d,\"h\":%d,\"in\":[", W, H);
    emit_bytes(in, W * H * 3);
    printf("],\"out\":[");
    emit_bytes(gout, W * H);
    printf("]}\n");

    // RX parse round-trip for each header form + a couple of malformed strings.
    const char *cases[] = {
        "\nSending Pic:3x2C;", "\nSending Pic:3x2Cp4;", "\nSending Pic:3x2;",
        "\nSending Pic:3x2p2;", "\nSending Pic:320x240C;",
        "\nSending Pic:0x2;", "\nSending Pic:3x2Cp9;", "junk no header",
    };
    for (unsigned k = 0; k < sizeof(cases) / sizeof(*cases); k++) {
        int w, h, spp; bool col;
        bool ok = run_parser(cases[k], w, h, col, spp);
        printf("{\"kind\":\"parse\",\"s\":\"");
        for (const char *c = cases[k]; *c; c++) { if (*c == '\n') printf("\\n"); else putchar(*c); }
        printf("\",\"ok\":%s,\"w\":%d,\"h\":%d,\"color\":%s,\"rxspp\":%d}\n",
               ok ? "true" : "false", w, h, col ? "true" : "false", spp);
    }
    return 0;
}
