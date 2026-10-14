# Inspect & Ask: matching a page to a local project and session (single release)

Status: plan for review. One release delivers everything below; implementation
order inside it follows dependencies only.

## Goal

Few actions, no misdelivery. A page (local dev server or a test domain) maps to
the right local project root and agent session after at most one explicit
confirmation per scope, and with zero actions afterwards while the target is
unchanged. Test domains are the main case.

## Principles

- Only locally verifiable facts can make a target: user confirmation in the
  extension frame, a session's own `inspect link`, a trusted local
  `inspect.json`, and dev-server port ownership on loopback. Page data (CI source
  paths, revision meta) only ranks and annotates.
- No new page-visible identity: no repo, session or path meta is added to pages.
- A persisted default is written only from a trusted, visibility-checked click
  in the frame or from `inspect link`; Send never doubles as first authorization.
- Different git worktrees are different projects. A monorepo app keeps its
  subdirectory as the root.

## Concepts

- **Scope**: `origin` plus optional `pathScope` (path segments, default `/`). A
  page matches the binding whose scope has the same origin and the longest
  `pathScope` that is a segment prefix of the page path (`/admin` matches
  `/admin` and `/admin/orders`, not `/administrator`).
- **Target**: `{root, sessionId, sessionCwd}`. `root` is canonical: a worktree
  toplevel or an app directory inside it.
- **Related**: a session is related to a root when both resolve to the same
  worktree toplevel (`git rev-parse --show-toplevel`) and one contains the other.
  Sessions in another worktree of the same repo are never related.

## Data (host, `~/.chrome-use/inspect/bindings.json`, version 2)

```json
{"version": 2,
 "bindings": [{"origin": "https://test-admin.x.com", "pathScope": "/", "root": "/w/admin",
   "sessionId": "claude-…", "sessionCwd": "/w/admin", "sessionLabel": "admin-main",
   "source": "confirmed|linked", "evidence": {"kind": "port", "port": 5173, "cwd": "/w/admin"},
   "consents": {"plugin": false, "testEnv": false}, "updatedAt": 0}],
 "pendingClaims": [{"origin": "…", "pathScope": "/", "root": "…", "sessionId": "…",
   "sessionCwd": "…", "requestedAt": 0}]}
```

Version 1 files migrate on first write: each entry becomes `pathScope: "/"`,
`source: "confirmed"`.

## Project configuration (`.chrome-use/inspect.json`, committed by the team)

```json
{"version": 1, "sites": [
  {"origins": ["https://test-admin.x.com", "http://localhost:5173"], "pathScope": "/"},
  {"origins": ["https://*.preview.x.com"], "pathScope": "/admin"}]}
```

- The file's directory's parent is the app root (`apps/admin/.chrome-use/inspect.json`
  → root `apps/admin`).
- Read only inside known worktrees (toplevels of ocs session cwds and of
  existing binding roots) via `git ls-files '*.chrome-use/inspect.json'`, at most
  50 files, 3 s timeout. Never a filesystem-wide search.
- Origins are exact; `https://*.domain` matches exactly one extra DNS label
  (decision point for the user, see end).
- A match proposes and preselects; it never authorizes on its own.

## Resolution (`inspectResolve`, read-only)

Input: `origin`, `pagePath` (no query/fragment), `sourcePaths`, `revision?`,
`port?` (loopback only). Output: `{state, binding?, card?, candidates[], pendingClaim?}`.

1. **Bound scope**: longest-matching binding. Check the bound session is in the
   roster with the same canonical cwd; for bindings with port evidence on
   loopback, check the port is still owned by a process whose cwd is the root.
   All hold: `state: "bound"`. Otherwise `state: "needsConfirm"` with the reason
   (`sessionEnded`, `sessionMoved`, `portMoved`) and a card.
2. **Pending claim** for this scope: `state: "claimPending"` with old and new
   targets; the frame shows it before anything else.
3. **Unbound**: build candidate roots from config matches (`config`), loopback
   port owner cwd (`dev-server-port`, keeps app subdirectories), previous
   binding roots, and ocs session cwds (worktree toplevel). Rank by evidence
   (config > port > previous > session), then CI source-path suffix matches,
   then revision relation. Pair each root with its related live sessions.
4. **Preselect** the top pair only when the root has config or port evidence and
   exactly one related live session. Otherwise no preselection; the card lists
   root × session pairs.

Revision relation per candidate: page revision must match `^[0-9a-f]{7,40}$`;
`git cat-file -e <rev>^{commit}` then `git rev-list --left-right --count <rev>...HEAD`,
3 s timeout → `same | ahead | behind | diverged | unknown`. Shown and used for
ranking only.

## Confirmation card (frame)

One card, one decision: "Send requests from `<scope>` to `<session>` (`<kind>`,
`<status>`) in `<root>`", with evidence chips (config, dev server, CI paths
matched N, deployed `abc123`: local ahead 3 / behind / diverged / unknown).

- **Hand over**: persist as the scope default (`source: confirmed`).
- **Only this tab**: a target held by this picker instance in the background,
  never persisted; reload or a new picker ends it. It overrides the scope
  default for that tab only.
- **Choose another**: expand the root × session list.

All three are trusted, visibility-checked clicks (the existing gate). Send only
sends. Restart cases (`sessionEnded`, `sessionMoved`, `portMoved`) show the
previous target and preselect only under rule 4.

## Session-side linking (CLI)

`chrome-use inspect link <url> [--root <dir>] [--path-scope </seg>]`,
`chrome-use inspect unlink <url>`, `chrome-use inspect list`.

- Identity: `ocs whoami --json` gives the id; the cwd comes from the matching
  `ocs who --json` entry. No roster entry or a non-absolute cwd: refuse.
- Root: `--root`, else the app root of a config in the session's worktree that
  matches the URL, else the session cwd.
- Scope unbound or bound to the same session: write it (`source: linked`).
  Bound to another session: write a pending claim and print that the browser
  panel will ask; no `--force`.
- Skill reference tells agents to run it when the user says to hand a site to
  them.

## Host operations

| op | change |
|---|---|
| `inspectResolve` | new, above |
| `inspectStatus` | kept for 0.5.56 extensions (unchanged behaviour) |
| `inspectBind` | adds `pathScope`, `source: confirmed`, records evidence; clears consents when root or session changes (as now) |
| `inspectClaimDecide` | `{origin, pathScope, accept}`; accept replaces the binding, reject drops the claim |
| `inspectDeliver` | adds `pagePath` for scope matching and optional `target {root, sessionId}` for a tab target; the host re-validates any target (live, related, canonical root) and re-runs the port check |
| capabilities | `"inspectResolve": true`; a new extension on an older host falls back to `inspectStatus` |

Source paths resolve inside the root first, then inside its worktree toplevel
(monorepo builds that record repo-relative paths), never outside the worktree.

## Extension

- Frame: card above; pending-claim banner (Replace / Keep); scope shown in the
  target line ("→ admin-main @ apps/admin, test-admin.x.com/").
- Background: per-instance tab target; sends `pagePath`.
- Options: scopes with source (confirmed / linked), pending claims, unlink.

## Acceptance (build host, fake ocs with configurable roster and worktrees)

1. Local dev, unique session: preselected card, one click; next pick zero clicks.
2. Port reuse: dev server restarted from another worktree on the same port →
   `portMoved` card; nothing delivered to the old root.
3. Test domain with `inspect.json`: preselected card, one click; then zero.
4. Same origin, two apps (`/admin`, `/shop` path scopes): each page maps to its
   own root and session; `/administrator` matches neither.
5. Two worktrees, one session each, CI paths match both: revision relation ranks
   the one containing the deployed commit; no preselection across worktrees.
6. Two sessions in one worktree: no preselection; choice remembered.
7. Session restart (new id): `sessionEnded` card, one click.
8. Session cwd moved: `sessionMoved` card.
9. `inspect link` from a session on an unbound scope: zero clicks in the browser.
10. `inspect link` conflict: pending claim shown; Replace swaps, Keep retains;
    nothing reaches the claimant before Replace.
11. Tab target: tab A on session X while the default is Y; tab B still Y;
    reload ends A's override.
12. Monorepo: dev server in `apps/admin`, session at repo root → root
    `apps/admin`; repo-relative and absolute CI paths resolve.
13. Forged `data-insp-path` and revision cannot change a bound target or
    preselect without config/port evidence.
14. New extension on a host without `inspectResolve`: old flow works.

Also: the existing 14 scenarios, the headed real-shortcut check, a real-ocs run
of `inspect link` plus delivery, unit tests for every rule above, and the stealth
rig re-run bound to the new zip (no new page-visible artifacts).

## Release

CLI 1.5.214 and extension 0.5.57 together; docs (zh/en), README, skill
reference (`inspect link`), CLI help, privacy note for `inspect.json` reading.

## Decision for the user

- Wildcard origins (`https://*.preview.x.com`, one label) in `inspect.json`:
  include (proposed, common for preview deployments) or exact origins only.
