#!/usr/bin/env bash
# Build + run the MMSSTV SSTV TX golden-vector extractor (GRA-289, Phase 17).
# Links the *unmodified* MMSSTV DSP core (sstv.cpp + fir.cpp) via the fake-<vcl.h>
# shim, with zero edits to the reference tree.
#   upstream: n5ac/mmsstv @ 8060b5f, checked out at ../mmsstv (repo sibling)
#   emits:    JSON consumed by crates/dsp/tests/vectors/sstv_scottie1_tx.json
# Run from the omnimodem repo root:
#   scratch/refvectors/build_sstv_tx.sh > crates/dsp/tests/vectors/sstv_scottie1_tx.json
set -euo pipefail

MM=${MMSSTV:-../mmsstv}
SHIM="$(dirname "$0")/sstv_shim"
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

CXXFLAGS="-std=c++14 -O2 -w -Wno-unknown-pragmas -fpermissive -include $SHIM/ComLib.h -I $SHIM -I $MM"

g++ $CXXFLAGS -c "$MM/fir.cpp"                       -o "$OUT/fir.o"
g++ $CXXFLAGS -c "$MM/sstv.cpp"                      -o "$OUT/sstv.o"
g++ $CXXFLAGS -c "$(dirname "$0")/sstv_tx_dump.cxx"  -o "$OUT/dump.o"
g++ "$OUT/fir.o" "$OUT/sstv.o" "$OUT/dump.o" -o "$OUT/sstv_tx_dump"

"$OUT/sstv_tx_dump"
