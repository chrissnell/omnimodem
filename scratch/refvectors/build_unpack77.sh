#!/usr/bin/env bash
# Build the reference 77-bit message unpacker. Usage: build_unpack77.sh <77 bits>
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; LIB="$ROOT/wsjtx/lib"
B="$(mktemp -d)"; trap 'rm -rf "$B"' EXIT; cd "$B"
gfortran -c "$LIB/packjt.f90" "$LIB/77bit/packjt77.f90"
gfortran "$ROOT/scratch/refvectors/unpack77_dump.f90" packjt77.o packjt.o \
    "$LIB/fmtmsg.f90" "$LIB/deg2grid.f90" "$LIB/grid2deg.f90" "$LIB/chkcall.f90" -o "$B/unpack77_dump"
"$B/unpack77_dump" "$1"
