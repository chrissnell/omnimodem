#!/usr/bin/env bash
# Build the reference 77-bit message packer from unmodified wsjtx sources.
# Usage: build_pack77.sh "CQ K1ABC FN42"
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; LIB="$ROOT/wsjtx/lib"
B="$(mktemp -d)"; trap 'rm -rf "$B"' EXIT; cd "$B"
gfortran -c "$LIB/packjt.f90" "$LIB/77bit/packjt77.f90"
gfortran "$ROOT/scratch/refvectors/pack77_dump.f90" packjt77.o packjt.o \
    "$LIB/fmtmsg.f90" "$LIB/deg2grid.f90" "$LIB/grid2deg.f90" "$LIB/chkcall.f90" -o "$B/pack77_dump"
"$B/pack77_dump" "$1"
