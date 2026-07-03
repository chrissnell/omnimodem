#!/usr/bin/env bash
# Build the FST4 LDPC golden-vector generator from the unmodified wsjtx sources.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LIB="$ROOT/wsjtx/lib"; FST4="$LIB/fst4"
B="$(mktemp -d)"; trap 'rm -rf "$B"' EXIT
cd "$B"
gfortran -c "$LIB/crc.f90"                        # crc.mod (interfaces only)
gfortran -c -I"$FST4" "$FST4/encode240_101.f90"   # includes ldpc_240_101_generator.f90
gfortran -c -I"$FST4" "$FST4/encode240_74.f90"
gfortran -I"$FST4" "$ROOT/scratch/refvectors/fst4_ldpc_dump.f90" \
    encode240_101.o encode240_74.o -o "$B/fst4_ldpc_dump"
"$B/fst4_ldpc_dump"
