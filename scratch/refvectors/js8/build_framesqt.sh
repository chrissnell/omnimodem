#!/usr/bin/env bash
# Authoritative callsign + compound/directed frame vectors from the REAL
# varicode.cpp (Qt build). Requires Qt5Core + boost + the js8call checkout.
# Note: one Qt-version patch on the local copy — varicode.cpp:1246
# `callsign.first(index)` → `callsign.left(index)` (QStringView::first(n) needs
# a newer Qt than the installed 5.15). Does not affect the pack functions.
set -euo pipefail
JS=${1:-/work/1d8f9b53-fbd4-4904-b388-7257e3f8eb55/57e69dd7/workdir/js8call}
cd "$(dirname "$0")/framesqt"
cp "$JS/varicode.h" "$JS/jsc.h" "$JS/varicode.cpp" "$JS/jsc.cpp" "$JS/jsc_map.cpp" "$JS/jsc_list.cpp" "$JS/lib/crc12.cpp" .
sed -i 's/callsign\.first(index)/callsign.left(index)/g' varicode.cpp
/usr/bin/moc varicode.h -o moc_varicode.cpp
CF=$(pkg-config --cflags Qt5Core); LF=$(pkg-config --libs Qt5Core)
g++ -O2 -std=c++17 -fPIC -I. -I"$JS" $CF frames_dump.cpp varicode.cpp moc_varicode.cpp \
  jsc.cpp jsc_map.cpp jsc_list.cpp crc12.cpp $LF -o frames_dump
./frames_dump
