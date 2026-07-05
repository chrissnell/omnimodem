#!/usr/bin/env bash
# Build + run the fldigi DominoEX Varicode + IFK+ golden-vector extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# links:    fldigi/src/dominoex/dominovar.cxx (unmodified)
# emits:    JSON lines consumed by crates/dsp/tests/vectors/dominoex_varicode.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_dominoex_varicode.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

# fldigi is checked out alongside omnimodem in the workspace.
FLDIGI=${FLDIGI:-../fldigi}
OUT=$(mktemp -d)
# dominovar.cxx only needs <config.h> (empty stub is fine) and its own header.
printf '' > "$OUT/config.h"

g++ -std=c++11 -O2 \
    -I "$OUT" \
    -I "$FLDIGI/src/include" \
    scratch/refvectors/dominoex_varicode_dump.cxx \
    "$FLDIGI/src/dominoex/dominovar.cxx" \
    -o "$OUT/dominoex_varicode_dump"

"$OUT/dominoex_varicode_dump"
rm -rf "$OUT"
