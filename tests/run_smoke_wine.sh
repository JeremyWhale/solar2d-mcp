#!/bin/bash
# Dev-only: run the Windows build under Wine on a hidden Xvfb display and smoke-test it.
set -u
T="${SOLAR2D_MCP_TOOLS:-/media/jeremy/1TB SSD/solar2d-mcp-tools}"; . "$T/env.sh"
R="$(cd "$(dirname "$0")/.." && pwd)"
EXE="$R/target/x86_64-pc-windows-gnu/release/solar2d-mcp.exe"
"$T/xvfb/usr/bin/Xvfb" :77 -screen 0 1280x900x24 -nolisten tcp >/dev/null 2>&1 & XP=$!
sleep 1
export DISPLAY=:77
cd "$T"
python3 -I "$R/tests/mcp_smoke.py" "$R/tests/out" "Z:$(echo "$R/tests/testapp" | tr / '\\\\')" -- "$WINE" "$EXE"
rc=$?
"$(dirname "$WINE")/wineserver" -k 2>/dev/null; kill $XP 2>/dev/null
exit $rc
