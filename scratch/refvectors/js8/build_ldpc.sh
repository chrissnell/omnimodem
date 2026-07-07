#!/usr/bin/env bash
# Authoritative (174,87) LDPC codeword vector from the UNMODIFIED js8call encoder.
# Requires js8call checked out at ../../../../js8call.
set -euo pipefail
JS=${1:-/work/1d8f9b53-fbd4-4904-b388-7257e3f8eb55/57e69dd7/workdir/js8call}
cd "$(dirname "$0")/ldpc"
cp "$JS/lib/ft8/encode174.f90" "$JS/lib/ft8/ldpc_174_87_params.f90" .
gfortran -O2 -o dump_encode174 dump_encode174.f90 encode174.f90
./dump_encode174
