#!/bin/sh
# Run hook acceptance tests without host mounts, credentials, or network access.
# Uses an already available official Node image; never pulls or installs tools.
set -eu
cd "$(dirname "$0")/.."

image=$(docker image inspect node:24-bookworm --format '{{.Id}}')
name="chrome-use-hook-test-$$"
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Only these two files enter the container. In particular, neither .git nor
# any host directory, socket, environment, or credential is shared with it.
tar -cf - .husky/pre-push scripts/pre-push.test.js |
  docker run --rm -i --pull never --name "$name" \
    --network none --user 65534:65534 --read-only \
    --cap-drop ALL --security-opt no-new-privileges \
    --pids-limit 64 --memory 256m --memory-swap 256m --cpus 1 \
    --tmpfs /tmp:rw,exec,nosuid,nodev,size=64m,mode=1777 \
    --entrypoint /usr/bin/env "$image" \
    -i PATH=/usr/local/bin:/usr/bin:/bin TMPDIR=/tmp \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    /bin/sh -c 'umask 077; tar -xf - -C /tmp && cd /tmp && timeout 60 node --test scripts/pre-push.test.js'
