#!/usr/bin/env bash
# Build+run the FST4 tone reference generator from unmodified wsjtx sources.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; LIB="$ROOT/wsjtx/lib"; FST4="$LIB/fst4"
B="$(mktemp -d)"; trap 'rm -rf "$B"' EXIT; cd "$B"
cp "$LIB/crc.f90" "$FST4/get_crc24.f90" .
gfortran -c crc.f90; gfortran -c get_crc24.f90
gfortran -c "$ROOT/scratch/refvectors/stub_packjt77.f90"
gfortran -c -I"$FST4" "$FST4/encode240_101.f90"
gfortran -c -I"$FST4" "$FST4/encode240_74.f90"
gfortran -c -I"$FST4" "$FST4/genfst4.f90"
gfortran "$ROOT/scratch/refvectors/fst4_tone_dump.f90" genfst4.o encode240_101.o \
    encode240_74.o get_crc24.o stub_packjt77.o crc.o -o "$B/fst4_tone_dump"
"$B/fst4_tone_dump"
