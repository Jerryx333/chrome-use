#!/bin/sh
# Run live.mjs on the build host against a branch build:
#   ./run.sh <chrome-use binary> [released chrome-use binary to compare]
# Stages the binaries as ChromeUse.app copies (stage-bin.sh) and this
# checkout's extension. One browser run per host at a time.
set -eu
cd "$(dirname "$0")"
BIN=${1:?usage: $0 <chrome-use binary> [base binary]}; BASE=${2:-}
export PATH="$HOME/.local/bin:/opt/homebrew/bin:$PATH"
export CHROME="${CHROME:-$HOME/.chrome-use/browsers/chrome-155.0.8059.39/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing}"
export WORK="$(mktemp -d /tmp/aml-XXXXXX)"
cd ../panel-acceptance && . ./stage-bin.sh && cd - >/dev/null
stage_host_bin "$BIN" "$WORK"
if [ -n "$BASE" ]; then
  mkdir -p "$WORK/base"
  cp "$BASE" "$WORK/base/chrome-use" && chmod 755 "$WORK/base/chrome-use"
  export BASE_BIN="$WORK/base/chrome-use"
fi
mkdir -p "$WORK/ext" && cp -R ../../extensions/ab-connect/. "$WORK/ext/"
rm -f "$WORK"/ext/*.test.js
export EXT_DIR="$WORK/ext"
node live.mjs
