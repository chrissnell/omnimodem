#!/usr/bin/env bash
# Extract the JSC dictionary blob + golden compress/decompress vectors for the
# omnimodem JS8 port (Phase W5). Requires the js8call reference checked out at
# ../../../../js8call (multica repo checkout git@github.com:js8call/js8call.git).
#
# Provenance: js8call @ a7ff1be — tables jsc_map.cpp / jsc_list.cpp are linked
# UNMODIFIED via a Qt-free jsc.h; the compress/decompress algorithm is transcribed
# verbatim from jsc.cpp (jsc.cpp itself needs Qt, unavailable here).
#
# Outputs (copied into the tree by hand after review):
#   jsc_dict.bin      -> crates/dsp/src/framing/jsc_dict.bin
#   jsc_vectors.json  -> crates/dsp/tests/vectors/js8_jsc.json
set -euo pipefail
REF=${1:-../../../js8call}
mkdir -p build && cd build
cp ../jsc.h .
cp "$REF"/jsc_map.cpp "$REF"/jsc_list.cpp .
g++ -O0 -c jsc_map.cpp -o jsc_map.o
g++ -O0 -c jsc_list.cpp -o jsc_list.o
g++ -O2 -std=c++17 jsc_map.o jsc_list.o ../dumper.cpp -o dumper
./dumper
echo "wrote build/jsc_dict.bin build/jsc_vectors.json"
