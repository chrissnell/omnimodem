// Minimal (Qt-free) header to compile js8call's jsc_map.cpp / jsc_list.cpp
// unmodified. Only declares the static table storage they define.
// ref: js8call/jsc.h (a7ff1be) — Tuple layout + array sizes preserved verbatim.
#ifndef JSC_H
#define JSC_H
typedef struct Tuple {
    char const * str;
    int size;
    int index;
} Tuple;
class JSC {
public:
    static const unsigned int size = 262144;
    static const Tuple map[262144];
    static const Tuple list[262144];
    static const unsigned int prefixSize = 103;
    static const Tuple prefix[103];
};
#endif
