// fsq_varicode_dump.cxx — fldigi FSQ (FSQCALL) Varicode + IFK + CRC8 extractor.
//
// Links the *unmodified* fldigi FSQ varicode tables
// (src/fsq/fsq_varicode.cxx: fsq_varicode[256][2] / wsq_varidecode[]) and emits,
// for the whole alphabet and for a set of fixed frames:
//   1. the per-character varicode symbol sequence fsq::send_char feeds
//      fsq::send_symbol: sym1 = fsq_varicode[ch][0], and sym2 = fsq_varicode
//      [ch][1] only when sym2 > 28;
//   2. the IFK tone-index sequence send_symbol produces
//      (tone = (prevtone + sym + 1) % 33, prevtone := tone, start 0);
//   3. the streaming decode: fsq::process_symbol's prev_nibble state machine
//      over the symbol stream, and the ASCII it recovers;
//   4. the CRC8 callsign checksum fsq::tx_process appends to the directed
//      header (mycall ":" crc), transcribed verbatim from include/crc8.h.
//
// The varicode tables + the CRC8 polynomial are the bit-exact reference; the
// short send_char / send_symbol / send_idle / process_symbol framing glue is
// transcribed here verbatim with cites (that glue lives in the fsq modem class
// which cannot link standalone: FLTK / modem runtime).
//
// Provenance:
//   upstream: fldigi 4.2.x, checked out at ../fldigi
//   source:   src/fsq/fsq_varicode.cxx (fsq_varicode / wsq_varidecode)
//             src/fsq/fsq.cxx:1352-1382 (send_symbol/send_idle/send_char),
//                             1017-1110 (process_symbol streaming decode),
//                             1482-1521 (tx_process BOT/EOT framing)
//             include/crc8.h (CRC8, poly 0x07, init 0x00)
//   build:    scratch/refvectors/build_fsq_varicode.sh
//
// Output: JSON lines to stdout.

#include <cstdio>
#include <cstring>
#include <string>

// ref: src/fsq/fsq_varicode.cxx (tables, unmodified).
extern int fsq_varicode[][2];
extern int wsq_varidecode[];

// ref: include/crc8.h — CRC-8/CCITT (poly 0x07), init 0x00, no reflection,
// returned as 2-char lowercase hex.
struct CRC8 {
    unsigned char table[256];
    CRC8() {
        for (int i = 0; i < 256; i++) {
            unsigned char crc = i;
            for (int j = 0; j < 8; j++)
                crc = (crc << 1) ^ ((crc & 0x80) ? 0x07 : 0);
            table[i] = crc & 0xFF;
        }
    }
    std::string sval(const std::string &s) {
        unsigned char val = 0x00;
        for (size_t i = 0; i < s.length(); i++)
            val = table[val ^ (unsigned char)s[i]];
        char ss[3];
        snprintf(ss, sizeof(ss), "%02x", val);
        return ss;
    }
} crc;

// ref: fsq.cxx:1367-1382 send_char / 1359-1363 send_idle — symbols emitted.
static void char_syms(int ch, std::string &syms) {
    if (!ch) { // send_idle
        syms.push_back(28);
        syms.push_back(30);
        return;
    }
    int sym1 = fsq_varicode[ch][0];
    int sym2 = fsq_varicode[ch][1];
    syms.push_back((char)sym1);
    if (sym2 > 28) syms.push_back((char)sym2);
}

// ref: fsq.cxx:1352-1356 send_symbol — IFK tone advance, 33 tones, +1.
static int ifk_tone(int *prevtone, int sym) {
    int tone = (*prevtone + sym + 1) % 33;
    *prevtone = tone;
    return tone;
}

// ref: fsq.cxx:1017-1110 process_symbol — streaming prev_nibble decode.
static int prev_nibble = 0;
static int decode_sym(int curr_nibble) {
    int curr_ch = -1;
    if (prev_nibble < 29 && curr_nibble < 29)
        curr_ch = wsq_varidecode[prev_nibble];
    else if (prev_nibble < 29 && curr_nibble > 28 && curr_nibble < 32)
        curr_ch = wsq_varidecode[prev_nibble * 32 + curr_nibble];
    prev_nibble = curr_nibble;
    return curr_ch;
}

static void esc(const std::string &s) {
    for (char c : s) {
        if (c == '"' || c == '\\') printf("\\%c", c);
        else if (c == '\n') printf("\\n");
        else if (c == '\b') printf("\\b");
        else printf("%c", (unsigned char)c);
    }
}

// Emit syms + tones + streaming decode for an arbitrary on-air string.
static void dump_frame(const char *label, const std::string &onair) {
    std::string syms;
    for (char c : onair) char_syms((unsigned char)c, syms);

    int prevtone = 0;
    printf("{\"frame\":\"%s\",\"onair\":\"", label);
    esc(onair);
    printf("\",\"syms\":[");
    for (size_t i = 0; i < syms.length(); i++)
        printf(i ? ",%d" : "%d", (unsigned char)syms[i]);
    printf("],\"tones\":[");
    for (size_t i = 0; i < syms.length(); i++)
        printf(i ? ",%d" : "%d", ifk_tone(&prevtone, (unsigned char)syms[i]));
    printf("],\"decode\":\"");
    prev_nibble = 0;
    std::string dec;
    for (size_t i = 0; i < syms.length(); i++) {
        int c = decode_sym((unsigned char)syms[i]);
        if (c > 0) dec.push_back((char)c);
    }
    // flush trailing single-symbol char with two idle-equivalent symbols.
    for (int i = 0; i < 2; i++) {
        int c = decode_sym(28);
        if (c > 0) dec.push_back((char)c);
    }
    esc(dec);
    printf("\"}\n");
}

int main() {
    // Full alphabet: raw table sym1/sym2 for every character 0..255.
    printf("{\"table\":\"fsq_varicode\",\"rows\":[");
    for (int c = 0; c < 256; c++)
        printf("%s[%d,%d]", c ? "," : "", fsq_varicode[c][0], fsq_varicode[c][1]);
    printf("]}\n");

    // wsq_varidecode table (29*32 entries: prev in 0..28, curr in 0..32).
    printf("{\"table\":\"wsq_varidecode\",\"rows\":[");
    for (int i = 0; i < 29 * 32; i++)
        printf("%s%d", i ? "," : "", wsq_varidecode[i]);
    printf("]}\n");

    // CRC8 callsign checksums (directed-header key material).
    const char *calls[] = {"w1hkj", "k1abc", "n0call", "allcall", "cqcqcq", "ve3xyz/p"};
    printf("{\"table\":\"crc8\",\"rows\":[");
    for (int i = 0; i < 6; i++)
        printf("%s{\"s\":\"%s\",\"crc\":\"%s\"}", i ? "," : "", calls[i], crc.sval(calls[i]).c_str());
    printf("]}\n");

    // Plain text frame (no directed header).
    dump_frame("text", "the quick brown fox de w1hkj");

    // A full directed BOT/EOT frame, exactly as tx_process assembles it in
    // directed mode: " " + FSQBOL(" \n") + mycall + ":" + crc(mycall) + body +
    // FSQEOT("  \b  ").  mycall = w1hkj, body = "k1abc test".
    {
        std::string mycall = "w1hkj";
        std::string frame = std::string(" ") + " \n" + mycall + ":" + crc.sval(mycall);
        frame += "k1abc test";
        frame += "  \b  ";
        dump_frame("directed", frame);
    }
    return 0;
}
