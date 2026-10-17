#!/bin/sh
# Verifiable-panel-turns live acceptance on the build host.
# Usage: ./run.sh <host binary or ChromeUse.app> [ONLY=1,4]
# The binary is copied into a fresh ChromeUse.app each run (stage-bin.sh):
# macOS SIGKILLs a binary that was overwritten in place while its old pages
# are cached.
set -e
cd "$(dirname "$0")"
. ./stage-bin.sh
SRC="$1"; shift || true
[ -n "$SRC" ] && [ -e "$SRC" ] || { echo "usage: $0 <chrome-use binary or ChromeUse.app>"; exit 64; }
export PATH="$HOME/.local/bin:/opt/homebrew/bin:$PATH"
export CHROME="${CHROME:-$HOME/.chrome-use/browsers/chrome-155.0.8059.39/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing}"
STAMP="$(date +%m%d%H%M%S)"
# Short: daemon sockets live under $WORK/home/.chrome-use (unix limit 103 bytes).
export WORK="${WORK_ROOT:-/tmp/pa}/$STAMP"
export OUT="${OUT:-$PWD/results/$(date +%Y-%m-%d)}"
# Disk on the build host is tight: keep earlier runs' journals, drop their binary copies.
rm -rf "${WORK_ROOT:-/tmp/pa}"/*/bin/chrome-use-* "${WORK_ROOT:-/tmp/pa}"/*/ChromeUse.app 2>/dev/null || true
# bin/ holds the comparison binaries some checks copy in (check 15).
mkdir -p "$WORK/bin" "$OUT"
stage_host_bin "$SRC" "$WORK"
# Default: this checkout's extension, so acceptance covers the build being released.
export EXT_DIR="${EXT_DIR:-$(cd ../../extensions/ab-connect 2>/dev/null && pwd || echo "$PWD/ext")}"
# The checks read the panel's English text, and macOS ignores --lang: stage a
# copy of the extension with only the English messages (as inspect-acceptance does).
if [ -d "$EXT_DIR/_locales" ]; then
  rm -rf "$WORK/ext"; mkdir -p "$WORK/ext"; cp -R "$EXT_DIR/." "$WORK/ext/"
  find "$WORK/ext/_locales" -mindepth 1 -maxdepth 1 ! -name en -exec rm -rf {} +
  export EXT_DIR="$WORK/ext"
fi
# v1.5.214 release binary and extension for checks 15 and 16 (comparison runs).
export OLD_BIN="${OLD_BIN:-$HOME/panel-accept/old/chrome-use}"
export OLD_EXT_DIR="${OLD_EXT_DIR:-$HOME/panel-accept/old/ext}"
# Real backends for the R runs: Codex with its own login, OpenCode 2.0.25
# with a private data directory (see cli/src/agent_panel/fixtures/README.md).
export REAL_CODEX_HOME="${REAL_CODEX_HOME:-$HOME/.codex}"
export REAL_OPENCODE_BIN="${REAL_OPENCODE_BIN:-/private/tmp/chrome-use-opencode-2.0.25}"
export REAL_OPENCODE_AUTH="${REAL_OPENCODE_AUTH:-$HOME/.local/share/opencode/auth.json}"
# The native host loads a launchd job per profile directory for the panel
# service; they stay loaded after a run. Unload those of acceptance runs only
# (their program lives under /tmp/pa), never the host's other services.
unload_run_jobs() {
  [ "$(uname)" = Darwin ] || return 0
  for label in $(launchctl list | awk '/com\.chrome-use\.panel-service\./ {print $3}'); do
    case "$(launchctl print "gui/$(id -u)/$label" 2>/dev/null | awk '/^\tprogram = / {print $3; exit}')" in
      /tmp/pa/*|/private/tmp/pa/*) launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true ;;
    esac
  done
}
unload_run_jobs
# A run interrupted earlier (dropped SSH) can still hold the fixture ports.
pkill -f "panel-acceptance/site/server.mjs" 2>/dev/null || true
pkill -f "user-data-dir=/tmp/pa/" 2>/dev/null || true
sleep 1
status=0
node accept.mjs "$@" || status=$?
pkill -f "user-data-dir=$WORK/" 2>/dev/null || true
unload_run_jobs
echo "work: $WORK"
exit $status
