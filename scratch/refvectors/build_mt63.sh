#!/usr/bin/env bash
# Build + run the fldigi MT63 encoder + modulator-phase golden extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# links:    fldigi/src/mt63/mt63base.cxx (MT63encoder + MT63tx) and
#           fldigi/src/mt63/dsp.cxx (Walsh transform + FFT/window/comb), plus the
#           symbol.dat / mt63intl.dat data files they #include — all unmodified.
# emits:    JSON consumed by crates/dsp/tests/vectors/mt63.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_mt63.sh > crates/dsp/tests/vectors/mt63.json
set -euo pipefail
cd "$(dirname "$0")/../.."

FLDIGI=${FLDIGI:-../fldigi}
OUT=$(mktemp -d)
printf '' > "$OUT/config.h"

g++ -std=c++11 -O2 \
    -I "$OUT" \
    -I "$FLDIGI/src/include" \
    -I "$FLDIGI/src/mt63" \
    scratch/refvectors/mt63_dump.cxx \
    "$FLDIGI/src/mt63/mt63base.cxx" \
    "$FLDIGI/src/mt63/dsp.cxx" \
    -o "$OUT/mt63_dump"

"$OUT/mt63_dump"
rm -rf "$OUT"
