#!/bin/sh
# Mush8 - install-phone.sh
# Constantin Clerc (c22dev) - 0.1
set -eu
cd "$(dirname "$0")"

DEVICE=""
NSP=0
for a in "$@"; do
  case "$a" in
    --nsp) NSP=1 ;;
    *) DEVICE="$a" ;;
  esac
done
BUNDLE_ID="${BUNDLE_ID:-ch.cclerc.mush8}"
APP="$PWD/target/ios-xcode/Build/Products/Release-iphoneos/Mush8.app"
PREPARED=work/prepared_ios.noindex/prepared
[ -d target/ios-res/modules ] || { echo "build first: ./make-ios.sh (add --romfs for the prepared game data)"; exit 1; }

devices() {
  xcrun devicectl list devices 2>/dev/null | awk '/connected|available \(paired\)/ && /iPhone|iPad/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) print $i}'
}
if [ -z "$DEVICE" ]; then
  echo "waiting for an unlocked iPhone / iPad…"
  while [ -z "$(devices)" ]; do sleep 3; done
  if [ "$(devices | wc -l)" -gt 1 ]; then
    echo "several devices connected, pass the one to use:"
    xcrun devicectl list devices 2>/dev/null | grep -E 'connected|available \(paired\)'
    exit 1
  fi
  DEVICE="$(devices)"
fi
MODEL="$(xcrun devicectl device info details --device "$DEVICE" --json-output - 2>/dev/null | /usr/bin/python3 -I -c '
import json, sys
try: print(json.load(sys.stdin)["result"]["hardwareProperties"]["productType"])
except Exception: pass
')"
echo "device $DEVICE ${MODEL:-unknown model}"
MAJOR="${MODEL#iPhone}"
MAJOR="${MAJOR#iPad}"
MAJOR="${MAJOR%%,*}"
case "$MODEL" in
  iPad13,[4-9]|iPad13,1[0167]|iPad14,[3-689]|iPad14,1[01]|iPad15,[3-6]) BIG=1 ;;
  iPad*,*|iPhone*,*) [ "$MAJOR" -ge 16 ] 2>/dev/null && BIG=1 || BIG=0 ;;
  *) BIG=0 ;;
esac
if [ "$BIG" = 1 ] && [ "$NSP" = 0 ]; then
  echo "8 GB device (iPhone 15 Pro and newer, M-series or A17 Pro iPad): using the .nsp files, not the prepared game data"
  NSP=1
fi

TEAM="${TEAM:-$(defaults export com.apple.dt.Xcode - 2>/dev/null | /usr/bin/python3 -I -c '
import plistlib, sys
try: d = plistlib.loads(sys.stdin.buffer.read())
except Exception: sys.exit()
teams = [t for ts in d.get("IDEProvisioningTeamByIdentifier", {}).values() for t in ts if t.get("teamID")]
teams.sort(key=lambda t: bool(t.get("isFreeProvisioningTeam")))
print(teams[0]["teamID"] if teams else "")
')}"
[ -n "$TEAM" ] || { echo "sign into Xcode (Settings > Accounts) or set TEAM=<your Apple developer team id>"; exit 1; }

echo "signing for this device…"
xcodebuild -quiet -project ios/Mush8.xcodeproj -scheme Mush8 -configuration Release \
  -destination "id=$DEVICE" -derivedDataPath target/ios-xcode -allowProvisioningUpdates -allowProvisioningDeviceRegistration build \
  PRODUCT_BUNDLE_IDENTIFIER="$BUNDLE_ID" CODE_SIGN_ENTITLEMENTS=Mush8.entitlements DEVELOPMENT_TEAM="$TEAM"

echo "installing…"
xcrun devicectl device install app --device "$DEVICE" "$APP"

LIST="$(mktemp)"
xcrun devicectl device info files --device "$DEVICE" --domain-type appDataContainer --domain-identifier "$BUNDLE_ID" --json-output "$LIST" >/dev/null 2>&1 || true
HAVE="$(/usr/bin/python3 -I -c '
import json, sys
try: files = json.load(open(sys.argv[1]))["result"]["files"]
except Exception: files = []
for f in files:
    if not f["resources"]["isDirectory"]: print(f["relativePath"], f["metadata"]["size"])
' "$LIST")"
rm -f "$LIST"
put() {
  [ -e "$1" ] || { echo "  missing here: $1"; return; }
  if printf '%s\n' "$HAVE" | grep -qxF "Documents/$2 $(stat -L -f %z "$1")"; then
    echo "  already there: $2"
  else
    echo "  copying $2 ($(du -h "$1" | cut -f1))"
    xcrun devicectl device copy to --device "$DEVICE" --domain-type appDataContainer --domain-identifier "$BUNDLE_ID" --source "$1" --destination "Documents/$2" >/dev/null
  fi
}

echo "copying game files…"
put prod.keys prod.keys
for f in Firmware/*; do put "$f" "$f"; done
if [ "$NSP" = 0 ] && [ -f "$PREPARED/romfs.bin" ]; then
  put "$PREPARED/main_id" prepared/main_id
  for f in "$PREPARED"/dlc-*.bin; do [ -e "$f" ] && put "$f" "prepared/$(basename "$f")"; done
  put "$PREPARED/romfs.bin" prepared/romfs.bin
else
  UPDATE=update.nsp
  [ -e "$UPDATE" ] || [ ! -e upd4-0-1.nsp ] || UPDATE=upd4-0-1.nsp
  [ -e dlc.nsp ] && put dlc.nsp dlc.nsp
  put "$UPDATE" update.nsp
  put base.nsp base.nsp
fi
echo "done: open Mush8 on the device"
