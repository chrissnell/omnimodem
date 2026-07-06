#!/usr/bin/env bash
# Build + run the fldigi MFSK picture sub-protocol golden extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# transcribes: src/mfsk/mfsk-pic.cxx (TX header/colour-reorder/luma) and
#              src/mfsk/mfsk.cxx (check_picture_header RX parser) — the pure
#              integer/string functions, verbatim with cites (mfsk-pic.cxx /
#              mfsk.cxx cannot link standalone: FLTK + modem runtime).
# emits:    JSON consumed by crates/dsp/tests/vectors/mfskpic.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_mfskpic.sh > crates/dsp/tests/vectors/mfskpic.json
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=$(mktemp -d)
g++ -std=c++11 -O2 scratch/refvectors/mfskpic_dump.cxx -o "$OUT/mfskpic_dump"
"$OUT/mfskpic_dump"
rm -rf "$OUT"
