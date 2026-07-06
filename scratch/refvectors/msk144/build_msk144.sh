#!/usr/bin/env bash
# Build+run the MSK144 golden-vector generator from unmodified wsjtx sources.
#
# Requires the boost CRC header (header-only). We fetch boostorg/crc +
# boostorg/config shallowly into $BOOST_INC if not already present.
#
# Reference sources (unmodified): wsjtx/lib/{genmsk_128_90,encode_128_90,crc,
# crc13.cpp,ldpc_128_90_generator.f90,ldpc_128_90_reordered_parity.f90}.
# Injected 77-bit message via scratch/refvectors/msk144/stub_packjt77_inject.f90.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
# wsjtx reference may be checked out inside the repo (wsjtx/) or as a sibling
# (../wsjtx) per the issue's "checked out at ../wsjtx" note.
if [ -d "$ROOT/wsjtx/lib" ]; then LIB="$ROOT/wsjtx/lib"; else LIB="$ROOT/../wsjtx/lib"; fi
HERE="$ROOT/scratch/refvectors/msk144"
BOOST_INC="${BOOST_INC:-/tmp/boostinc}"

if [ ! -f "$BOOST_INC/boost/crc.hpp" ]; then
  tmp="$(mktemp -d)"
  git clone --depth 1 https://github.com/boostorg/crc.git "$tmp/crc"
  git clone --depth 1 https://github.com/boostorg/config.git "$tmp/config"
  mkdir -p "$BOOST_INC/boost"
  cp -r "$tmp/crc/include/boost/." "$BOOST_INC/boost/"
  cp -r "$tmp/config/include/boost/." "$BOOST_INC/boost/"
  rm -rf "$tmp"
fi

B="$(mktemp -d)"; trap 'rm -rf "$B"' EXIT; cd "$B"
cp "$LIB/crc.f90" "$LIB/encode_128_90.f90" "$LIB/genmsk_128_90.f90" \
   "$LIB/ldpc_128_90_generator.f90" .
g++ -I"$BOOST_INC" -c "$LIB/crc13.cpp" -o crc13.o
gfortran -c crc.f90
gfortran -c "$HERE/stub_packjt77_inject.f90"
gfortran -c "$HERE/stub_genmsk40.f90"
gfortran -c -I. encode_128_90.f90
gfortran -c -I. genmsk_128_90.f90
gfortran "$HERE/msk144_dump.f90" genmsk_128_90.o encode_128_90.o \
   stub_packjt77_inject.o stub_genmsk40.o crc.o crc13.o -lstdc++ -o msk144_dump
./msk144_dump "$@"
