#!/bin/bash
# Dev-only: run a command with Wine env set up and a hidden Xvfb display (:77).
T="/media/jeremy/1TB SSD/solar2d-mcp-tools"; . "$T/env.sh"
"$T/xvfb/usr/bin/Xvfb" :77 -screen 0 1280x900x24 -nolisten tcp >/dev/null 2>&1 & XP=$!
sleep 1; export DISPLAY=:77; cd "$T"
"$@"; rc=$?
"$(dirname "$WINE")/wineserver" -k 2>/dev/null; kill $XP 2>/dev/null
exit $rc
