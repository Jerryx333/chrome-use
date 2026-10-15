#!/bin/sh
# Run site-project-live.mjs on the build host against a released CLI:
#   ./site-project-live.sh v1.5.220 [extension git ref, default origin/main]
# Downloads the release's darwin-arm64 binary, checks its sha256, stages the
# extension from git, and runs the check. One browser run per host at a time.
set -eu
cd "$(dirname "$0")"
TAG=${1:?usage: $0 <release tag> [extension ref]}; REF=${2:-origin/main}
export PATH="$HOME/.local/bin:/opt/homebrew/bin:$PATH"
export CHROME="${CHROME:-$HOME/.chrome-use/browsers/chrome-155.0.8059.39/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing}"
export WORK="$(mktemp -d /tmp/sp-XXXXXX)"; export OUT="${OUT:-$WORK/out}"
ASSET=chrome-use-darwin-arm64.tar.gz
mkdir -p "$WORK/release" "$WORK/bin" "$WORK/ext"
gh release download "$TAG" -R leeguooooo/chrome-use -p "$ASSET" -p "$ASSET.sha256" -D "$WORK/release"
( cd "$WORK/release" && shasum -a 256 -c "$ASSET.sha256" )
tar -xzf "$WORK/release/$ASSET" -C "$WORK/bin"
export HOST_BIN="$(find "$WORK/bin" -type f -name chrome-use -perm -u+x | head -1)"
"$HOST_BIN" --version
ROOT=$(git rev-parse --show-toplevel)
git -C "$ROOT" fetch -q origin
git -C "$ROOT" archive "$REF" extensions/ab-connect | tar -x -C "$WORK/ext"
export EXT_DIR="$WORK/ext/extensions/ab-connect"
rm -f "$EXT_DIR"/*.test.js
grep '"version"' "$EXT_DIR/manifest.json"
node site-project-live.mjs
