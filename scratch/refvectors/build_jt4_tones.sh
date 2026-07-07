#!/usr/bin/env bash
# Build+run the JT4 channel-symbol reference generator from unmodified wsjtx
# sources. Prints the 72 payload bits then the 206 4-FSK symbol values (0..3)
# for a fixed payload. Upstream: WSJTX/wsjtx (see lib/jt4.f90, gen4.f90,
# encode232.f90, interleave4.f90). Isolates the FEC/interleave/sync stages.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# wsjtx checks out in-tree (ROOT/wsjtx) or as a sibling of the repo; accept both.
LIB="${WSJTX_LIB:-$ROOT/wsjtx/lib}"
[ -d "$LIB" ] || LIB="$ROOT/../wsjtx/lib"
B="$(mktemp -d)"; trap 'rm -rf "$B"' EXIT; cd "$B"
for f in jt4 entail encode232 interleave4; do cp "$LIB/$f.f90" .; done
cp "$LIB/conv232.f90" .
gfortran -c jt4.f90
for f in entail encode232 interleave4; do gfortran -c -I"$LIB" "$f.f90"; done
gfortran "$ROOT/scratch/refvectors/jt4_tone_dump.f90" \
  entail.o encode232.o interleave4.o jt4.o -o "$B/jt4_tone_dump"
"$B/jt4_tone_dump"
