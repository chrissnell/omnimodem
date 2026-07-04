#!/usr/bin/env bash
# Build + run the fldigi PSK-R (K=7 robust) FEC golden-vector extractor.
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ./fldigi
# links: fldigi/src/filters/viterbi.cxx + src/mfsk/mfskvaricode.cxx
# emits: JSON consumed by crates/dsp/tests/vectors/psk_robust.json
# Run from the omnimodem repo root: scratch/refvectors/build_psk_robust.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
FLDIGI=fldigi
OUT=$(mktemp -d)
printf '' > "$OUT/config.h"
printf 'extern int parity(unsigned long w);\n' > "$OUT/misc.h"
g++ -std=c++11 -O2 -I "$OUT" -I "$FLDIGI/src/include" \
    scratch/refvectors/psk_robust_dump.cxx \
    "$FLDIGI/src/filters/viterbi.cxx" \
    "$FLDIGI/src/mfsk/mfskvaricode.cxx" \
    -o "$OUT/psk_robust_dump"
"$OUT/psk_robust_dump"
rm -rf "$OUT"
