#!/usr/bin/env bash
# Build + run the fldigi THOR golden-vector extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# links:    fldigi/src/filters/viterbi.cxx, src/mfsk/interleave.cxx,
#           src/mfsk/mfskvaricode.cxx, src/thor/thorvaricode.cxx (all unmodified)
# emits:    JSON lines consumed by crates/dsp/tests/vectors/thor_varicode.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_thor.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

FLDIGI=${FLDIGI:-../fldigi}
OUT=$(mktemp -d)
printf '' > "$OUT/config.h"

g++ -std=c++11 -O2 \
    -I "$OUT" \
    -I "$FLDIGI/src/include" \
    scratch/refvectors/thor_dump.cxx \
    "$FLDIGI/src/filters/viterbi.cxx" \
    "$FLDIGI/src/mfsk/interleave.cxx" \
    "$FLDIGI/src/mfsk/mfskvaricode.cxx" \
    "$FLDIGI/src/thor/thorvaricode.cxx" \
    -o "$OUT/thor_dump"

"$OUT/thor_dump"
rm -rf "$OUT"
