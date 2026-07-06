#!/usr/bin/env bash
# Build + run the fldigi IFKP Varicode + IFK golden-vector extractor.
#
# upstream: fldigi 4.2.x, checked out at ../fldigi
# links:    fldigi/src/ifkp/ifkp_varicode.cxx (unmodified)
# emits:    JSON lines consumed by crates/dsp/tests/vectors/ifkp_varicode.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_ifkp_varicode.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

FLDIGI=${FLDIGI:-../fldigi}
OUT=$(mktemp -d)

g++ -std=c++11 -O2 \
    scratch/refvectors/ifkp_varicode_dump.cxx \
    "$FLDIGI/src/ifkp/ifkp_varicode.cxx" \
    -o "$OUT/ifkp_varicode_dump"

"$OUT/ifkp_varicode_dump"
rm -rf "$OUT"
