#!/bin/sh
# Matching acceptance on the build host: fixtures, dev servers, browser
# scenarios, host checks, then the new extension against an older host.
#   ./run-matching.sh [OLD_HOST=/path/to/v1.5.213/chrome-use]
set -e
cd "$(dirname "$0")"
# A run interrupted earlier (dropped SSH) can still hold the fixture ports.
pkill -f "node accept.mjs|node accept-matching.mjs|node hostcheck.mjs" 2>/dev/null || true
pkill -f "user-data-dir=.*inspect-accept-" 2>/dev/null || true
rm -f fake-roster.json
sleep 1
export PATH="$HOME/.local/bin:/opt/homebrew/bin:$PATH"
export CHROME="$HOME/.chrome-use/browsers/chrome-155.0.8059.39/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
sh setup-matching.sh
rm -rf ext-acc home && cp -R ext ext-acc && mkdir -p home
python3 - <<'PY'
import json
p='ext-acc/manifest.json'; m=json.load(open(p))
m['host_permissions']=['<all_urls>']
json.dump(m,open(p,'w'),indent=2)
PY
pkill -f 'user-data-dir=.*inspect-accept-' 2>/dev/null || true
pkill -f "vite --port 5173" 2>/dev/null || true
(cd react-app && npx vite --port 5173 --strictPort >/tmp/inspect-vite-react.log 2>&1 &)
for i in $(seq 1 60); do curl -sf http://localhost:5173/ >/dev/null && break; sleep 0.5; done
status=0
node accept-matching.mjs || status=1
pkill -f 'user-data-dir=.*inspect-accept-' 2>/dev/null || true
node hostcheck.mjs || status=1
if [ -n "${OLD_HOST:-}" ]; then
  cur=$(readlink bin/current)
  cp "$OLD_HOST" "bin/chrome-use-old-$$" && ln -sfn "chrome-use-old-$$" bin/current
  rm -rf home && mkdir -p home
  LEGACY_HOST=1 node accept-matching.mjs || status=1
  ln -sfn "$cur" bin/current && rm -f "bin/chrome-use-old-$$"
fi
pkill -f "vite --port 5173" 2>/dev/null || true
pkill -f 'user-data-dir=.*inspect-accept-' 2>/dev/null || true
rm -f fake-roster.json
exit $status
