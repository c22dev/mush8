#!/bin/sh
# Mush8 - make-app.sh
# Constantin Clerc (c22dev) - 0.1
set -eu
cd "$(dirname "$0")"
APP="$PWD/build/Mush8.app"

cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/nxhost target/release/nxfs target/release/nxpatch "$APP/Contents/MacOS/"
swiftc -O -parse-as-library -target arm64-apple-macos14 launcher/Launcher.swift launcher/ModDiscovery.swift launcher/ModContents.swift launcher/GameBanana.swift launcher/ModInstaller.swift launcher/ModBrowser.swift \
  -import-objc-header launcher/Archive.h -larchive -o "$APP/Contents/MacOS/Launcher"
xcrun actool AppIcon.icon --compile "$APP/Contents/Resources" --app-icon AppIcon --platform macosx \
  --minimum-deployment-target 14.0 --output-partial-info-plist "$(mktemp -d)/icon.plist" >/dev/null

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Mush8</string>
  <key>CFBundleDisplayName</key><string>Mush8</string>
  <key>CFBundleIdentifier</key><string>ch.cclerc.mush8</string>
  <key>CFBundleExecutable</key><string>Launcher</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIconName</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.games</string>
  <key>GCSupportsGameMode</key><true/>
  <key>LSSupportsGameMode</key><true/>
  <key>NSLocalNetworkUsageDescription</key><string>Mush8 finds other players, and iPhones used as controllers, on your local network.</string>
  <key>NSBonjourServices</key>
  <array><string>_mush8._tcp</string><string>_mush8-mp._udp</string><string>_mush8-pad._udp</string></array>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"
echo "built: $APP ($(du -sh "$APP" | cut -f1))"
