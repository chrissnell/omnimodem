#!/usr/bin/env bash
# Build + run the fldigi QPSK FEC golden-vector extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ./fldigi
# links:    fldigi/src/filters/viterbi.cxx (real `encoder`) + src/psk/pskvaricode.cxx
# emits:    JSON consumed by crates/dsp/tests/vectors/psk_qpsk.json
#
# viterbi.cxx only needs `parity()` (provided in the driver) and <config.h>;
# a stub include dir shadows fldigi's misc.h with a one-line declaration so we
# don't drag the whole app in. Run from the omnimodem repo root:
#   scratch/refvectors/build_psk_qpsk.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

FLDIGI=fldigi
OUT=$(mktemp -d)
printf '' > "$OUT/config.h"
printf 'extern int parity(unsigned long w);\n' > "$OUT/misc.h"

g++ -std=c++11 -O2 \
    -I "$OUT" \
    -I "$FLDIGI/src/include" \
    scratch/refvectors/psk_qpsk_dump.cxx \
    "$FLDIGI/src/filters/viterbi.cxx" \
    "$FLDIGI/src/psk/pskvaricode.cxx" \
    -o "$OUT/psk_qpsk_dump"

"$OUT/psk_qpsk_dump"
rm -rf "$OUT"
