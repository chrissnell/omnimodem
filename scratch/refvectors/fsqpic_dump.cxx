// FSQ in-band picture sub-protocol golden-vector extractor.
//
// upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
// source:   src/fsq/fsq-pic.cxx (cb_fsqpicTransmit header tokens :369-378)
//           src/fsq/fsq.cxx     (parse_pcnt RX size/mode table :876-902;
//           sendpic TX pixel/plane :1419-1465; recvpic RX byte :1206;
//           RGB[]={2,1,0} plane map :1210; phidiff = 2*pi*frequency/sr :188)
//
// fsq-pic.cxx / fsq.cxx cannot link standalone (FLTK + modem runtime), so the
// pure header/parse/plane/quantiser functions are transcribed here VERBATIM with
// cites — same doctrine as feldhell_dump / mfskpic_dump / ifkppic_dump.
//
// FSQ's TX (dev = -200 + px*1.5) and RX (byte = dev/1.5 + 128) affines are NOT
// exact inverses: the RX down-converts at `frequency` (phidiff, :188), so a
// clean loopback lands ~6 counts low. That asymmetry is fldigi's real on-wire
// behaviour; the quantiser vectors below pin it bit-exactly. FSQ grey luma
// (0.3R+0.6G+0.1B) is continuous on the wire (:1424-1426) → tolerance, not a
// bit-exact vector. Header tokens, RX parse table, colour plane order (B->G->R),
// and the integer quantiser ARE bit-exact.
//
// emits: JSON-lines consumed by crates/dsp/tests/vectors/fsqpic.json

#include <cstdio>

// ---- CLAMP as fldigi defines it (misc.h): clamp then the caller casts to int.
static double CLAMP(double x, double lo, double hi) {
    return x < lo ? lo : (x > hi ? hi : x);
}

// ---- TX header token per size selector. ref: fsq-pic.cxx:369-378 (image_txt
// append "% X"). selfsqpicSize index -> mode char.
static const char *tx_token(int sel) {
    switch (sel) {
        case 0: return "% S"; // 160x120 clr
        case 1: return "% L"; // 320x240 clr
        case 2: return "% F"; // 640x480 gry
        case 3: return "% V"; // 640x480 clr
        case 4: return "% P"; // 240x300 clr
        case 5: return "% p"; // 240x300 gry
        case 6: return "% M"; // 120x150 clr
        case 7: return "% m"; // 120x150 gry
    }
    return "";
}

// ---- RX parse table. ref: fsq.cxx:876-902 (parse_pcnt switch on rx_text[2]).
// image_mode 0-7; grey = image_mode in {2,5,7} (recvpic :1212). Returns 1 on a
// recognised char, else 0.
static int parse_pcnt(char c, int &W, int &H, int &image_mode, int &grey) {
    W = H = 0;
    image_mode = -1;
    switch (c) {
        case 'L': image_mode = 0; W = 320; H = 240; break;
        case 'S': image_mode = 1; W = 160; H = 120; break;
        case 'F': image_mode = 2; W = 640; H = 480; break;
        case 'V': image_mode = 3; W = 640; H = 480; break;
        case 'P': image_mode = 4; W = 240; H = 300; break;
        case 'p': image_mode = 5; W = 240; H = 300; break;
        case 'M': image_mode = 6; W = 120; H = 150; break;
        case 'm': image_mode = 7; W = 120; H = 150; break;
        default: return 0;
    }
    grey = (image_mode == 2 || image_mode == 5 || image_mode == 7) ? 1 : 0;
    return 1;
}

// ---- TX colour plane order: B->G->R row-major. ref: fsq.cxx:1444-1450
// (for row, for color = 2..0 descending, for col: TxGetPixel(nbr, color)).
static void tx_color(const unsigned char *in, unsigned char *out, int W, int H) {
    int k = 0;
    for (int row = 0; row < H; row++)
        for (int color = 2; color >= 0; color--)
            for (int col = 0; col < W; col++)
                out[k++] = in[3 * (col + row * W) + color];
}

static void emit_bytes(const unsigned char *b, int n) {
    for (int i = 0; i < n; i++) printf("%s%d", i ? "," : "", b[i]);
}

int main() {
    printf("{\"_meta\":{\"upstream\":\"w1hkj/fldigi 4.1.23 @ 61b97f413\",");
    printf("\"files\":[\"src/fsq/fsq-pic.cxx:369-378\",\"src/fsq/fsq.cxx:876-902 (parse_pcnt),1206,1210,1444-1450\"],");
    printf("\"driver\":\"scratch/refvectors/build_fsqpic.sh\",");
    printf("\"note\":\"bit-exact: TX header tokens, RX parse table, B->G->R plane order, integer quantiser (TX/RX affines are NOT inverse - fldigi lands ~6 counts low). Grey luma continuous (tolerance).\"}}\n");

    // TX header tokens per size selector.
    const int wh[8][2] = {{160,120},{320,240},{640,480},{640,480},{240,300},{240,300},{120,150},{120,150}};
    const int grey_sel[8] = {0,0,1,0,0,1,0,1};
    for (int sel = 0; sel < 8; sel++) {
        printf("{\"kind\":\"header\",\"sel\":%d,\"w\":%d,\"h\":%d,\"grey\":%s,\"s\":\"%s\"}\n",
               sel, wh[sel][0], wh[sel][1], grey_sel[sel] ? "true" : "false", tx_token(sel));
    }

    // RX parse table for every image char + one unknown. color = !grey.
    const char *chars = "LSFVPpMmZ";
    for (const char *c = chars; *c; c++) {
        int W, H, image_mode, grey;
        int ok = parse_pcnt(*c, W, H, image_mode, grey);
        printf("{\"kind\":\"parse\",\"c\":\"%c\",\"ok\":%s,\"w\":%d,\"h\":%d,\"mode\":%d,\"color\":%s}\n",
               *c, ok ? "true" : "false", W, H, image_mode, (ok && !grey) ? "true" : "false");
    }

    // Quantiser: TX deviation (Hz, from carrier) and RX byte for a clean loopback
    // (measured dev == TX dev). Pins the asymmetric FSQ affines.
    const int pxs[] = {0, 1, 64, 127, 128, 192, 255};
    for (int i = 0; i < 7; i++) {
        int px = pxs[i];
        double dev = -200.0 + px * 1.5;              // fsq.cxx:1432
        int byte = (int)CLAMP(dev / 1.5 + 128, 0.0, 255.0); // fsq.cxx:1206
        printf("{\"kind\":\"quant\",\"px\":%d,\"dev\":%.6f,\"byte\":%d}\n", px, dev, byte);
    }

    // Colour plane raster (B->G->R) for a 3x2 test image.
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
