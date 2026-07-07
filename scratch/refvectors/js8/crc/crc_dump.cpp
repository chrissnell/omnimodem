// Authoritative CRC-12 golden vectors from the UNMODIFIED js8call crc12.cpp
// (boost::augmented_crc<12,0xc06>). Emits, for a set of 11-byte buffers, the raw
// crc12 and crc12^42 (the js8_crc12 form). Validates the Rust transcription.
#include <cstdio>
#include <cstring>
extern "C" { short crc12(unsigned char const* data, int length); }

static void run(const char* label, unsigned char* b) {
    unsigned short c = (unsigned short)crc12(b, 11) & 0x0FFF;
    printf("%s raw=%u xor42=%u\n", label, c, c ^ 42);
}

int main() {
    // A few deterministic 11-byte buffers (last 12 bits are the zeroed CRC slot).
    unsigned char b1[11] = {0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00};
    unsigned char b2[11] = {0x01,0x02,0x03,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00};
    unsigned char b3[11] = {0xAB,0xCD,0xEF,0x12,0x34,0x56,0x78,0x9A,0xBC,0x00,0x00};
    unsigned char b4[11] = {0xDE,0xAD,0xBE,0xEF,0xCA,0xFE,0xBA,0xBE,0x11,0x80,0x00}; // byte9 i3bit=4<<5=0x80
    run("b1", b1); run("b2", b2); run("b3", b3); run("b4", b4);
    return 0;
}
