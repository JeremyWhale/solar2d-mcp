#!/bin/bash
# Build, test, sign and (with --publish) release the version in Cargo.toml.
#   tools/release.sh            build + sign into target/dist, publish nothing
#   tools/release.sh --publish  also run all the tests, tag, push and create the GitHub release
# Needs .github/release-notes/v<version>.md and the private signing key.
set -euo pipefail
cd "$(dirname "$0")/.."
KEY="${SOLAR2D_MCP_SIGNING_KEY:-$HOME/.config/solar2d-mcp/release-signing.pem}"
V=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1); TAG="v$V"
NOTES=".github/release-notes/$TAG.md"; D=target/dist; ZIP="solar2d-mcp-$TAG-windows-x64.zip"

[ -f "$KEY" ] || { echo "signing key not found: $KEY"; exit 1; }
[ -f "$NOTES" ] || { echo "missing $NOTES"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "commit your changes first"; exit 1; }
! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || { echo "$TAG already exists"; exit 1; }

# the key must be the one the exe trusts, or nobody could ever install this release
PUB=$(openssl pkey -in "$KEY" -pubout -outform DER | tail -c 32 | xxd -p -c 32)
EMBEDDED=$(sed -n '/const PUBLIC_KEY/,/];/p' src/update.rs | grep -o '0x[0-9a-f][0-9a-f]' | sed 's/0x//' | tr -d '\n')
[ "$PUB" = "$EMBEDDED" ] || { echo "signing key doesn't match PUBLIC_KEY in src/update.rs"; exit 1; }

touch src/main.rs; tools/build-windows.sh
rm -rf "$D"; mkdir -p "$D"; cp target/x86_64-pc-windows-gnu/release/solar2d-mcp.exe "$D/"
python3 -I - "$D/$ZIP" "$D/solar2d-mcp.exe" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_DEFLATED) as z:
    z.write(sys.argv[2], "solar2d-mcp.exe"); z.write("README.md"); z.write("LICENSE")
PY
printf 'solar2d-mcp update manifest 1\nversion: %s\nsha256: %s\nsize: %s\n' "$V" \
  "$(sha256sum "$D/solar2d-mcp.exe" | cut -d' ' -f1)" "$(stat -c %s "$D/solar2d-mcp.exe")" > "$D/update-manifest.txt"
openssl pkeyutl -sign -inkey "$KEY" -rawin -in "$D/update-manifest.txt" -out "$D/update-manifest.txt.sig"
openssl pkey -in "$KEY" -pubout -out "$D/.pub.pem"
openssl pkeyutl -verify -pubin -inkey "$D/.pub.pem" -rawin -in "$D/update-manifest.txt" -sigfile "$D/update-manifest.txt.sig"; rm "$D/.pub.pem"
(cd "$D" && sha256sum solar2d-mcp.exe "$ZIP" update-manifest.txt update-manifest.txt.sig > SHA256SUMS.txt)
ls -la "$D"
[ "${1:-}" = "--publish" ] || { echo "Built $TAG in $D (not published)."; exit 0; }

tests/run_smoke_wine.sh 2>&1 | grep -E '^(FAIL|RESULT)' | tee /tmp/solar2d-release-tests.txt
tests/with_wine_display.sh python3 -I "$PWD/tests/widget_smoke.py" 2>&1 | grep -E 'FAIL|RESULT' | tee -a /tmp/solar2d-release-tests.txt
tests/update_test.sh 2>&1 | grep -E '^(FAIL|RESULT)' | tee -a /tmp/solar2d-release-tests.txt
[ "$(grep -c 'RESULT: ALL PASSED' /tmp/solar2d-release-tests.txt)" = 3 ] || { echo "tests failed, not publishing"; exit 1; }

git tag -a "$TAG" -m "solar2d-mcp $TAG"
git push origin HEAD "$TAG"
gh release create "$TAG" "$D/solar2d-mcp.exe" "$D/$ZIP" "$D/update-manifest.txt" "$D/update-manifest.txt.sig" "$D/SHA256SUMS.txt" \
  --title "solar2d-mcp $TAG" --notes-file "$NOTES"
