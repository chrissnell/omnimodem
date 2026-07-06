#!/usr/bin/env bash
# Build + run the fldigi FSQ (FSQCALL) Varicode + IFK + CRC8 golden extractor.
#
# upstream: fldigi 4.2.x, checked out at ../fldigi
# links:    fldigi/src/fsq/fsq_varicode.cxx (unmodified)
# emits:    JSON lines consumed by crates/dsp/tests/vectors/fsq_varicode.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_fsq_varicode.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

FLDIGI=${FLDIGI:-../fldigi}
OUT=$(mktemp -d)

g++ -std=c++11 -O2 \
    scratch/refvectors/fsq_varicode_dump.cxx \
    "$FLDIGI/src/fsq/fsq_varicode.cxx" \
    -o "$OUT/fsq_varicode_dump"

"$OUT/fsq_varicode_dump"
rm -rf "$OUT"
