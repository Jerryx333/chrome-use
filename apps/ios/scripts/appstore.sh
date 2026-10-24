#!/usr/bin/env bash
# Archive, export and optionally upload the native iOS companion.
# Usage: BUILD_NUMBER=2 bash scripts/appstore.sh [--no-upload]
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ASC_KEY_ID:?}" "${ASC_ISSUER_ID:?}" "${ASC_KEY_PATH:?}" "${BUILD_NUMBER:?}"
OUT="${RELEASE_OUT:-$(mktemp -d "${TMPDIR:-/tmp}/chrome-ios-release.XXXXXX")}"
AUTH=(-allowProvisioningUpdates -authenticationKeyPath "$ASC_KEY_PATH"
      -authenticationKeyID "$ASC_KEY_ID" -authenticationKeyIssuerID "$ASC_ISSUER_ID")
mkdir -p "$OUT"
xcodegen generate --quiet
xcodebuild -project ChromeUse.xcodeproj -scheme ChromeUse -configuration Release \
  -destination 'generic/platform=iOS' -jobs 2 -archivePath "$OUT/ChromeUse.xcarchive" \
  CURRENT_PROJECT_VERSION="$BUILD_NUMBER" "${AUTH[@]}" archive > "$OUT/archive.log" 2>&1
cat > "$OUT/ExportOptions.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>app-store-connect</string>
  <key>destination</key><string>export</string>
  <key>signingStyle</key><string>automatic</string>
  <key>teamID</key><string>6ZPXG4KVVS</string>
  <key>manageAppVersionAndBuildNumber</key><false/>
</dict></plist>
PLIST
xcodebuild -exportArchive -archivePath "$OUT/ChromeUse.xcarchive" \
  -exportOptionsPlist "$OUT/ExportOptions.plist" -exportPath "$OUT" "${AUTH[@]}" > "$OUT/export.log" 2>&1
if [[ "${1:-}" == "--no-upload" ]]; then
  echo "exported chrome-use Remote build $BUILD_NUMBER: $OUT"
  exit 0
fi
plutil -replace destination -string upload "$OUT/ExportOptions.plist"
xcodebuild -exportArchive -archivePath "$OUT/ChromeUse.xcarchive" \
  -exportOptionsPlist "$OUT/ExportOptions.plist" -exportPath "$OUT" "${AUTH[@]}" > "$OUT/upload.log" 2>&1
echo "uploaded chrome-use Remote build $BUILD_NUMBER: $OUT"
