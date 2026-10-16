#!/bin/sh
# Inspect & Ask acceptance runner (build host). Usage: ./run.sh [ONLY=a,b]
set -e
cd "$(dirname "$0")"
# A run interrupted earlier (dropped SSH) can still hold the fixture ports.
pkill -f "node accept.mjs|node accept-matching.mjs|node hostcheck.mjs" 2>/dev/null || true
pkill -f "user-data-dir=.*inspect-accept-" 2>/dev/null || true
rm -f fake-roster.json
sleep 1
export PATH="$HOME/.local/bin:/opt/homebrew/bin:$PATH"
export CHROME="$HOME/.chrome-use/browsers/chrome-155.0.8059.39/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
rm -rf ext-acc home && cp -R ext ext-acc && mkdir -p home
python3 - <<'PY'
import json
p='ext-acc/manifest.json'; m=json.load(open(p))
m['host_permissions']=['<all_urls>']  # stands in for the activeTab grant a real shortcut gives
json.dump(m,open(p,'w'),indent=2)
PY
# Scenarios match English button text; without other locales the UI is
# English whatever the build host's system language.
find ext-acc/_locales -mindepth 1 -maxdepth 1 ! -name en -exec rm -rf {} +
(cd react-app && npx vite --port 5173 --strictPort >/tmp/inspect-vite-react.log 2>&1 &) 
(cd vue-app && npx vite --port 5174 --strictPort >/tmp/inspect-vite-vue.log 2>&1 &)
for p in 5173 5174; do for i in $(seq 1 60); do curl -sf "http://localhost:$p/" >/dev/null && break; sleep 0.5; done; done
pkill -f 'user-data-dir=.*inspect-accept-' 2>/dev/null || true
status=0
node accept.mjs || status=$?
pkill -f "vite --port 5173" || true
pkill -f 'user-data-dir=.*inspect-accept-' 2>/dev/null || true
pkill -f "vite --port 5174" || true
exit $status
