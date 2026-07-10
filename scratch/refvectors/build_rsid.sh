#!/usr/bin/env bash
# Build + run the fldigi RSID (Reed-Solomon Identifier) golden-vector extractor.
#
# upstream: w1hkj/fldigi, checked out at ../fldigi
# links:    nothing — rsid_dump.cxx transcribes the pure-integer RS encoder
#           (rsid.cxx Squares/indices/Encode) and copies the two ID tables
#           (rsid_defs.cxx RSID_LIST / RSID_LIST2) verbatim. fldigi's cRsId
#           object cannot link standalone (drags in the fltk/modem runtime), so
#           the feldhell_dump.cxx precedent — transcribe integer logic, cite it —
#           is followed here.
# emits:    JSON consumed by crates/dsp/tests/vectors/rsid.json
#
# Run from the omnimodem repo root:
#   scratch/refvectors/build_rsid.sh > crates/dsp/tests/vectors/rsid.json
set -euo pipefail
cd "$(dirname "$0")/../.."

OUT=$(mktemp -d)
g++ -std=c++11 -O2 scratch/refvectors/rsid_dump.cxx -o "$OUT/rsid_dump"
"$OUT/rsid_dump"
rm -rf "$OUT"
