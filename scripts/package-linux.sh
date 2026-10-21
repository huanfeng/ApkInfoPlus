#!/usr/bin/env bash
# 打 Linux 的 AppImage 与 tar.gz（x86_64）。
# 用法：scripts/package-linux.sh <version> <输出目录>
# 前置：已构建 target/release/apkinfoplus；appimagetool 在 PATH 里（或由 APPIMAGETOOL 指定）。
set -euo pipefail

VERSION="$1"
OUT="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/apkinfoplus"
WORK="$(mktemp -d)"
APPDIR="$WORK/AppDir"
mkdir -p "$OUT"

mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/256x256/apps"
cp "$BIN" "$APPDIR/usr/bin/apkinfoplus"
cp "$ROOT/packaging/linux/apkinfoplus.desktop" "$APPDIR/usr/share/applications/"
cp "$ROOT/packaging/linux/apkinfoplus.desktop" "$APPDIR/"
cp "$ROOT/crates/app/assets/icon.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/apkinfoplus.png"
cp "$ROOT/crates/app/assets/icon.png" "$APPDIR/apkinfoplus.png"
ln -s usr/bin/apkinfoplus "$APPDIR/AppRun"
ARCH=x86_64 "${APPIMAGETOOL:-appimagetool}" "$APPDIR" "$OUT/ApkInfoPlus-$VERSION-linux-x86_64.AppImage"

TAR_DIR="$WORK/ApkInfoPlus-$VERSION-linux-x86_64"
mkdir -p "$TAR_DIR"
cp "$BIN" "$ROOT/LICENSE" "$ROOT/README.md" "$TAR_DIR/"
cp "$ROOT/packaging/linux/apkinfoplus.desktop" "$ROOT/crates/app/assets/icon.png" "$TAR_DIR/"
tar -C "$WORK" -czf "$OUT/ApkInfoPlus-$VERSION-linux-x86_64.tar.gz" "$(basename "$TAR_DIR")"
rm -rf "$WORK"
