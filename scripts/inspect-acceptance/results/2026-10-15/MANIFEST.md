# Inspect & Ask matching acceptance, 2026-10-15

| Item | Value |
|---|---|
| Source | feat/inspect-matching; host code at 24664e07 (no Rust change after it), extension at the release commit 7fd967d4 |
| Host binary | macOS arm64, sha256 `36c434343796a15ca36a13cd2160ea0ceedb30f62b83c4a02d10fe75a892acdf` (remote-cargo build on the build host) |
| Extension package | `extensions/ab-connect.zip` 0.5.57, sha256 `784476d1456fd96398d459cf9689e67716f7b4bba32f5780ef3c740ceeb32877` |
| Browser | Chrome for Testing 155.0.8059.39 on the build host |
| Rust | inspect tests 59, full `test --bin chrome-use` 2102 passed / 0 failed; fmt clean; clippy no new warnings; Windows CI green |
| Extension unit tests | 410 passed |

Files:

- `results.json`: the earlier 14 Inspect & Ask scenarios, all passed with the new host and extension.
- `results-matching.json`: 16 matching scenarios, all passed (local dev, port reused by another worktree, test domain config, two apps on one origin, worktrees ranked by revision, two sessions, session restarted / moved, inspect link, claim conflict, tab-only target, monorepo app root with forged marks, wildcard config, SPA route change, target changed after the panel showed it, new extension on a v1.5.213 host).
- `results-hostcheck.json`: 8 host-level checks passed (legacy callers on scoped origins, v1 migration, claims and stale decisions, broader claim listing narrower scopes, config validation, unlink identity, forged tab target, two configs needing --root).
- `results-legacy-host.json`: the new extension against the released v1.5.213 host falls back to the earlier flow.
- `headed-results.json`: unmodified manifest (activeTab, no host permissions), real Alt+Shift+C keystroke via peekaboo, picker opened, screenshot through the activeTab grant.
- `stealth-v0557-zip.json`, `timing-v0557-zip.json`: the 0.5.57 zip (key re-added for the stable id); worker and relay report 0.5.57; static-id probes indistinguishable from a non-existent extension; relay stealth bench passed. Limited observations on CfT 155, not a guarantee.
- `real-ocs-link.json`: `inspect link` run inside a live Claude Code session with the real ocs, then a delivery quoting the bound fingerprint; the session received it; same id again returned duplicate.

Fake ocs and host_permissions are used in the headless scenarios; real ocs and the real shortcut are covered by the last two items separately, not in one combined run.
