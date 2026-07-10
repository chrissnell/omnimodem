#!/usr/bin/env bash
# Build + run the fldigi FSQ picture sub-protocol golden extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# transcribes: src/fsq/fsq-pic.cxx (TX header tokens) and src/fsq/fsq.cxx
#              (parse_pcnt RX table, B->G->R plane order, quantiser affines) —
#              verbatim with cites (fsq-pic.cxx/fsq.cxx cannot link: FLTK +
#              modem runtime).
# emits:    JSON consumed by crates/dsp/tests/vectors/fsqpic.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_fsqpic.sh > crates/dsp/tests/vectors/fsqpic.json
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT=$(mktemp -d)
g++ -std=c++11 -O2 scratch/refvectors/fsqpic_dump.cxx -o "$OUT/fsqpic_dump"
"$OUT/fsqpic_dump"
rm -rf "$OUT"
