#!/bin/sh
# Exercise install.sh's binary placement (bare binary or ChromeUse.app plus a
# symlink) against fixture archives, without network or a real bundle.
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
sed -n '/^# --- install the binary/,/^# --- guided setup:/p' "$repo/install.sh" > "$work/place.sh"
grep -q 'ChromeUse.app' "$work/place.sh" || { echo "placement section not found"; exit 1; }

# One install: $1 platform, $2 "bundle" when the archive carries one.
install() {
  rm -rf "$work/tmp" && mkdir -p "$work/tmp"
  printf '#!/bin/sh\necho bare-%s\n' "$3" > "$work/tmp/chrome-use"
  chmod +x "$work/tmp/chrome-use"
  if [ "$2" = bundle ]; then
    mkdir -p "$work/tmp/ChromeUse.app/Contents/MacOS"
    printf '#!/bin/sh\necho bundle-%s\n' "$3" > "$work/tmp/ChromeUse.app/Contents/MacOS/chrome-use"
    chmod +x "$work/tmp/ChromeUse.app/Contents/MacOS/chrome-use"
  fi
  (
    BIN_NAME=chrome-use plat=$1 tmp="$work/tmp" bindir="$work/prefix/bin"
    info() { :; }
    LSREGISTER=true
    mkdir -p "$bindir"
    . "$work/place.sh"
  )
}
cli="$work/prefix/bin/chrome-use"
app="$work/prefix/share/chrome-use/ChromeUse.app"
expect() { [ "$("$cli")" = "$1" ] || { echo "expected $1, ran $("$cli")"; exit 1; }; }

install darwin bundle 1
[ -L "$cli" ] && expect bundle-1 && [ -x "$app/Contents/MacOS/chrome-use" ]
printf 'pass: bundle installed behind a symlink\n'

install darwin bundle 2
expect bundle-2
[ -z "$(ls -d "$app".* 2>/dev/null)" ] || { echo "left a previous bundle behind"; exit 1; }
printf 'pass: bundle replaced whole\n'

install darwin bare 3
[ ! -L "$cli" ] && expect bare-3 && [ ! -e "$app" ]
printf 'pass: archive without a bundle replaces the symlink and removes the bundle\n'

install darwin bundle 4
AGENT_BROWSER_NO_APP_BUNDLE=1 install darwin bundle 5
[ ! -L "$cli" ] && expect bare-5 && [ ! -e "$app" ]
printf 'pass: opt-out installs the bare binary\n'

rm -rf "$work/prefix"
install linux bundle 6
[ ! -L "$cli" ] && expect bare-6 && [ ! -e "$work/prefix/share" ]
printf 'pass: Linux is untouched\n'
