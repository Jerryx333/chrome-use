#!/bin/sh
# Render the Chrome Web Store listing images into extensions/store/.
#
# The side panel and settings page are the real extension files, run against
# the fake host in extensions/store/screenshots/mock.js, so the images follow
# the UI. Rerun after a visible UI change:
#
#   sh scripts/store-screenshots.sh
set -eu
cd "$(dirname "$0")/.."
CHROME=${CHROME:-"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"}
OUT=extensions/store
STAGE=$(mktemp -d)
PROFILE=$(mktemp -d)
trap 'kill "$SERVER" 2>/dev/null; rm -rf "$STAGE" "$PROFILE"' EXIT

cp -R extensions/ab-connect "$STAGE/ab-connect"
mkdir -p "$STAGE/store"
cp -R extensions/store/screenshots "$STAGE/store/screenshots"
# Load the fake host before the page's own script.
for page in sidepanel.html options.html; do
  perl -0pi -e 's#(<script[^>]*src="(sidepanel|options)\.js")#<script src="../store/screenshots/mock.js"></script>\n$1#' "$STAGE/ab-connect/$page"
done

PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$STAGE" >/dev/null 2>&1 &
SERVER=$!
sleep 1
BASE="http://127.0.0.1:$PORT"

enc() { python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }

# shot NAME WIDTH HEIGHT DARK PAGE QUERY
shot() {
  scheme=""
  [ "$4" = dark ] && scheme="--blink-settings=preferredColorScheme=0"
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 $scheme \
    --user-data-dir="$PROFILE" --window-size="$2,$3" --virtual-time-budget=6000 \
    --screenshot="$PWD/$OUT/$1.png" "$BASE/store/screenshots/$5?$6" >/dev/null 2>&1
  echo "wrote $OUT/$1.png"
}

# compose NAME DARK HEADLINE SUB BULLETS PANEL_QUERY [PAGE]
compose() {
  page=${7-"$BASE/store/screenshots/site.html"}
  query="h=$(enc "$3")&sub=$(enc "$4")&b=$(enc "$5")&panel=$(enc "$BASE/ab-connect/sidepanel.html?$6")"
  [ -n "$page" ] && query="$query&page=$(enc "$page")"
  [ "$2" = dark ] && query="$query&dark=1"
  shot "$1" 1280 800 "$2" compose.html "$query"
}

compose screenshot-1-chat light \
  "Your local agent, in the browser you already use" \
  "Ask Codex, Claude Code, OpenCode or Pi to work on the page in front of you." \
  "Uses your existing CLI login|Every step is listed as it runs|Answers stay next to the page" \
  "s=idle"

compose screenshot-2-agents light \
  "Pick the agent and model per task" \
  "Agents are found on this computer. Change models and reasoning effort without editing config files." \
  "See which CLIs are signed in|Preview each config change first|Restore the previous config any time" \
  "s=idle&agents=1&config=1&plan=1"

compose screenshot-3-approve dark \
  "You stay in control" \
  "The agent stops and asks before it runs a command or writes a file." \
  "Allow once, or for this conversation|Conversations kept per site|History stays on this computer" \
  "s=approve"

# The settings page fills the window on its own.
query="h=$(enc "Local by design")&sub=$(enc "The extension talks only to the chrome-use CLI on this computer, through Chrome native messaging.")&b=$(enc "No account, no cloud relay|No tracking or analytics|Open source, Apache-2.0")&page=$(enc "$BASE/ab-connect/options.html")&tab=$(enc "chrome-use settings")&url=$(enc "chrome-extension://chrome-use/options.html")"
shot screenshot-4-settings 1280 800 light compose.html "$query"

shot promo-440x280 440 280 light promo.html "size=small"
shot marquee-1400x560 1400 560 light promo.html "size=marquee&panel=$(enc "$BASE/ab-connect/sidepanel.html?s=idle")"
