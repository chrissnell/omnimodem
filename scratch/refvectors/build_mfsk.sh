#!/usr/bin/env bash
# Build + run the fldigi MFSK TX-chain golden-vector extractor.
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# links (unmodified): src/filters/viterbi.cxx, src/mfsk/interleave.cxx,
#                     src/mfsk/mfskvaricode.cxx  (grayencode/parity inlined)
# emits: JSON consumed by crates/dsp/tests/vectors/mfsk.json
# Run from the omnimodem repo root: scratch/refvectors/build_mfsk.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
FLDIGI=${FLDIGI:-../fldigi}
OUT=$(mktemp -d)
printf '' > "$OUT/config.h"
g++ -std=c++11 -O2 -I "$OUT" -I "$FLDIGI/src/include" \
    scratch/refvectors/mfsk_dump.cxx \
    "$FLDIGI/src/filters/viterbi.cxx" \
    "$FLDIGI/src/mfsk/interleave.cxx" \
    "$FLDIGI/src/mfsk/mfskvaricode.cxx" \
    -o "$OUT/mfsk_dump"
"$OUT/mfsk_dump"
rm -rf "$OUT"
