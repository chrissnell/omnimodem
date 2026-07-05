#!/usr/bin/env bash
# Build + run the fldigi Feld Hell font + on-air column-stream golden extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# links:    fldigi/src/feld/feldfonts.cxx (+ the fifteen Feld*-{12,14}.cxx it
#           #includes) — the bitmap font tables, unmodified.
# emits:    JSON consumed by crates/dsp/tests/vectors/feldhell.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_feldhell.sh > crates/dsp/tests/vectors/feldhell.json
set -euo pipefail
cd "$(dirname "$0")/../.."

# fldigi is checked out alongside omnimodem in the workspace.
FLDIGI=${FLDIGI:-../fldigi}
OUT=$(mktemp -d)
# feldfonts.cxx only needs <config.h> (empty stub is fine) and fontdef.h.
printf '' > "$OUT/config.h"

g++ -std=c++11 -O2 \
    -I "$OUT" \
    -I "$FLDIGI/src/include" \
    scratch/refvectors/feldhell_dump.cxx \
    "$FLDIGI/src/feld/feldfonts.cxx" \
    -o "$OUT/feldhell_dump"

"$OUT/feldhell_dump"
rm -rf "$OUT"
