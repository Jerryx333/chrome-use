#!/bin/sh
# Verifiable-panel-turns live acceptance on the build host.
# Usage: ./run.sh <host binary> [ONLY=1,4]
# The binary is copied under a fresh name each run: macOS SIGKILLs a binary
# that was overwritten in place while its old pages are cached.
set -e
cd "$(dirname "$0")"
SRC="$1"; shift || true
[ -n "$SRC" ] && [ -x "$SRC" ] || { echo "usage: $0 <chrome-use binary>"; exit 64; }
export PATH="$HOME/.local/bin:/opt/homebrew/bin:$PATH"
export CHROME="${CHROME:-$HOME/.chrome-use/browsers/chrome-155.0.8059.39/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing}"
STAMP="$(date +%m%d%H%M%S)"
# Short: daemon sockets live under $WORK/home/.chrome-use (unix limit 103 bytes).
export WORK="${WORK_ROOT:-/tmp/pa}/$STAMP"
export OUT="${OUT:-$PWD/results/$(date +%Y-%m-%d)}"
# Disk on the build host is tight: keep earlier runs' journals, drop their binary copies.
rm -f "${WORK_ROOT:-/tmp/pa}"/*/bin/chrome-use-* 2>/dev/null || true
mkdir -p "$WORK/bin" "$OUT"
export HOST_BIN="$WORK/bin/chrome-use-$STAMP"
cp "$SRC" "$HOST_BIN"
chmod 755 "$HOST_BIN"
export EXT_DIR="${EXT_DIR:-$PWD/ext}"
# v1.5.214 release binary and extension for checks 15 and 16 (comparison runs).
export OLD_BIN="${OLD_BIN:-$HOME/panel-accept/old/chrome-use}"
export OLD_EXT_DIR="${OLD_EXT_DIR:-$HOME/panel-accept/old/ext}"
# Real backends for the R runs: Codex with its own login, OpenCode 2.0.25
# with a private data directory (see cli/src/agent_panel/fixtures/README.md).
export REAL_CODEX_HOME="${REAL_CODEX_HOME:-$HOME/.codex}"
export REAL_OPENCODE_BIN="${REAL_OPENCODE_BIN:-/private/tmp/chrome-use-opencode-2.0.25}"
export REAL_OPENCODE_AUTH="${REAL_OPENCODE_AUTH:-$HOME/.local/share/opencode/auth.json}"
# A run interrupted earlier (dropped SSH) can still hold the fixture ports.
pkill -f "panel-acceptance/site/server.mjs" 2>/dev/null || true
pkill -f "user-data-dir=/tmp/pa/" 2>/dev/null || true
sleep 1
status=0
node accept.mjs "$@" || status=$?
pkill -f "user-data-dir=$WORK/" 2>/dev/null || true
echo "work: $WORK"
exit $status
