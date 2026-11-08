#!/usr/bin/env bash
# Builds "Peel Studio.app" (and a .dmg) in dist/.
#
#   scripts/package-macos.sh            # native architecture
#   SIGN_ID="Developer ID Application: …" scripts/package-macos.sh
#
# Without SIGN_ID the bundle is ad-hoc signed, which runs locally but is not
# notarized for distribution.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
APP="dist/Peel Studio.app"
cargo build --release -p peel-app

rm -rf "$APP" dist/iconset
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" dist/iconset
cp target/release/peel-studio "$APP/Contents/MacOS/peel-studio"

# Icon set from the generated PNGs.
cp assets/icons/peel-16.png dist/iconset/icon_16x16.png
cp assets/icons/peel-32.png dist/iconset/icon_16x16@2x.png
cp assets/icons/peel-32.png dist/iconset/icon_32x32.png
cp assets/icons/peel-64.png dist/iconset/icon_32x32@2x.png
cp assets/icons/peel-128.png dist/iconset/icon_128x128.png
cp assets/icons/peel-256.png dist/iconset/icon_128x128@2x.png
cp assets/icons/peel-256.png dist/iconset/icon_256x256.png
cp assets/icons/peel-512.png dist/iconset/icon_256x256@2x.png
cp assets/icons/peel-512.png dist/iconset/icon_512x512.png
cp assets/icons/peel-1024.png dist/iconset/icon_512x512@2x.png
mv dist/iconset dist/Peel.iconset
iconutil -c icns dist/Peel.iconset -o "$APP/Contents/Resources/Peel.icns"
rm -rf dist/Peel.iconset

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Peel Studio</string>
  <key>CFBundleDisplayName</key><string>Peel Studio</string>
  <key>CFBundleIdentifier</key><string>io.github.vheissu.peelstudio</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>peel-studio</string>
  <key>CFBundleIconFile</key><string>Peel</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Peel Studio records audio from the input you choose.</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Peel Studio Project</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>CFBundleTypeIconFile</key><string>Peel</string>
      <key>LSHandlerRank</key><string>Owner</string>
      <key>CFBundleTypeExtensions</key><array><string>peelproj</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Standard MIDI File</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>CFBundleTypeExtensions</key><array><string>mid</string><string>midi</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

codesign --force --options runtime --timestamp=none -s "${SIGN_ID:--}" "$APP"
codesign --verify --verbose=1 "$APP"

DMG="dist/Peel-Studio-${VERSION}-macos-$(uname -m).dmg"
rm -f "$DMG"
hdiutil create -quiet -volname "Peel Studio" -srcfolder "$APP" -ov -format UDZO "$DMG"
echo "Built $APP and $DMG"
