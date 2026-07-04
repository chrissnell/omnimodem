#!/usr/bin/env bash
# Build + run the fldigi MFSK (IZ8BLY) Varicode golden-vector extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ./fldigi
# links:    fldigi/src/mfsk/mfskvaricode.cxx (unmodified)
# emits:    JSON consumed by crates/dsp/tests/vectors/psk_mfsk.json
#
# mfskvaricode.cxx only needs <config.h> (empty stub) and its own header.
# Run from the omnimodem repo root:
#   scratch/refvectors/build_mfsk_varicode.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

FLDIGI=fldigi
OUT=$(mktemp -d)
printf '' > "$OUT/config.h"

g++ -std=c++11 -O2 \
    -I "$OUT" \
    -I "$FLDIGI/src/include" \
    scratch/refvectors/mfsk_varicode_dump.cxx \
    "$FLDIGI/src/mfsk/mfskvaricode.cxx" \
    -o "$OUT/mfsk_varicode_dump"

"$OUT/mfsk_varicode_dump"
rm -rf "$OUT"
