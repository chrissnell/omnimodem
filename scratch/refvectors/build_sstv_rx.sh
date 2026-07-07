#!/usr/bin/env bash
# Build + run the MMSSTV SSTV RX golden-vector / cross-decode extractor (GRA-289).
# Renders one mode's TX audio with the unmodified CSSTVMOD, feeds it back through the
# unmodified CSSTVDEM, and reports the detected VIS -> mode + sync-state trace. This is
# the "reference decodes our TX" substitute (MMSSTV ships no Linux CLI decoder).
#   upstream: n5ac/mmsstv @ 8060b5f, checked out at ../mmsstv
# Run from the omnimodem repo root:
#   scratch/refvectors/build_sstv_rx.sh > crates/dsp/tests/vectors/sstv_scottie1_rx.json
set -euo pipefail

MM=${MMSSTV:-../mmsstv}
SHIM="$(dirname "$0")/sstv_shim"
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

CXXFLAGS="-std=c++14 -O2 -w -Wno-unknown-pragmas -fpermissive -include $SHIM/ComLib.h -I $SHIM -I $MM"

g++ $CXXFLAGS -c "$MM/fir.cpp"                       -o "$OUT/fir.o"
g++ $CXXFLAGS -c "$MM/sstv.cpp"                      -o "$OUT/sstv.o"
g++ $CXXFLAGS -c "$(dirname "$0")/sstv_rx_dump.cxx"  -o "$OUT/rx.o"
g++ "$OUT/fir.o" "$OUT/sstv.o" "$OUT/rx.o" -o "$OUT/sstv_rx_dump"

"$OUT/sstv_rx_dump"
