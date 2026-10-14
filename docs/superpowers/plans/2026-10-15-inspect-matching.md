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
- **Target**: `{root, sourceRoots, sessionId, sessionCwd}`. `root` is canonical:
  a worktree toplevel or an app directory inside it. `sourceRoots` defaults to
  `[root]`; extra entries (shared packages) come only from a config's
  `sourceRoots` and are listed on the confirmation card. Source paths never
  resolve outside `sourceRoots`, even inside the same worktree.
- **Related**: a session is related to a root when both resolve to the same
  worktree toplevel (`git rev-parse --show-toplevel`) and one contains the other.
  Sessions in another worktree of the same repo are never related.

## Data (host, `~/.chrome-use/inspect/bindings.json`, version 2)

```json
{"version": 2,
 "bindings": [{"origin": "https://test-admin.x.com", "pathScope": "/", "root": "/w/admin", "sourceRoots": ["/w/admin"],
   "sessionId": "claude-…", "sessionCwd": "/w/admin", "sessionLabel": "admin-main",
   "source": "confirmed|linked", "evidence": {"kind": "port", "port": 5173, "cwd": "/w/admin"},
   "consents": {"plugin": false, "testEnv": false}, "updatedAt": 0}],
 "pendingClaims": [{"claimId": "<128-bit hex>", "origin": "…", "pathScope": "/", "root": "…",
   "sessionId": "…", "sessionCwd": "…", "affects": ["<fingerprint of every binding whose scope overlaps>"],
   "requestedAt": 0}]}
```

Version 1 files migrate on first write: each entry becomes `pathScope: "/"`,
`source: "confirmed"`, `sourceRoots: [root]`.

## Project configuration (`.chrome-use/inspect.json`, committed by the team)

```json
{"version": 1, "sites": [
  {"origins": ["https://test-admin.x.com", "http://localhost:5173"], "pathScope": "/"},
  {"origins": ["https://*.preview.x.com"], "pathScope": "/admin"}]}
```

- The file's directory's parent is the app root (`apps/admin/.chrome-use/inspect.json`
  → root `apps/admin`); the page never names a root. Optional `"sourceRoots"`
  (paths relative to the app root, staying inside the worktree, no `..` escaping
  it) names shared packages; they are shown on the card and stored only when the
  user confirms.
- Constraints: the file and each parent component must not be symlinks
  (`symlink_metadata`), at most 64 KiB, strict schema (unknown fields or a
  `version` other than 1 reject the file with a visible reason), at most 32 sites.
- Read only inside known worktrees (toplevels of ocs session cwds and of
  existing binding roots, at most 16 worktrees) via
  `git ls-files '*.chrome-use/inspect.json'`, at most 50 files. All git and lsof
  calls of one resolve share a total budget of 8 s (3 s per call); what does not
  fit is reported as skipped, never guessed. Never a filesystem-wide search.
- Origins are exact, or an explicit single-label wildcard `https://*.preview.x.com`:
  scheme and port must match exactly, `*` matches exactly one DNS label. An
  exact origin outranks a wildcard; equal matches in more than one worktree are
  never preselected. A binding always stores the page's exact origin, so one
  confirmation never covers other subdomains.
- A match proposes and preselects; it never authorizes on its own.

## Resolution (`inspectResolve`, read-only)

Input: `origin`, `pagePath` (no query/fragment), `sourcePaths`, `revision?`,
`port?` (loopback only). Output: `{state, binding?, card?, candidates[], pendingClaims[]}`.

1. **Effective binding**: the longest-matching scope among confirmed bindings
   only. Pending claims never take part in this match, so an unaccepted claim
   cannot shadow a valid default or make the page look unbound.
2. **Pending claims**: matched separately; every claim whose scope applies to
   this URL is returned (`pendingClaims`), also when a binding is effective. The
   frame shows them first, each with the current effective target and the
   claimed one. Until a claim is accepted the effective binding keeps receiving
   sends; the claimant receives nothing.
3. **Bound**: check the bound session is in the roster with the same canonical
   cwd, and, for bindings with port evidence on loopback, that the port's owner
   cwd is in the same worktree as the root and equal to, inside, or containing
   it. All hold: `state: "bound"`. Otherwise `state: "needsConfirm"` with the
   reason (`sessionEnded`, `sessionMoved`, `portMoved`) and a card.
4. **Unbound**: build candidate roots from config matches (`config`), loopback
   port owner cwd (`dev-server-port`, keeps app subdirectories), previous
   binding roots, and ocs session cwds (worktree toplevel); at most 12. Rank by
   evidence (exact config > wildcard config > port > previous > session), then
   CI source-path suffix matches, then revision relation. Pair each root with
   its related live sessions.
5. **Preselect** the top pair only when the root has config or port evidence,
   is not tied with another worktree, and has exactly one related live session.
   Otherwise no preselection; the card lists root × session pairs.

Revision relation per candidate: page revision must match `^[0-9a-f]{7,40}$`;
`git cat-file -e <rev>^{commit}` then `git rev-list --left-right --count <rev>...HEAD`,
3 s timeout → `same | ahead | behind | diverged | unknown`. Shown and used for
ranking only.

## Confirmation card (frame)

One card, one decision: "Send requests from `<scope>` to `<session>` (`<kind>`,
`<status>`, cwd `<sessionCwd>`) in `<root>`", listing any extra `sourceRoots`, with evidence chips (config, dev server, CI paths
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
`chrome-use inspect unlink <url> [--path-scope </seg>]`, `chrome-use inspect list`.

- `unlink` removes only the scope it names (longest match for the URL unless
  `--path-scope` is given), and only when that scope is bound to the calling
  session; it also withdraws that session's pending claims. Other sessions'
  bindings are removed from the extension options page.

- Identity: `ocs whoami --json` gives the id; the cwd comes from the matching
  `ocs who --json` entry. No roster entry or a non-absolute cwd: refuse.
- Root: `--root`, else the app root of a config in the session's worktree that
  matches the URL, else the session cwd.
- Overlap rule: two scopes overlap when they have the same origin and one
  `pathScope` is a segment prefix of the other (`/` overlaps `/admin`). The
  link is written directly (`source: linked`) only when no overlapping binding
  has a different target. Otherwise it becomes a pending claim (new `claimId`,
  `affects` = fingerprints of every overlapping binding with a different
  target), printed with the scopes it would take over; no `--force`. A narrower
  scope cannot bypass a broader binding: B linking `/admin` while `/` belongs to
  A is a claim, and B linking `/` while `/admin` belongs to A is a claim that
  lists `/admin` as affected (accepting it moves `/` to B and leaves `/admin`
  with A unless the user also replaces that one on the card).
- A newer claim on the same scope supersedes the older one; claims on
  different scopes coexist.
- When several configs in the session's worktree match the URL, link does not
  pick one; it lists them and asks for `--root`.
- Skill reference tells agents to run it when the user says to hand a site to
  them.

## Host operations

| op | change |
|---|---|
| `inspectResolve` | new, above |
| `inspectStatus` | kept for 0.5.56 extensions, origin-level only: when the origin has any path-scoped binding or pending claim it returns `upgradeRequired` instead of a binding, and the legacy `inspectDeliver` (no `pagePath`) refuses with `upgradeRequired`; it never picks a scope for an unknown path |
| `inspectBind` | adds `pathScope`, `source: confirmed`, records evidence; clears consents when root or session changes (as now) |
| `inspectClaimDecide` | `{claimId, expectedAffects, accept}`; under the bindings lock, recompute the complete set of bindings overlapping the claim's scope with a different target and refuse unless it equals `expectedAffects` exactly (a changed, removed or newly added overlapping binding makes the card stale) and the claim is still current; check, decision and write happen in that one lock |
| `inspectDeliver` | adds `pagePath` for scope matching and an optional tab target `{root, sourceRoots, sessionId, sessionCwd, confirmedFingerprint}` from the card the user clicked; the host recomputes the fingerprint from live data and refuses a mismatch, never widens `sourceRoots` beyond what that card showed, re-validates live/related/canonical root and re-runs the port check |
| capabilities | `"inspectResolve": true`; a new extension on an older host falls back to `inspectStatus` and says that path scopes, tab targets and claims need the newer CLI |

Source paths resolve only inside `sourceRoots`. A repo-relative path
(`apps/admin/src/x.tsx`) is matched by stripping the confirmed root's path
relative to the worktree toplevel, then canonicalized inside the root; absolute
CI paths use the existing suffix matching inside `sourceRoots`. Anything else is
unverified, even when it exists elsewhere in the same worktree.

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
    preselect without config/port evidence; an admin page carrying
    `apps/shop/...` or `../../shared/...` marks stays unverified unless that
    directory is a confirmed source root.
14. New extension on a host without `inspectResolve`: old flow works, with the
    stated limits.
15. Extension 0.5.56 on the new host: origin-level binding works; with a
    path-scoped binding or a pending claim on the origin, status and deliver
    return `upgradeRequired` and nothing is sent.
16. Version 1 bindings migrate to version 2 unchanged in effect; a v2 file with
    two scopes on one origin round-trips.
17. Pending claim on a bound scope is shown (scenario 10 with an active
    binding); accepting from a stale card after a second claim, or after the
    binding changed, is refused.
18. Wildcard config: `a.preview.x.com` matches `*.preview.x.com`,
    `a.b.preview.x.com` and another port do not; the stored binding is the
    exact origin; an exact-origin config in another worktree outranks it.
19. `inspect.json` that is a symlink, oversized, unknown version or unknown
    field is rejected with a visible reason; the resolve budget is respected.
20. `inspect unlink` from session B cannot remove session A's binding.
21. `/` bound to A, B links `/admin`: pending claim; `/admin` pages still go to
    A until accepted.
22. `/admin` bound to A, B links `/`: pending claim listing `/admin` as
    affected; accepting moves `/` only, `/admin` stays with A.
23. Two pending claims on different scopes of one origin are both shown and
    decided independently; deciding from a stale card is refused.
24. Tab target carrying wider `sourceRoots` or a wrong fingerprint is refused.
25. `inspect link` with two matching configs in one worktree asks for `--root`.

Also: the existing 14 scenarios, the headed real-shortcut check, a real-ocs run
of `inspect link` plus delivery, unit tests for every rule above, and the stealth
rig re-run bound to the new zip (no new page-visible artifacts).

## Release

CLI 1.5.214 and extension 0.5.57 together; docs (zh/en), README, skill
reference (`inspect link`), CLI help, privacy note for `inspect.json` reading.

## Decision for the user

- Wildcard origins in `inspect.json`: included in this release as explicit
  single-label patterns under the rules above, unless the user declines.
