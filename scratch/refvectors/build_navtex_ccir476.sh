#!/usr/bin/env bash
# Build + run the fldigi NAVTEX / SITOR-B CCIR-476 golden-vector extractor.
#
# upstream: fldigi 4.1.23 (commit 61b97f413), checked out at ../fldigi
# source:   src/navtex/navtex.cxx:465-592 (CCIR-476 tables + class),
#           src/navtex/navtex.cxx:1711-1743 (create_fec + encode) — the
#           self-contained table-driven pieces are transcribed verbatim into
#           navtex_ccir476_dump.cxx (the modem class cannot link standalone:
#           FLTK / fftfilt / modem runtime), same convention as
#           dominoex_varicode_dump.cxx.
# emits:    JSON consumed by crates/dsp/tests/vectors/navtex_ccir476.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_navtex_ccir476.sh > crates/dsp/tests/vectors/navtex_ccir476.json
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=$(mktemp -d)
g++ -std=c++11 -O2 \
    scratch/refvectors/navtex_ccir476_dump.cxx \
    -o "$OUT/navtex_ccir476_dump"
"$OUT/navtex_ccir476_dump"
rm -rf "$OUT"
