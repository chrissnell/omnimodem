#!/usr/bin/env bash
# Confirm the JSC codec bit-exact against the REAL Qt jsc.cpp. Requires Qt5Core
# + the js8call checkout. Emits text|nchars|compressed|decompressed lines.
set -euo pipefail
JS=${1:-/work/1d8f9b53-fbd4-4904-b388-7257e3f8eb55/57e69dd7/workdir/js8call}
cd "$(dirname "$0")/jscqt"
cp "$JS/jsc.h" .
CF=$(pkg-config --cflags Qt5Core); LF=$(pkg-config --libs Qt5Core)
g++ -O2 -std=c++17 -fPIC $CF jsc_qt_dump.cpp varicode_min.cpp \
  "$JS/jsc.cpp" "$JS/jsc_map.cpp" "$JS/jsc_list.cpp" $LF -o jsc_qt_dump
./jsc_qt_dump
