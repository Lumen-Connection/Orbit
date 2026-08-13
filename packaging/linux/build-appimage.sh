#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
EXE="${1:-$ROOT/target/release/orbit}"
OUT="${2:-$ROOT/Orbit-x86_64.AppImage}"
APPDIR="$(mktemp -d)"
trap 'rm -rf "$APPDIR"' EXIT

mkdir -p "$APPDIR/usr/bin" \
  "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/icons/hicolor/256x256/apps"
cp "$EXE" "$APPDIR/usr/bin/orbit"
chmod +x "$APPDIR/usr/bin/orbit"
cp "$ROOT/packaging/linux/orbit.desktop" "$APPDIR/usr/share/applications/orbit.desktop"
cp "$ROOT/assets/icon.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/orbit.png"
cp "$ROOT/packaging/linux/orbit.desktop" "$APPDIR/orbit.desktop"
cp "$ROOT/assets/icon.png" "$APPDIR/orbit.png"

TOOL="$ROOT/packaging/linux/appimagetool"
if [[ ! -x "$TOOL" ]]; then
  curl -L --fail -o "$TOOL" \
    https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
  chmod +x "$TOOL"
fi
ARCH=x86_64 "$TOOL" "$APPDIR" "$OUT"
echo "wrote $OUT"
