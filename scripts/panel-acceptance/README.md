# Verifiable panel turns: live acceptance

Live checks 1-20 of `docs/superpowers/plans/2026-10-15-verifiable-panel-turns.md`
section 14.2, plus each installed real backend through scenarios 1, 4 and 12.
Run on the build host only (never the maintainer's machine). Every expectation is
an assert; `results/<date>/results.json` keeps the evidence per check and
`MANIFEST.md` the summary.

## What runs

- `site/server.mjs`: the fixture site (a form whose save the server rejects, a
  paginated list, same-name, slow, aborting, empty and HTML-as-PDF downloads, a
  password form, a cookie page with `?access_token=` links, a redirect, a popup
  link, a toast page and a MutationObserver page that reports DOM changes). Every
  request is logged, so a check can prove a tool never reached the site.
- `backends/fixture-backend.mjs`: one fixture agent per protocol (Codex
  app-server, Claude stream-json, Pi JSONL RPC, OpenCode ACP), launched through
  the conversation `executable` override. No model runs: each turn follows the
  scenario its message names (`run scenario <name>`). Browser tools are called on
  the real `chrome-use mcp` started from the config the host wrote, so evidence
  flows through the real MCP, daemon, extension relay and Chrome. Event shapes
  follow `cli/src/agent_panel/fixtures/*-tool-marker.jsonl`.
- `rig.mjs`: Chrome for Testing (headless, its own `--user-data-dir`) with the
  unpacked extension, the native host with an isolated `HOME`, and the panel
  service for the extension's profile, started by the driver. A transparent
  proxy (`lib.mjs` `Proxy`) sits in front of the service: `service.json` is
  rewritten to point at it, so the native host, the conversations' MCP servers
  and the driver all go through it. It can hold a conversation's next
  `agentActionBegin` (check 20) or drop a `conversationSend` reply after the
  service stored the message (check 14).
- `accept.mjs` + `checks.mjs`: the driver. It talks to the service over its
  socket (never a native-host `hello`, which would take over the relay) and
  asserts on `conversationRead` and on the conversation folder. The side panel
  is the real one: a trusted CDP click on an extension page calls
  `chrome.sidePanel.open`, and screenshots come from the side-panel target.
  The CDP traffic of the relay is recorded by wrapping
  `chrome.debugger.sendCommand` inside the extension worker; web pages are never
  attached by the driver.

## Run

```sh
rsync -a --exclude results scripts/panel-acceptance/ scripts/inspect-acceptance/lib.mjs <host>:...   # see run.sh paths
rsync -a --exclude '*.test.js' extensions/ab-connect/ <host>:.../panel-acceptance/ext/
python3 scripts/remote-cargo.py --output <local path> build --bin chrome-use   # then scp it to the host
ssh <host> 'cd .../panel-acceptance && BUILD_COMMIT=<sha> ONLY=1,4 ./run.sh <chrome-use binary>'
node manifest.mjs results/<date>
```

`run.sh` copies the binary under a fresh name each run (macOS kills a binary
overwritten in place) and keeps the work directory short (`/tmp/pa/<stamp>`;
daemon sockets must fit the 103-byte Unix limit). Long runs go under `nohup`
with a done-file; SSH drops.

Runs are strictly one at a time on a host: `run.sh` stops the fixture site,
every `/tmp/pa` Chrome and the launchd jobs of earlier runs when it starts,
so a second run started meanwhile breaks the first.

The extension's native host can start the profile's panel service (through
launchd on macOS) before the driver does. `rig.mjs` stops such a service and
retries until the driver's own process owns the profile; before that, a lost
race made whole runs fail together (checks 5, 7, 15 and 18).

Inputs beyond the branch binary: the v1.5.214 release binary (`OLD_BIN`,
checksum-verified from the GitHub release) and the v1.5.214 extension
(`OLD_EXT_DIR`, `git archive v1.5.214 extensions/ab-connect`) for checks 15 and
16; real Codex uses its own login (`REAL_CODEX_HOME`); real OpenCode 2.0.25
(`REAL_OPENCODE_BIN`) uses a private data directory holding a copy of
`auth.json`, as `cli/src/agent_panel/fixtures/README.md` describes.

## Tab modes

`tab-modes-live.sh <chrome-use binary> [extension ref]` runs `tab-modes-live.mjs`:
the panel opened as a tab next to the real side panel on one conversation,
lending a user tab to the Codex fixture agent (a call without a tab handle and
one naming `chrome-tab:<id>` both run in it), taking it back from the side
panel (the next call is refused), and the grant following the tab across
origins until the tab closes.

## Limits

- Check 5's fault switch (skip the kill on stop timeout) does not exist in the
  product, so only the no-switch half runs.
- Check 14 drops the reply on the service socket, not inside the extension
  bridge.
- Check 17's "ffmpeg without libvpx" is a stand-in `ffmpeg` that refuses the
  VP8 encoder, put first on the MCP server's `PATH`.
- Claude Code and Pi are not installed on the build host; their protocols run
  only as fixture backends.
