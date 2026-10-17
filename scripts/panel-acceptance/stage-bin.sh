# Sourced by the *-live.sh scripts and run.sh:
#   stage_host_bin <chrome-use binary or ChromeUse.app> <work dir>
# Sets and exports HOST_BIN.
#
# Every run gets its own copy: macOS kills a binary overwritten in place
# while its old pages are cached. On macOS the copy is a ChromeUse.app with
# one stable signature, because privacy settings (Files and Folders) add an
# entry per bare binary path, so each run used to leave one more anonymous
# row on the build host. A bundle is one entry, keyed by its identifier,
# wherever it is copied, as long as its signing identity stays the same.
#
# The identity: SIGN_IDENTITY, else the first Developer ID Application one in
# SIGN_KEYCHAIN (default ~/Library/Keychains/cbbuild.keychain-db, which the
# caller unlocks; over SSH a locked keychain fails the signature). Without
# one the bundle is ad-hoc signed and a note says so.
stage_host_bin() {
  _src=$1 _work=$2
  # Callers run from scripts/panel-acceptance.
  _tools=$(cd .. && pwd)
  if [ "$(uname)" != Darwin ]; then
    mkdir -p "$_work/bin"
    cp "$_src" "$_work/bin/chrome-use"
    chmod 755 "$_work/bin/chrome-use"
    HOST_BIN="$_work/bin/chrome-use"
  elif [ -d "$_src" ]; then
    ditto "$_src" "$_work/ChromeUse.app"
    HOST_BIN="$_work/ChromeUse.app/Contents/MacOS/chrome-use"
  else
    _ver=$("$_src" --version | awk 'NR == 1 {print $2}')
    _kc=${SIGN_KEYCHAIN:-$HOME/Library/Keychains/cbbuild.keychain-db}
    _id=${SIGN_IDENTITY:-}
    if [ -z "$_id" ] && [ -f "$_kc" ]; then
      _id=$(security find-identity -v -p codesigning "$_kc" 2>/dev/null \
        | sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p' | head -1)
    fi
    if [ -z "$_id" ] || ! sh "$_tools/macos-app-bundle.sh" "$_src" "$_work" \
      --version "$_ver" --identity "$_id" --keychain "$_kc" >/dev/null 2>&1; then
      echo "note: ChromeUse.app is ad-hoc signed (no usable identity in $_kc; is it unlocked?)" >&2
      sh "$_tools/macos-app-bundle.sh" "$_src" "$_work" --version "$_ver" >/dev/null
    fi
    HOST_BIN="$_work/ChromeUse.app/Contents/MacOS/chrome-use"
  fi
  export HOST_BIN
}
