#!/usr/bin/env bash
# Build + run the fldigi IFKP picture sub-protocol golden extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# transcribes: src/ifkp/ifkp-pic.cxx (TX header char table, colour plane order)
#              and src/ifkp/ifkp.cxx (parse_pic RX size/colour table) — verbatim
#              with cites (ifkp-pic.cxx/ifkp.cxx cannot link: FLTK + modem runtime).
# emits:    JSON consumed by crates/dsp/tests/vectors/ifkppic.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_ifkppic.sh > crates/dsp/tests/vectors/ifkppic.json
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT=$(mktemp -d)
g++ -std=c++11 -O2 scratch/refvectors/ifkppic_dump.cxx -o "$OUT/ifkppic_dump"
"$OUT/ifkppic_dump"
rm -rf "$OUT"
