# JT65 golden-vector oracle (WSJT-X reference)

Produces `crates/dsp/tests/vectors/jt65/golden.txt` — for each of the 68
canonical `testmsg.f90` messages: `itype`, `dgen(12)`, `sent(63)`, `itone(126)`
straight from WSJT-X's own `packmsg` / `rs_encode` / `gen65`.

## Reproduce (needs gfortran + gcc; we used image `gcc:9`)

Copy these files from `wsjtx/lib/` next to `jt65vec.f90` + `build.sh`:
  packjt.f90 pfx.f90 gen65.f90 chkmsg.f90 fmtmsg.f90 interleave63.f90
  graycode65.f90 move.f90 grid2deg.f90 deg2grid.f90 testmsg.f90
  wrapkarn.c init_rs.c encode_rs.c decode_rs.c igray.c rs.h char.h int.h

Two source fix-ups for modern toolchains (WSJT-X built on older gcc):
  1. In `int.h`, delete the 4 forward prototypes for INIT_RS/ENCODE_RS/
     DECODE_RS/FREE_RS (they conflict with init_rs.c's `unsigned` params;
     the definitions + rs.h suffice).
  2. Build C RS units with `-DBIGSYM` (selects int.h → the `_int` codec,
     NROOTS=51).

Then:
  gcc -DBIGSYM -c init_rs.c encode_rs.c decode_rs.c
  gcc -c wrapkarn.c igray.c
  gfortran -std=legacy -c packjt.f90
  gfortran -std=legacy -o jt65vec jt65vec.f90 packjt.o gen65.f90 chkmsg.f90 \
    fmtmsg.f90 interleave63.f90 graycode65.f90 move.f90 grid2deg.f90 \
    deg2grid.f90 wrapkarn.o init_rs.o encode_rs.o decode_rs.o igray.o
  ./jt65vec > golden.txt
