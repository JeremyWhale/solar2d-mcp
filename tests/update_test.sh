#!/bin/bash
# Dev-only: end-to-end test of --update under Wine against a local HTTP server.
# Installs a throwaway "v0.1.9" build and tries good and hostile updates to the current build.
set -u
T="${SOLAR2D_MCP_TOOLS:-/media/jeremy/1TB SSD/solar2d-mcp-tools}"; . "$T/env.sh"
R="$(cd "$(dirname "$0")/.." && pwd)"
KEY="${SOLAR2D_MCP_SIGNING_KEY:-$HOME/.config/solar2d-mcp/release-signing.pem}"
W="$T/update-test"; OLD="$W/old-0.1.9.exe"; NEW="$R/target/x86_64-pc-windows-gnu/release/solar2d-mcp.exe"
CUR=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$R/Cargo.toml" | head -1)
mkdir -p "$W"
export WINEDEBUG=-all; unset DISPLAY
if [ ! -f "$OLD" ] || [ "$R/src/update.rs" -nt "$OLD" ]; then
  echo "building the throwaway v0.1.9..."
  rm -rf "$W/src"; mkdir -p "$W/src"; (cd "$R" && git ls-files -co --exclude-standard -z | grep -zvE '^tests/' | xargs -0 cp --parents -t "$W/src")
  sed -i 's/^version = ".*"$/version = "0.1.9"/' "$W/src/Cargo.toml"
  (cd "$W/src" && CARGO_TARGET_DIR="$W/target" cargo zigbuild --release --target x86_64-pc-windows-gnu 2>&1 | tail -1)
  cp "$W/target/x86_64-pc-windows-gnu/release/solar2d-mcp.exe" "$OLD"
fi

SITE="$W/site"; INSTALL="$W/install"; PORT=18765; fails=0
export SOLAR2D_MCP_UPDATE_URL="http://127.0.0.1:$PORT"
rm -rf "$SITE"; mkdir -p "$SITE"
python3 -I -m http.server "$PORT" --bind 127.0.0.1 --directory "$SITE" >/dev/null 2>&1 & HP=$!
trap 'kill $HP 2>/dev/null; "$(dirname "$WINE")/wineserver" -k 2>/dev/null' EXIT
sleep 1

publish() { # version exe-to-serve signing-key [hash-of-this-file-instead]
  cp "$2" "$SITE/solar2d-mcp.exe"
  local f="${4:-$2}"
  printf 'solar2d-mcp update manifest 1\nversion: %s\nsha256: %s\nsize: %s\n' "$1" "$(sha256sum "$f" | cut -d' ' -f1)" "$(stat -c %s "$f")" > "$SITE/update-manifest.txt"
  openssl pkeyutl -sign -inkey "$3" -rawin -in "$SITE/update-manifest.txt" -out "$SITE/update-manifest.txt.sig"
}
fresh()   { rm -rf "$INSTALL"; mkdir -p "$INSTALL"; cp "$OLD" "$INSTALL/solar2d-mcp.exe"; }
run()     { "$WINE" "$INSTALL/solar2d-mcp.exe" "$@" 2>&1 | tr -d '\r' | grep -v '^$'; }
version() { run --version | tail -1; }
check()   { if [ "$1" = ok ]; then echo "PASS $2"; else echo "FAIL $2"; fails=$((fails+1)); fi; }
expect()  { case "$2" in *"$1"*) echo ok;; *) echo "no [$1] in: $2" >&2; echo no;; esac; }
untouched() { [ "$(sha256sum < "$INSTALL/solar2d-mcp.exe")" = "$(sha256sum < "$OLD")" ] && [ "$(ls "$INSTALL" | wc -l)" = 1 ] && echo ok || echo no; }

openssl genpkey -algorithm ed25519 -out "$W/forged.pem" 2>/dev/null

fresh; publish "$CUR" "$NEW" "$KEY"
out=$(run --check-update); check "$(expect "v$CUR is available" "$out")" "--check-update finds v$CUR"

publish "$CUR" "$NEW" "$W/forged.pem"; out=$(run --update)
check "$(expect "signature is not valid" "$out")" "forged signature refused"; check "$(untouched)" "  ...install untouched"

publish "$CUR" "$NEW" "$KEY"; printf 'x' | dd of="$SITE/update-manifest.txt" bs=1 seek=45 conv=notrunc 2>/dev/null; out=$(run --update)
check "$(expect "signature is not valid" "$out")" "edited manifest refused"; check "$(untouched)" "  ...install untouched"

cp "$NEW" "$W/tampered.exe"; printf 'X' | dd of="$W/tampered.exe" bs=1 seek=100000 conv=notrunc 2>/dev/null
publish "$CUR" "$W/tampered.exe" "$KEY" "$NEW"; out=$(run --update)
check "$(expect "doesn't match the signed checksum" "$out")" "tampered exe refused"; check "$(untouched)" "  ...install untouched"

head -c 500000 "$NEW" > "$W/short.exe"
publish "$CUR" "$W/short.exe" "$KEY" "$NEW"; out=$(run --update)
check "$(expect "wrong size" "$out")" "truncated download refused"; check "$(untouched)" "  ...install untouched"

publish "0.1.0" "$NEW" "$KEY"; out=$(run --update)
check "$(expect "Already up to date" "$out")" "older version is not installed (no downgrade)"; check "$(untouched)" "  ...install untouched"

publish "9.0.0" "$NEW" "$KEY"; out=$(run --update)
check "$(expect "didn't start correctly" "$out")" "exe that isn't the promised version refused"; check "$(untouched)" "  ...install untouched"

export SOLAR2D_MCP_UPDATE_URL="http://127.0.0.1:1"; out=$(run --update); rc=${PIPESTATUS[0]}
check "$(expect "Update failed" "$out")" "offline: clear error"; check "$(untouched)" "  ...install untouched"
export SOLAR2D_MCP_UPDATE_URL="http://127.0.0.1:$PORT"

publish "$CUR" "$NEW" "$KEY"; out=$(run --update)
check "$(expect "Updated to v$CUR" "$out")" "good update installs"
check "$(expect "solar2d-mcp $CUR" "$(version)")" "  ...now reports v$CUR"
[ "$(sha256sum < "$INSTALL/solar2d-mcp.exe")" = "$(sha256sum < "$NEW")" ] && check ok "  ...exe is byte-identical to the release" || check no "  ...exe is byte-identical to the release"
out=$(run --update); check "$(expect "Already up to date" "$out")" "  ...a second --update does nothing"

fresh
INSTALL="$INSTALL" CUR="$CUR" python3 -I "$R/tests/update_notice_check.py"; [ $? = 0 ] || fails=$((fails+1))

echo; [ $fails = 0 ] && echo "RESULT: ALL PASSED" || echo "RESULT: $fails FAILED"
exit $fails
