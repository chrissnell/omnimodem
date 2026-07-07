// THOR in-band picture sub-protocol golden-vector extractor.
//
// upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
// source:   src/thor/thor-pic.cxx (cb file-transmit TX header builder
//           :370-439, picmode = "pic% \n" with [4] = mode char)
//           src/thor/thor.cxx    (parse_pic RX size/colour table :404-420;
//           colour plane fetch send_image :1349-1362; RX byte :974)
//
// thor-pic.cxx / thor.cxx cannot link standalone (FLTK + modem runtime), so the
// pure header/parse/plane functions are transcribed here VERBATIM with cites —
// same doctrine as feldhell_dump / mfskpic_dump / ifkppic_dump.
//
// THOR grey luma (0.3R+0.6G+0.1B) is applied as a *continuous double* on the wire
// (thor.cxx:1329-1331), not truncated to a byte, so it is FP/tolerance (covered
// by the loopback), not a bit-exact vector. The header strings, RX parse table,
// and colour plane order ARE bit-exact.
//
// emits: JSON-lines consumed by crates/dsp/tests/vectors/thorpic.json

#include <cstdio>

// ---- TX header char per (size index, grey). ref: thor-pic.cxx:390-439.
static char tx_char(int size, bool grey) {
    switch (size) {
        case 0: return grey ? 't' : 'T'; // 59x74
        case 1: return grey ? 'm' : 'M'; // 120x150
        case 2: return grey ? 'p' : 'P'; // 240x300
        case 3: return grey ? 's' : 'S'; // 160x120
        case 4: return grey ? 'l' : 'L'; // 320x240
        case 5: return grey ? 'F' : 'V'; // 640x480 (grey uses 'F', colour 'V')
    }
    return ' ';
}

// ---- RX parse table. ref: thor.cxx:404-420 (switch on pic_str[4]).
// image_mode: 0 = colour, 1 = grey. Returns 1 on a recognised char, else 0.
static int parse_pic(char c, int &W, int &H, int &image_mode, int &avatar) {
    W = H = 0;
    image_mode = 0; // default colour
    avatar = 0;
    switch (c) {
        case 'A': W = 59;  H = 74;  avatar = 1; break;
        case 'T': W = 59;  H = 74;  break;
        case 't': W = 59;  H = 74;  image_mode = 1; break;
        case 'S': W = 160; H = 120; break;
        case 's': W = 160; H = 120; image_mode = 1; break;
        case 'L': W = 320; H = 240; break;
        case 'l': W = 320; H = 240; image_mode = 1; break;
        case 'V': W = 640; H = 480; break;
        case 'v': W = 640; H = 480; image_mode = 1; break;
        case 'F': W = 640; H = 480; image_mode = 1; break;
        case 'P': W = 240; H = 300; break;
        case 'p': W = 240; H = 300; image_mode = 1; break;
        case 'M': W = 120; H = 150; break;
        case 'm': W = 120; H = 150; image_mode = 1; break;
        default: return 0;
    }
    return 1;
}

// ---- TX colour plane order: R→G→B row-major. ref: thor.cxx:1349-1362
// (for row, for color 0..3, for col: tx_pixel = TxGetPixel(nbr, color)).
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
    printf("\"files\":[\"src/thor/thor-pic.cxx:370-439\",\"src/thor/thor.cxx:404-420 (parse_pic),1349-1362,974\"],");
    printf("\"driver\":\"scratch/refvectors/build_thorpic.sh\",");
    printf("\"note\":\"bit-exact: TX header chars, RX parse table, colour plane order. Grey luma is continuous on the wire (tolerance).\"}}\n");

    // TX headers: "pic%X\n" per size x grey (thor-pic.cxx: picmode="pic% \n", [4]=ch).
    const int wh[6][2] = {{59,74},{120,150},{240,300},{160,120},{320,240},{640,480}};
    for (int sz = 0; sz < 6; sz++) {
        for (int g = 0; g < 2; g++) {
            char ch = tx_char(sz, g == 1);
            printf("{\"kind\":\"header\",\"w\":%d,\"h\":%d,\"grey\":%s,\"s\":\"pic%%%c\\n\"}\n",
                   wh[sz][0], wh[sz][1], g ? "true" : "false", ch);
        }
    }

    // RX parse table for every image char + one unknown. color = (image_mode==0).
    const char *chars = "ATtSsLlVvFPpMmZ";
    for (const char *c = chars; *c; c++) {
        int W, H, image_mode, avatar;
        int ok = parse_pic(*c, W, H, image_mode, avatar);
        printf("{\"kind\":\"parse\",\"c\":\"%c\",\"ok\":%s,\"w\":%d,\"h\":%d,\"color\":%s,\"avatar\":%s}\n",
               *c, ok ? "true" : "false", W, H, image_mode == 0 ? "true" : "false",
               avatar ? "true" : "false");
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
