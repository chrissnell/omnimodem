#!/bin/sh
set -e
cd /work
# Karn RS: the _int variants require int.h (selected by -DBIGSYM), which also
# hardwires NROOTS=51 (JT65). ref: wsjtx/lib/int.h
gcc -DBIGSYM -c init_rs.c encode_rs.c
gcc -c wrapkarn.c igray.c
gfortran -std=legacy -fallow-argument-mismatch -c packjt.f90
gfortran -std=legacy -fallow-argument-mismatch -o jt65vec jt65vec.f90 packjt.o \
  gen65.f90 chkmsg.f90 fmtmsg.f90 interleave63.f90 graycode65.f90 move.f90 grid2deg.f90 \
  wrapkarn.o init_rs.o encode_rs.o igray.o
./jt65vec > jt65_golden.txt
echo "=== BUILD+RUN OK; $(wc -l < jt65_golden.txt) lines ==="
head -3 jt65_golden.txt
