// IFKP in-band picture sub-protocol golden-vector extractor.
//
// upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
// source:   src/ifkp/ifkp-pic.cxx (cb_ifkppicTransmit TX header char table
//           :461-470; colour plane fetch ifkppic_TxGetPixel :454)
//           src/ifkp/ifkp.cxx    (parse_pic RX size/colour table :377-420)
//
// ifkp-pic.cxx / ifkp.cxx cannot link standalone (FLTK + modem runtime), so the
// pure header/parse/plane functions are transcribed here VERBATIM with cites —
// same doctrine as feldhell_dump / mfskpic_dump.
//
// IFKP grey luma (0.3R+0.6G+0.1B) is applied as a *continuous double* on the wire
// (ifkp.cxx:815-817), not truncated to a byte, so it is FP/tolerance (covered by
// the loopback), not a bit-exact vector. The header strings, RX parse table, and
// colour plane order ARE bit-exact.
//
// emits: JSON-lines consumed by crates/dsp/tests/vectors/ifkppic.json

#include <cstdio>
#include <cstring>
#include <string>

// ---- TX header char per (size index, grey). ref: ifkp-pic.cxx:461-470.
static char tx_char(int size, bool grey) {
    switch (size) {
        case 0: return grey ? 't' : 'T'; // 59x74
        case 1: return grey ? 'm' : 'M'; // 120x150
        case 2: return grey ? 'p' : 'P'; // 240x300
        case 3: return grey ? 's' : 'S'; // 160x120
        case 4: return grey ? 'l' : 'L'; // 320x240
        case 5: return grey ? 'F' : 'V'; // 640x480
    }
    return ' ';
}

// ---- RX parse table. ref: ifkp.cxx:385-405 (parse_pic switch on pic_str[4]).
// Returns 1 on a recognised image char, sets W/H/color/avatar; 0 otherwise.
static int parse_pic(char c, int &W, int &H, int &color, int &avatar) {
    W = H = 0;
    color = 1; // default colour (image_mode 0 → colour); grey sets image_mode 1
    avatar = 0;
    switch (c) {
        case 'A': W = 59;  H = 74;  avatar = 1; break;
        case 'T': W = 59;  H = 74;  break;
        case 't': W = 59;  H = 74;  color = 0; break;
        case 'S': W = 160; H = 120; break;
        case 's': W = 160; H = 120; color = 0; break;
        case 'L': W = 320; H = 240; break;
        case 'l': W = 320; H = 240; color = 0; break;
        case 'V': W = 640; H = 480; break;
        case 'v': W = 640; H = 480; color = 0; break;
        case 'F': W = 640; H = 480; color = 0; break;
        case 'P': W = 240; H = 300; break;
        case 'p': W = 240; H = 300; color = 0; break;
        case 'M': W = 120; H = 150; break;
        case 'm': W = 120; H = 150; color = 0; break;
        default: return 0;
    }
    return 1;
}

// ---- TX colour plane order: R→G→B row-major (ifkp.cxx colour send loop).
static void tx_color(const unsigned char *in, unsigned char *out, int W, int H) {
    for (int row = 0; row < H; row++)
        for (int color = 0; color < 3; color++)
            for (int col = 0; col < W; col++)
                out[(row * 3 + color) * W + col] = in[3 * (col + row * W) + color];
}

static void emit_bytes(const unsigned char *b, int n) {
    for (int i = 0; i < n; i++) printf("%s%d", i ? "," : "", b[i]);
}

int main() {
    printf("{\"_meta\":{\"upstream\":\"w1hkj/fldigi 4.1.23 @ 61b97f413\",");
    printf("\"files\":[\"src/ifkp/ifkp-pic.cxx:461-470,454\",\"src/ifkp/ifkp.cxx:385-420 (parse_pic)\"],");
    printf("\"driver\":\"scratch/refvectors/build_ifkppic.sh\",");
    printf("\"note\":\"bit-exact: TX header chars, RX parse table, colour plane order. Grey luma is continuous on the wire (tolerance).\"}}\n");

    // TX headers: " pic%X" per size x grey.
    const int wh[6][2] = {{59,74},{120,150},{240,300},{160,120},{320,240},{640,480}};
    for (int sz = 0; sz < 6; sz++) {
        for (int g = 0; g < 2; g++) {
            char ch = tx_char(sz, g == 1);
            printf("{\"kind\":\"header\",\"w\":%d,\"h\":%d,\"grey\":%s,\"s\":\" pic%%%c\"}\n",
                   wh[sz][0], wh[sz][1], g ? "true" : "false", ch);
        }
    }

    // RX parse table for every image char + one unknown.
    const char *chars = "ATtSsLlVvFPpMmZ";
    for (const char *c = chars; *c; c++) {
        int W, H, color, avatar;
        int ok = parse_pic(*c, W, H, color, avatar);
        printf("{\"kind\":\"parse\",\"c\":\"%c\",\"ok\":%s,\"w\":%d,\"h\":%d,\"color\":%s,\"avatar\":%s}\n",
               *c, ok ? "true" : "false", W, H, color ? "true" : "false", avatar ? "true" : "false");
    }

    // Colour plane raster (R→G→B) for a 3x2 test image.
    const int W = 3, H = 2;
    unsigned char in[3 * 2 * 3] = {
        10, 11, 12, 20, 21, 22, 30, 31, 32,
        40, 41, 42, 50, 51, 52, 60, 61, 62,
    };
    unsigned char out[3 * 2 * 3];
    tx_color(in, out, W, H);
    printf("{\"kind\":\"color_raster\",\"w\":%d,\"h\":%d,\"in\":[", W, H);
    emit_bytes(in, W * H * 3);
    printf("],\"out\":[");
    emit_bytes(out, W * H * 3);
    printf("]}\n");
    return 0;
}
