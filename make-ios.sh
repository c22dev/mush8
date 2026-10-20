#!/bin/sh
# Mush8 - make-ios.sh
# Constantin Clerc (c22dev) - 0.1
set -eu
cd "$(dirname "$0")"

MODE=""
DEVICE=""
SHADERS=0
for a in "$@"; do
  case "$a" in
    --install|--ipa) MODE="$a" ;;
    --shaders) SHADERS=1 ;;
    *) DEVICE="$a" ;;
  esac
done
ENTITLEMENTS=Mush8.entitlements
if [ "${FREE:-0}" = 1 ] || [ "$MODE" = "--ipa" ]; then
  ENTITLEMENTS=Mush8-sideload.entitlements
fi
TEAM="${TEAM:-$(defaults export com.apple.dt.Xcode - 2>/dev/null | /usr/bin/python3 -I -c '
import plistlib, sys
try: d = plistlib.loads(sys.stdin.buffer.read())
except Exception: sys.exit()
teams = [t for ts in d.get("IDEProvisioningTeamByIdentifier", {}).values() for t in ts if t.get("teamID")]
teams.sort(key=lambda t: bool(t.get("isFreeProvisioningTeam")))
print(teams[0]["teamID"] if teams else "")
')}"
[ -n "$TEAM" ] || [ "$MODE" = "--ipa" ] || { echo "sign into Xcode (Settings > Accounts) or set TEAM=<your Apple developer team id>"; exit 1; }
BUNDLE_ID="${BUNDLE_ID:-ch.cclerc.mush8}"
TOOLCHAIN=1.98.1
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
PATCHED="$PWD/work/v-upd/patched"
CACHE=work/shader_cache_ios.noindex
RES=target/ios-res
# Opt-in profile: keep all executable adaptations baked before signing.
if [ "${FIRMWARE_FREE:-0}" = 1 ]; then
  export NX_MISSING_MII=1
  export NX_FALLBACK_FONT="${NX_FALLBACK_FONT:-$PWD/work/host-font.ttf}"
  [ -f "$NX_FALLBACK_FONT" ] || { echo "prepare a local font first: see ADAPTATIONS.md"; exit 1; }
fi

cargo build --release
[ -f work/v-upd/exefs/main ] || ./target/release/nxfs extract --base base.nsp --update upd4-0-1.nsp --out work/v-upd
[ "$(cat "$PATCHED/nxpatch.stamp" 2>/dev/null)" = "$(./target/release/nxpatch stamp)" ] || ./target/release/nxpatch rewrite --out "$PATCHED" \
  work/v-upd/exefs/main work/v-upd/exefs/subsdk0 work/v-upd/exefs/sdk

mkdir -p "$CACHE"
LIST="$CACHE/pipelines.all.tsv"
cat data/pipelines.tsv work/pipelines.tsv 2>/dev/null | awk 'NF && !seen[$0]++' > "$LIST.new"
STAMP="$(shasum "$LIST.new" | cut -d' ' -f1)"
mv "$LIST.new" "$LIST"

if [ "$SHADERS" = 1 ] || [ ! -f "$CACHE/index.tsv" ] || [ "$(cat "$CACHE/pipelines.stamp" 2>/dev/null)" != "$STAMP" ]; then
  echo "precompiling shaders for iOS (log: work/precompile_ios.log)…"
  NX_SHADER_SDK=iphoneos NX_SHADER_CACHE="$CACHE" ./target/release/nxhost --precompile > work/precompile_ios.log 2>&1 \
    || { tail -20 work/precompile_ios.log; exit 1; }
  tail -3 work/precompile_ios.log
  echo "$STAMP" > "$CACHE/pipelines.stamp"
fi

NX_MODULES_DIR="$PATCHED" IPHONEOS_DEPLOYMENT_TARGET=26.0 \
  cargo "+$TOOLCHAIN" build --release -p nxhost --target aarch64-apple-ios --target-dir target/ios

rm -rf "$RES"
mkdir -p "$RES/modules" "$RES/shaders" "$RES/fallback"
if [ -f "${NX_FALLBACK_FONT:-work/host-font.ttf}" ]; then
  cp "${NX_FALLBACK_FONT:-work/host-font.ttf}" "$RES/fallback/host-font.ttf"
fi
cp "$PATCHED"/*.layout work/v-upd/exefs/main.npdm "$RES/modules/"
cp -c "$CACHE"/*.metallib "$CACHE"/index.tsv "$CACHE"/guest.tsv "$RES/shaders/"
cp "$LIST" "$RES/shaders/pipelines.tsv"

(cd ios && xcodegen generate --quiet)
rm -rf target/ios-xcode/Build/Products
APP="$PWD/target/ios-xcode/Build/Products/Release-iphoneos/Mush8.app"
if [ "$MODE" = "--ipa" ]; then
  xcodebuild -quiet -project ios/Mush8.xcodeproj -scheme Mush8 -configuration Release \
    -destination 'generic/platform=iOS' -derivedDataPath target/ios-xcode build \
    PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_ID" CODE_SIGNING_ALLOWED=NO
  rm -f "$APP/embedded.mobileprovision"
  codesign --force --sign - --entitlements ios/Mush8-sideload.entitlements --generate-entitlement-der "$APP"
else
  xcodebuild -quiet -project ios/Mush8.xcodeproj -scheme Mush8 -configuration Release \
    -destination 'generic/platform=iOS' -derivedDataPath target/ios-xcode -allowProvisioningUpdates build \
    PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_ID" CODE_SIGN_ENTITLEMENTS="$ENTITLEMENTS" DEVELOPMENT_TEAM="$TEAM"
fi
codesign --verify "$APP"
echo "built: $APP ($(du -sh "$APP" | cut -f1))"

if [ "$MODE" = "--ipa" ]; then
  STAGE="$(mktemp -d)"
  mkdir "$STAGE/Payload"
  cp -cRp "$APP" "$STAGE/Payload/"
  mkdir -p build
  IPA="$PWD/build/Mush8.ipa"
  rm -f "$IPA"
  (cd "$STAGE" && zip -qry "$IPA" Payload)
  rm -rf "$STAGE"
  echo "built: $IPA ($(du -h "$IPA" | cut -f1))"
fi

if [ "$MODE" = "--install" ]; then
  if [ -z "$DEVICE" ]; then
    DEVICE="$(xcrun devicectl list devices 2>/dev/null | awk '/available \(paired\)|connected/ && /iPhone|iPad/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) {print $i; exit}}')"
  fi
  [ -n "$DEVICE" ] || { echo "no iPhone / iPad connected (unlocked, Developer Mode on)"; exit 1; }
  xcrun devicectl device install app --device "$DEVICE" "$APP"
  echo "installed on $DEVICE: copy prod.keys, base.nsp, upd4-0-1.nsp (dlc.nsp, Firmware/) into the app's Files folder"
fi
