#!/bin/sh
# Full-chain native host: the released binary with its own HOME, and the
# user's real ocs through CHAIN_OCS (a wrapper that runs ocs with the real HOME).
DIR="$(cd "$(dirname "$0")" && pwd)"
export HOME="$DIR/home"
mkdir -p "$HOME"
export CHROME_USE_INSPECT_OCS="$CHAIN_OCS"
exec "$DIR/chrome-use" __nm-host "$@"
