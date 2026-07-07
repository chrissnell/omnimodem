#!/usr/bin/env bash
# Generate the reference JS8 waveforms for the cross-decode gate. Pure gfortran
# (reference encode174 + genjs8 tone assembly + genjs8refsig) — no Qt/boost/FFTW.
# The 87-bit MSGBITS is emitted by the Rust side (a "CQ K1ABC" data frame).
set -euo pipefail
JS=${1:-/work/1d8f9b53-fbd4-4904-b388-7257e3f8eb55/57e69dd7/workdir/js8call}
MSGBITS=011111100010110010010010110000001111000111001110101001111111111111111111111011100100110
cd "$(dirname "$0")/refwave"
cp "$JS/lib/ft8/encode174.f90" "$JS/lib/ft8/ldpc_174_87_params.f90" .
gfortran -O2 -o dump_refwave dump_refwave.f90 encode174.f90
./dump_refwave $MSGBITS 1920 1 1500 js8_ref_normal.i16   # Normal, original Costas
./dump_refwave $MSGBITS 600  2 1500 js8_ref_turbo.i16    # Turbo, symmetrical Costas
echo "-> copy js8_ref_*.i16 to crates/dsp/tests/vectors/ after review"
