#!/bin/sh
# Wrap a macOS chrome-use binary in ChromeUse.app, optionally signed.
#
#   scripts/macos-app-bundle.sh <chrome-use binary> <out dir> [options]
#     --identity ID     codesign identity (omit: ad-hoc signature)
#     --keychain KC     keychain holding the identity
#     --version X.Y.Z   bundle version (default: cli/Cargo.toml)
#
# Writes <out dir>/ChromeUse.app; Contents/MacOS/chrome-use is the binary.
#
# Why a bundle: macOS privacy settings (Files and Folders) key a bare
# executable by its path and show it with no name or icon, so every copy or
# build of the binary that touches a protected folder becomes another
# anonymous row. A process inside a signed bundle is keyed by the bundle
# identifier and shown as "ChromeUse" with its icon, however many copies
# exist. That matters for the processes macOS holds responsible for their
# own file access: the panel service launchd starts, the agents it runs,
# and the native-messaging host.
#
# The bundle is signed as a whole (the main executable is signed by the
# bundle signature; `--deep` is neither needed nor wanted) with the
# hardened runtime that notarization requires. The release workflow and the
# acceptance harness both use this script.
set -eu

usage() { echo "usage: $0 <chrome-use binary> <out dir> [--identity ID] [--keychain KC] [--version X.Y.Z]" >&2; exit 64; }
[ $# -ge 2 ] || usage
bin=$1 out=$2
shift 2
identity="" keychain="" version=""
while [ $# -gt 0 ]; do
  case "$1" in
    --identity) identity=${2:?}; shift 2 ;;
    --keychain) keychain=${2:?}; shift 2 ;;
    --version) version=${2:?}; shift 2 ;;
    *) usage ;;
  esac
done
[ -f "$bin" ] || { echo "no binary at $bin" >&2; exit 66; }

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ -z "$version" ]; then
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/cli/Cargo.toml" | head -1)
fi
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) : ;;
  *) echo "bad bundle version: $version" >&2; exit 65 ;;
esac

app="$out/ChromeUse.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/chrome-use"
chmod 755 "$app/Contents/MacOS/chrome-use"

# The icon: the extension's, which is the largest source there is (128 px).
# Privacy settings draw it at 16-32 pt, so the upscaled 256 px set is only a
# fallback for Finder.
icons=$(mktemp -d)
trap 'rm -rf "$icons"' EXIT
set_dir="$icons/ChromeUse.iconset"
mkdir "$set_dir"
src="$repo/extensions/ab-connect/icons/icon128.png"
for spec in 16:icon_16x16 32:icon_16x16@2x 32:icon_32x32 64:icon_32x32@2x 128:icon_128x128 256:icon_128x128@2x; do
  size=${spec%%:*} name=${spec#*:}
  sips -z "$size" "$size" "$src" --out "$set_dir/$name.png" >/dev/null
done
iconutil -c icns "$set_dir" -o "$app/Contents/Resources/ChromeUse.icns"

# LSUIElement keeps it out of the Dock and the app switcher. Not
# LSBackgroundOnly: the folder picker the service opens must still come to
# the front. The usage strings are what the privacy prompts show.
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key><string>en</string>
	<key>CFBundleExecutable</key><string>chrome-use</string>
	<key>CFBundleIdentifier</key><string>com.leeguoo.chrome-use</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key><string>chrome-use</string>
	<key>CFBundleDisplayName</key><string>ChromeUse</string>
	<key>CFBundleIconFile</key><string>ChromeUse</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>$version</string>
	<key>CFBundleVersion</key><string>$version</string>
	<key>LSMinimumSystemVersion</key><string>11.0</string>
	<key>LSUIElement</key><true/>
	<key>NSHumanReadableCopyright</key><string>Apache-2.0</string>
	<key>NSDesktopFolderUsageDescription</key><string>Agents you run from the chrome-use panel work in the project folder you choose.</string>
	<key>NSDocumentsFolderUsageDescription</key><string>Agents you run from the chrome-use panel work in the project folder you choose.</string>
	<key>NSDownloadsFolderUsageDescription</key><string>chrome-use saves downloads where you or your agent asked.</string>
	<key>NSRemovableVolumesUsageDescription</key><string>Agents you run from the chrome-use panel work in the project folder you choose.</string>
	<key>NSNetworkVolumesUsageDescription</key><string>Agents you run from the chrome-use panel work in the project folder you choose.</string>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null
printf 'APPL????' > "$app/Contents/PkgInfo"

if [ -n "$identity" ]; then
  set -- --force --options runtime --timestamp -s "$identity"
  [ -z "$keychain" ] || set -- "$@" --keychain "$keychain"
  codesign "$@" "$app"
else
  # Ad-hoc: runnable, but its requirement is the code hash, so privacy
  # settings treat every build as a new app.
  codesign --force -s - "$app"
fi
codesign --verify --strict --verbose=2 "$app"
echo "$app"
