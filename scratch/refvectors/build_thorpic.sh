#!/usr/bin/env bash
# Build + run the fldigi THOR picture sub-protocol golden extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# transcribes: src/thor/thor-pic.cxx (TX header builder, picmode="pic% \n")
#              and src/thor/thor.cxx (parse_pic RX size/colour table, colour
#              plane order) — verbatim with cites (thor-pic.cxx/thor.cxx cannot
#              link: FLTK + modem runtime).
# emits:    JSON consumed by crates/dsp/tests/vectors/thorpic.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_thorpic.sh > crates/dsp/tests/vectors/thorpic.json
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT=$(mktemp -d)
g++ -std=c++11 -O2 scratch/refvectors/thorpic_dump.cxx -o "$OUT/thorpic_dump"
"$OUT/thorpic_dump"
rm -rf "$OUT"
