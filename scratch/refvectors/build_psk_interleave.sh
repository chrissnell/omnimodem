#!/usr/bin/env bash
# Build + run the fldigi PSK-R (de)interleaver golden-vector extractor.
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ./fldigi
# links: fldigi/src/mfsk/interleave.cxx
# emits: JSON consumed by crates/dsp/tests/vectors/psk_interleave.json
# Run from the omnimodem repo root: scratch/refvectors/build_psk_interleave.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
FLDIGI=fldigi
OUT=$(mktemp -d)
printf '' > "$OUT/config.h"
g++ -std=c++11 -O2 -I "$OUT" -I "$FLDIGI/src/include" \
    scratch/refvectors/psk_interleave_dump.cxx \
    "$FLDIGI/src/mfsk/interleave.cxx" \
    -o "$OUT/psk_interleave_dump"
"$OUT/psk_interleave_dump"
rm -rf "$OUT"
