#!/usr/bin/env bash
# 打 macOS 通用二进制的 .app 与 .dmg（在 macOS 上运行）。
# 用法：scripts/package-macos.sh <version> <输出目录>
# 前置：已构建 target/{aarch64,x86_64}-apple-darwin/release/apkinfoplus。
set -euo pipefail

VERSION="$1"
OUT="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
APP="$WORK/APK Info Plus.app"

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$OUT"
lipo -create \
  "$ROOT/target/aarch64-apple-darwin/release/apkinfoplus" \
  "$ROOT/target/x86_64-apple-darwin/release/apkinfoplus" \
  -output "$APP/Contents/MacOS/apkinfoplus"
sed "s/@VERSION@/$VERSION/g" "$ROOT/packaging/macos/Info.plist" > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# 图标：源图 256px，iconset 的更大档位由 sips 放大补齐。
ICONSET="$WORK/AppIcon.iconset"
mkdir -p "$ICONSET"
SRC="$ROOT/crates/app/assets/icon.png"
for s in 16 32 128 256 512; do
  sips -z $s $s "$SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

# 未签名的 ad-hoc 签名：Apple Silicon 上没有签名的二进制直接拒绝运行。
codesign --force --deep --sign - "$APP"

DMG_SRC="$WORK/dmg"
mkdir -p "$DMG_SRC"
cp -R "$APP" "$DMG_SRC/"
ln -s /Applications "$DMG_SRC/Applications"
hdiutil create -volname "APK Info Plus $VERSION" -srcfolder "$DMG_SRC" -ov -format UDZO \
  "$OUT/ApkInfoPlus-$VERSION-macos-universal.dmg"
rm -rf "$WORK"
