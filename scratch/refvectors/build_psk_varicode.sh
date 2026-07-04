#!/usr/bin/env bash
# Build + run the fldigi PSK31 Varicode golden-vector extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ./fldigi
# links:    fldigi/src/psk/pskvaricode.cxx (unmodified)
# emits:    JSON lines consumed by crates/dsp/tests/vectors/psk_bpsk.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_psk_varicode.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

FLDIGI=fldigi
OUT=$(mktemp -d)
# pskvaricode.cxx only needs <config.h> (empty stub is fine) and its own header.
printf '' > "$OUT/config.h"

g++ -std=c++11 -O2 \
    -I "$OUT" \
    -I "$FLDIGI/src/include" \
    scratch/refvectors/psk_varicode_dump.cxx \
    "$FLDIGI/src/psk/pskvaricode.cxx" \
    -o "$OUT/psk_varicode_dump"

"$OUT/psk_varicode_dump"
rm -rf "$OUT"
