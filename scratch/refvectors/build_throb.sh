#!/usr/bin/env bash
# Build + run the fldigi Throb / ThrobX golden-vector extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# source:   src/throb/throb.cxx (tone-pair + char tables, tx_process framing)
# emits:    JSON consumed by crates/dsp/tests/vectors/throb.json
#
# throb.cxx's tx_process() is a `modem` subclass method tied to the FLTK/config
# runtime, so — unlike the pskvaricode/dominovar drivers — it cannot be linked
# standalone. The driver transcribes the reference tables verbatim (cited) and
# replays only tx_process()'s char->symbol selection. Run from the repo root:
#   scratch/refvectors/build_throb.sh > crates/dsp/tests/vectors/throb.json
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=$(mktemp -d)
g++ -std=c++11 -O2 scratch/refvectors/throb_dump.cxx -o "$OUT/throb_dump"
"$OUT/throb_dump"
rm -rf "$OUT"
