#!/bin/sh
# Run inspect-pick-live.mjs on the build host against a branch build:
#   [THEME=dark] [LOCALE=en] ./inspect-pick-live.sh <chrome-use binary> [extension git ref, default HEAD]
# Copies the binary into a fresh ChromeUse.app (stage-bin.sh; macOS kills a
# binary overwritten in place), stages the extension from git with host_permissions in place of the
# activeTab grant a real shortcut gives, and runs the check. One browser run
# per host at a time.
set -eu
cd "$(dirname "$0")"
BIN=${1:?usage: $0 <chrome-use binary> [extension ref]}; REF=${2:-HEAD}
export PATH="$HOME/.local/bin:/opt/homebrew/bin:$PATH"
export CHROME="${CHROME:-$HOME/.chrome-use/browsers/chrome-155.0.8059.39/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing}"
export WORK="$(mktemp -d /tmp/ip-XXXXXX)"; export OUT="${OUT:-$WORK/out}"
mkdir -p "$WORK/bin" "$WORK/ext"
. ./stage-bin.sh
stage_host_bin "$BIN" "$WORK"
"$HOST_BIN" --version
ROOT=$(git rev-parse --show-toplevel)
git -C "$ROOT" archive "$REF" extensions/ab-connect | tar -x -C "$WORK/ext"
export EXT_DIR="$WORK/ext/extensions/ab-connect"
rm -f "$EXT_DIR"/*.test.js
python3 - "$EXT_DIR/manifest.json" <<'PY'
import json, sys
p = sys.argv[1]; m = json.load(open(p))
m['host_permissions'] = ['<all_urls>']  # stands in for the activeTab grant a real shortcut gives
json.dump(m, open(p, 'w'), indent=2)
PY
grep '"version"' "$EXT_DIR/manifest.json"
# LOCALE=en: drop the other locales so the UI is English whatever the host's language.
if [ "${LOCALE:-}" = en ]; then find "$EXT_DIR/_locales" -mindepth 1 -maxdepth 1 ! -name en -exec rm -rf {} +; fi
node inspect-pick-live.mjs
