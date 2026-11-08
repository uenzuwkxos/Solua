#!/usr/bin/env bash
# Builds the Linux packages (.deb and .AppImage) into ./dist
# Requirements (once):  cargo install cargo-deb
#   AppImage: appimagetool on PATH, or let this script download it (needs internet).
# Usage: packaging/linux/build.sh [deb] [appimage]     (no argument = both)
set -euo pipefail
cd "$(dirname "$0")/../.."

want=("$@"); [ ${#want[@]} -eq 0 ] && want=(deb appimage)
has() { printf '%s\n' "${want[@]}" | grep -qx "$1"; }

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
mkdir -p dist
cargo build --release

if has deb; then
  cargo deb --no-build --no-strip -o "dist/solua_${VERSION}_amd64.deb"
fi

if has appimage; then
  APPDIR=$(mktemp -d)/Solua.AppDir
  mkdir -p "$APPDIR/usr/bin"
  install -m755 target/release/Solua "$APPDIR/usr/bin/solua"
  install -m644 packaging/linux/solua.desktop "$APPDIR/solua.desktop"
  install -m644 packaging/linux/icons/hicolor/256x256/apps/solua.png "$APPDIR/solua.png"
  cp "$APPDIR/solua.png" "$APPDIR/.DirIcon"
  printf '#!/bin/sh\nexec "$(dirname "$(readlink -f "$0")")/usr/bin/solua" "$@"\n' > "$APPDIR/AppRun"
  chmod +x "$APPDIR/AppRun"
  TOOL=$(command -v appimagetool || true)
  if [ -z "$TOOL" ]; then
    TOOL=dist/appimagetool
    [ -x "$TOOL" ] || { curl -fsSL -o "$TOOL" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage; chmod +x "$TOOL"; }
  fi
  ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "dist/Solua-${VERSION}-x86_64.AppImage" 2>&1 | tail -3 || \
  ARCH=x86_64 "$TOOL" "$APPDIR" "dist/Solua-${VERSION}-x86_64.AppImage"
fi

ls -lh dist
