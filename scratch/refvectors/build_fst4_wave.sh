#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"; LIB="$ROOT/wsjtx/lib"
B="$(mktemp -d)"; trap 'rm -rf "$B"' EXIT; cd "$B"
gfortran -c "$ROOT/scratch/refvectors/stub_prog_args.f90"
gfortran -c "$LIB/ft2/gfsk_pulse.f90"
gfortran -c "$LIB/fst4/gen_fst4wave.f90"
gfortran "$ROOT/scratch/refvectors/fst4_wave_dump.f90" gen_fst4wave.o gfsk_pulse.o \
    stub_prog_args.o -o "$B/fst4_wave_dump"
"$B/fst4_wave_dump"
