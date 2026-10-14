#!/bin/sh
# Acceptance native host: the branch binary with an isolated HOME so relay and
# inspect files never touch the build host's real chrome-use state.
DIR="$(cd "$(dirname "$0")" && pwd)"
export HOME="$DIR/home"
mkdir -p "$HOME"
exec "$DIR/bin/chrome-use" __nm-host "$@"
