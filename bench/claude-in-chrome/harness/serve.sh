#!/bin/sh
# Serve the fixtures: the main pages on 127.0.0.1:8711 and the coupon widget
# on localhost:8712, so the iframe in frame.html is cross-origin.
# Run on the machine where the browser runs. Ctrl-C stops both.
set -eu
here=$(cd "$(dirname "$0")/../fixtures" && pwd)
python3 -m http.server 8711 --bind 127.0.0.1 --directory "$here/a" &
a=$!
python3 -m http.server 8712 --bind 127.0.0.1 --directory "$here/b" &
b=$!
trap 'kill $a $b 2>/dev/null' INT TERM EXIT
wait
