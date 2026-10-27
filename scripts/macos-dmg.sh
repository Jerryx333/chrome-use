#!/bin/sh
# Put ChromeUse.app in a disk image, optionally signed.
#
#   scripts/macos-dmg.sh <ChromeUse.app> <out.dmg> [--identity ID] [--keychain KC]
#
# The image is what the extension's setup guide downloads for a user who has
# the extension but no CLI yet. Opened from Finder, ChromeUse.app installs
# itself (cli/src/gui_setup.rs): it copies the bundle where install.sh puts
# it, links `chrome-use`, registers the native-messaging host and says so in
# a dialog. No terminal, no admin password.
#
# Signed with the Developer ID Application identity the bundle uses; the
# release workflow then notarizes and staples the image. Without an identity
# the image is unsigned, and Gatekeeper refuses the app inside it on a
# double-click (a fork's build).
set -eu

usage() { echo "usage: $0 <ChromeUse.app> <out.dmg> [--identity ID] [--keychain KC]" >&2; exit 64; }
[ $# -ge 2 ] || usage
app=$1 out=$2
shift 2
identity="" keychain=""
while [ $# -gt 0 ]; do
  case "$1" in
    --identity) identity=${2:?}; shift 2 ;;
    --keychain) keychain=${2:?}; shift 2 ;;
    *) usage ;;
  esac
done
[ -x "$app/Contents/MacOS/chrome-use" ] || { echo "no ChromeUse.app at $app" >&2; exit 66; }

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
# ditto keeps the signature and the stapled ticket intact.
ditto "$app" "$stage/ChromeUse.app"
rm -f "$out"
hdiutil create -volname "chrome-use" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$out" >/dev/null

if [ -n "$identity" ]; then
  set -- --force --timestamp -s "$identity"
  [ -z "$keychain" ] || set -- "$@" --keychain "$keychain"
  codesign "$@" "$out"
  codesign --verify --verbose=2 "$out"
fi
echo "$out"
