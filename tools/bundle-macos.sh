#!/usr/bin/env bash
# Packs the two macOS builds into a universal Feather.app and zips it.
#   tools/bundle-macos.sh <version>
# Expects release binaries for aarch64-apple-darwin and x86_64-apple-darwin.
set -euo pipefail

version="$1"
app="Feather.app"

rm -rf "$app" icon.iconset
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

lipo -create -output "$app/Contents/MacOS/feather" \
  target/aarch64-apple-darwin/release/feather \
  target/x86_64-apple-darwin/release/feather

mkdir icon.iconset
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/icon-1024.png --out "icon.iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" assets/icon-1024.png --out "icon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns icon.iconset -o "$app/Contents/Resources/icon.icns"
rm -rf icon.iconset

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Feather</string>
  <key>CFBundleDisplayName</key><string>Feather</string>
  <key>CFBundleIdentifier</key><string>dev.lh3k.feather</string>
  <key>CFBundleExecutable</key><string>feather</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature: required for the arm64 half to run at all. It is not a
# Developer ID signature, so Gatekeeper still asks on first launch.
codesign --force --deep --sign - "$app"

ditto -c -k --sequesterRsrc --keepParent "$app" feather-macos-universal.zip
echo "Built feather-macos-universal.zip"
