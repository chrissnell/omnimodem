// JSC dictionary + golden-vector extractor for the omnimodem JS8 port (Phase W5).
//
// Links the UNMODIFIED js8call tables (jsc_map.cpp, jsc_list.cpp @ a7ff1be) via a
// Qt-free jsc.h. The compress/decompress/lookup/codeword algorithm is transcribed
// verbatim from js8call/jsc.cpp (Qt QString/QList/QVector -> std::string/std::vector),
// because jsc.cpp itself needs Qt (unavailable in this env). The TABLES are the
// reference; the ~140-line algorithm is transcribed identically. Golden vectors
// produced here gate the Rust port (crates/dsp/src/framing/jsc.rs).
//
// Outputs:
//   jsc_dict.bin   - binary dictionary blob for embedding (see format below)
//   jsc_vectors.json - compress/decompress golden vectors for a fixed message set
//
// ref: js8call/jsc.cpp (compress/decompress/codeword/lookup), varicode.cpp
//      (intToBits/bitsToInt), jsc_map.cpp/jsc_list.cpp (tables).

#include "jsc.h"
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>
#include <cmath>

typedef std::vector<bool> Codeword;

// ---- Varicode::intToBits / bitsToInt (ref: varicode.cpp:649-682) ----
static Codeword intToBits(uint64_t value, int expected) {
    Codeword bits;
    while (value) { bits.insert(bits.begin(), (bool)(value & 1)); value >>= 1; }
    while ((int)bits.size() < expected) bits.insert(bits.begin(), false);
    return bits;
}
static uint64_t bitsToInt(const Codeword &v) {
    uint64_t out = 0; for (bool b : v) out = (out << 1) + (int)b; return out;
}

// ---- JSC::lookup (ref: jsc.cpp:196-238) ----
static uint32_t lookup(const char *b, bool *ok) {
    uint32_t index = 0, count = 0; bool found = false;
    for (uint32_t i = 0; i < JSC::prefixSize; i++) {
        if (b[0] != JSC::prefix[i].str[0]) continue;
        if (JSC::prefix[i].size == 1) { if (ok) *ok = true; return JSC::list[JSC::prefix[i].index].index; }
        index = JSC::prefix[i].index; count = JSC::prefix[i].size; found = true; break;
    }
    if (!found) { if (ok) *ok = false; return 0; }
    for (uint32_t i = index; i < index + count; i++) {
        uint32_t len = JSC::list[i].size;
        if (strncmp(b, JSC::list[i].str, len) == 0) { if (ok) *ok = true; return JSC::list[i].index; }
    }
    if (ok) *ok = false; return 0;
}

// ---- JSC::codeword (ref: jsc.cpp:31-50) ----
static Codeword codeword(uint32_t index, bool separate, uint32_t bytesize, uint32_t s, uint32_t c) {
    std::vector<Codeword> out;
    uint32_t v = ((index % s) << 1) + (uint32_t)separate;
    out.insert(out.begin(), intToBits(v, bytesize + 1));
    uint32_t x = index / s;
    while (x > 0) { x -= 1; out.insert(out.begin(), intToBits((x % c) + s, bytesize)); x /= c; }
    Codeword word;
    for (auto &w : out) word.insert(word.end(), w.begin(), w.end());
    return word;
}

// ---- JSC::compress (ref: jsc.cpp:52-95) ----
// Returns concatenated bit stream + count of (bits,nchars) pairs via out_pairs.
struct Pair { Codeword bits; uint32_t n; };
static std::vector<Pair> compress(const std::string &text) {
    std::vector<Pair> out;
    const uint32_t b = 4, s = 7; const uint32_t c = (uint32_t)std::pow(2, 4) - s;
    // split on " " KeepEmptyParts
    std::vector<std::string> words; { size_t p = 0; while (true) { size_t q = text.find(' ', p);
        if (q == std::string::npos) { words.push_back(text.substr(p)); break; } words.push_back(text.substr(p, q - p)); p = q + 1; } }
    for (int i = 0, len = (int)words.size(); i < len; i++) {
        std::string w = words[i];
        bool isLastWord = (i == len - 1); bool isSpaceCharacter = false;
        if (w.empty() && !isLastWord) { w = " "; isSpaceCharacter = true; }
        while (!w.empty()) {
            bool ok = false; uint32_t index = lookup(w.c_str(), &ok);
            if (!ok) break;
            Tuple t = JSC::map[index];
            w = w.substr(t.size);
            bool isLast = w.empty();
            bool shouldAppendSpace = isLast && !isSpaceCharacter && !isLastWord;
            out.push_back({ codeword(index, shouldAppendSpace, b, s, c), (uint32_t)t.size + (shouldAppendSpace ? 1u : 0u) });
        }
    }
    return out;
}

// ---- JSC::decompress (ref: jsc.cpp:97-171) ----
static std::string decompress(const Codeword &bitvec) {
    const uint32_t s = 7; const uint32_t c = (uint32_t)std::pow(2, 4) - s;
    std::string out;
    uint32_t base[8];
    base[0] = 0; base[1] = s;
    base[2] = base[1] + s*c;   base[3] = base[2] + s*c*c;
    base[4] = base[3] + s*c*c*c; base[5] = base[4] + s*c*c*c*c;
    base[6] = base[5] + s*c*c*c*c*c; base[7] = base[6] + s*c*c*c*c*c*c;
    std::vector<uint64_t> bytes; std::vector<uint32_t> separators;
    int i = 0; int count = (int)bitvec.size();
    while (i < count) {
        if (i + 4 > count) break;
        Codeword b4(bitvec.begin() + i, bitvec.begin() + i + 4);
        uint64_t byte = bitsToInt(b4); bytes.push_back(byte); i += 4;
        if (byte < s) { if (count - i > 0 && bitvec[i]) separators.push_back((uint32_t)bytes.size() - 1); i += 1; }
    }
    uint32_t start = 0;
    while (start < (uint32_t)bytes.size()) {
        uint32_t k = 0, j = 0;
        while (start + k < (uint32_t)bytes.size() && bytes[start + k] >= s) { j = j*c + (uint32_t)(bytes[start + k] - s); k++; }
        if (j >= JSC::size) break;
        if (start + k >= (uint32_t)bytes.size()) break;
        j = j*s + (uint32_t)bytes[start + k] + base[k];
        if (j >= JSC::size) break;
        out += std::string(JSC::map[j].str); // latin1
        if (!separators.empty() && separators.front() == start + k) { out += " "; separators.erase(separators.begin()); }
        start = start + (k + 1);
    }
    return out;
}

// ---- helpers ----
static void write_u32(FILE *f, uint32_t v) { fwrite(&v, 4, 1, f); }

static std::string bits_to_str(const Codeword &b) { std::string s; s.reserve(b.size()); for (bool x : b) s += (x ? '1' : '0'); return s; }
static std::string json_escape(const std::string &in) {
    std::string o; for (unsigned char ch : in) {
        if (ch == '"' || ch == '\\') { o += '\\'; o += ch; }
        else if (ch == '\n') o += "\\n"; else if (ch == '\r') o += "\\r"; else if (ch == '\t') o += "\\t";
        else if (ch < 0x20 || ch >= 0x7f) { char buf[8]; snprintf(buf, sizeof buf, "\\u%04x", ch); o += buf; }
        else o += ch; }
    return o;
}

int main() {
    // ---------- 1. dictionary blob ----------
    // Format (little-endian):
    //   u32 count (=262144)
    //   words section: for each of count words: u8 map_size (reference .size field,
    //     used by compress's substr; == strlen except the ROSIDS filler @262143 where
    //     it is 1), u8 slen (strlen), then slen latin1 bytes (index order = map)
    //   u32 count again, then count u32: list[i].index (search-order permutation)
    //   u32 prefixSize (=103), then prefixSize records: u8 first_byte, u32 count, u32 list_start
    {
        FILE *f = fopen("jsc_dict.bin", "wb");
        write_u32(f, JSC::size);
        for (uint32_t i = 0; i < JSC::size; i++) {
            const char *sp = JSC::map[i].str; int l = (int)strlen(sp);
            unsigned char ms = (unsigned char)JSC::map[i].size;
            unsigned char lb = (unsigned char)l;
            fwrite(&ms, 1, 1, f); fwrite(&lb, 1, 1, f); fwrite(sp, 1, l, f);
        }
        write_u32(f, JSC::size);
        for (uint32_t i = 0; i < JSC::size; i++) write_u32(f, (uint32_t)JSC::list[i].index);
        write_u32(f, JSC::prefixSize);
        for (uint32_t i = 0; i < JSC::prefixSize; i++) {
            unsigned char fb = (unsigned char)JSC::prefix[i].str[0]; fwrite(&fb, 1, 1, f);
            write_u32(f, (uint32_t)JSC::prefix[i].size); write_u32(f, (uint32_t)JSC::prefix[i].index);
        }
        fclose(f);
    }

    // ---------- 2. golden compress/decompress vectors ----------
    const char *msgs[] = {
        "HELLO WORLD", "CQ CQ CQ DE K1ABC", "THE QUICK BROWN FOX", "ACK 73",
        "TESTING 123", "STATUS OK", "A", "E", "HELLO", "WORLD",
        "THIS IS A TEST MESSAGE FOR JS8", "QSL DE W1AW", "GM ES 73",
    };
    FILE *j = fopen("jsc_vectors.json", "wb");
    fprintf(j, "{\n  \"_meta\": {\n");
    fprintf(j, "    \"upstream\": \"js8call/js8call @ a7ff1be\",\n");
    fprintf(j, "    \"files\": [\"jsc.cpp\", \"jsc_map.cpp\", \"jsc_list.cpp\", \"varicode.cpp\"],\n");
    fprintf(j, "    \"cmd\": \"g++ -O0 jsc_map.o jsc_list.o dumper.cpp -o dumper && ./dumper\",\n");
    fprintf(j, "    \"note\": \"JSC codec golden vectors. 'compressed' is the concatenated bit stream (MSB-first per 4-bit group). 'decompressed' is decompress(compressed). Tables are verbatim from the reference; algorithm transcribed from jsc.cpp (Qt-free).\"\n");
    fprintf(j, "  },\n  \"vectors\": [\n");
    int nmsg = (int)(sizeof(msgs) / sizeof(msgs[0]));
    for (int m = 0; m < nmsg; m++) {
        std::string text = msgs[m];
        auto pairs = compress(text);
        Codeword all; uint32_t nchars = 0;
        for (auto &p : pairs) { all.insert(all.end(), p.bits.begin(), p.bits.end()); nchars += p.n; }
        std::string round = decompress(all);
        fprintf(j, "    {\"text\": \"%s\", \"nchars\": %u, \"nbits\": %zu, \"compressed\": \"%s\", \"decompressed\": \"%s\"}%s\n",
                json_escape(text).c_str(), nchars, all.size(), bits_to_str(all).c_str(),
                json_escape(round).c_str(), (m == nmsg - 1 ? "" : ","));
    }
    fprintf(j, "  ]\n}\n");
    fclose(j);

    printf("wrote jsc_dict.bin and jsc_vectors.json (%u dict entries)\n", JSC::size);
    return 0;
}
