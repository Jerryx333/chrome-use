# Inspect & Ask matching: host ↔ extension contract

Companion to `2026-10-15-inspect-matching.md`. Every result object carries
`"schemaVersion": 1`. Unknown request fields are rejected.

## Shared shapes

```text
Scope       = { origin, pathScope }                       pathScope: "/" or "/seg[/seg…]"
SessionInfo = { id, kind, name?, status?, cwd }
Target      = { root, sourceRoots[], sessionId, sessionCwd, sessionLabel? }
Pair        = Scope + Target + {
                kind, status?, evidence[]:  "config" | "config-wildcard" | "dev-server-port" | "previous-binding" | "ocs-session",
                matchedPaths: n,
                revision?: { relation: "same"|"ahead"|"behind"|"diverged"|"unknown", ahead?, behind? },
                fingerprint }
Binding     = Scope + Target + { source: "confirmed"|"linked", consents{plugin,testEnv}, evidence?, updatedAt, fingerprint,
                session?: SessionInfo | null }
Claim       = Scope + Target + { claimId, requestedAt, affects[]: (Scope + Target + {fingerprint}) , expectedAffects[]: fingerprint }
```

`fingerprint` = lowercase hex SHA-256 (first 32 chars) over
`origin \n pathScope \n root \n sourceRoots joined by \n \n sessionId \n sessionCwd`,
recomputed by the host from live data whenever a request quotes one.

## inspectResolve

Request: `{ origin, pagePath, sourcePaths?: string[] (≤16), revision?: string, port?: u16 }`

Result:

```text
{ schemaVersion, platform, ocs{installed, version?, path?}, rosterError?,
  state: "bound" | "needsConfirm" | "unbound",
  reason?: "sessionEnded" | "sessionMoved" | "portMoved",
  binding?: Binding,                       // effective binding (bound, or the stale one on needsConfirm)
  card?: { preselected?: Pair, pairs: Pair[] },   // on needsConfirm and unbound
  pendingClaims: Claim[],                  // every claim applying to this URL, always present
  sessions: SessionInfo[],                 // live sessions related to the effective/top root (for display)
  configErrors?: [{ path, reason }],
  skipped?: [string] }                     // budget overruns, never guessed
```

## inspectBind

Request: `{ origin, pathScope?: "/", root, sourceRoots?: string[], sessionId, sessionLabel?, fingerprint }`
The host rebuilds the pair from live data (config sourceRoots, session cwd,
related check) and refuses unless its fingerprint equals `fingerprint`. Stores
the exact origin, `source: "confirmed"`, evidence (port when the pair had it).
Consents are kept only when root and session are unchanged.
Result: `{ schemaVersion, binding: Binding }`.

## inspectClaimDecide

Request: `{ claimId, expectedAffects: fingerprint[], accept: bool }`
Under the bindings lock: the claim must be current for its scope and the full
set of overlapping bindings with a different target must equal
`expectedAffects`. Accept writes the claimed binding for the claim's scope
only; reject drops the claim.
Result: `{ schemaVersion, decided: "accepted" | "rejected", binding?: Binding }`.

## inspectDeliver (additions)

Request adds `pagePath` (new callers) and optional
`target: { root, sourceRoots[], sessionId, sessionCwd, fingerprint }` for a
tab-only target. With `pagePath`, the effective binding is the longest
confirmed scope (or the tab target after fingerprint re-validation). Without
`pagePath` (extension 0.5.56) only an origin-level `/` binding is used, and
`state: "upgradeRequired"` is returned when the origin has any other scope or a
pending claim. New states: `upgradeRequired`, `targetChanged` (tab target or
port check failed). Everything else as before.

## inspectStatus (legacy)

Unchanged for 0.5.56, except: when the origin has a path-scoped binding or a
pending claim, `binding` is omitted and `upgradeRequired: true` is set.

## inspectBindings / inspectUnbind (options page)

`inspectBindings` → `{ schemaVersion, bindings: Binding[], pendingClaims: Claim[] }`.
`inspectUnbind` request `{ origin, pathScope?: "/" }` → `{ schemaVersion, unbound: true, existed }`
(the options page acts for the user and may remove any binding).

## Capabilities

`"inspectResolve": true` in `capabilities()`.

## CLI

```text
chrome-use inspect link <url> [--root <dir>] [--path-scope </seg>] [--json]
chrome-use inspect unlink <url> [--path-scope </seg>] [--json]
chrome-use inspect list [--json]
```

Identity from `ocs whoami --json` (id) joined with `ocs who --json` (cwd).
Exit codes: 0 linked/unlinked, 2 pending claim written (browser confirmation
needed), 1 refused (reason printed).

## Agreed during implementation

1. `inspectBind` without `fingerprint` is the 0.5.56 shape: origin-level `/`
   only, `sourceRoots [root]`, refused with `upgradeRequired` when the origin
   has another scope or a pending claim. `sessionId` stays optional.
2. `inspectConsent` and `inspectLaunchCommand` take optional `pathScope`
   (default `/`).
3. `sourceRoots` in `inspect.json` is file-level (the app root is per file).
4. `card.pairs` may contain a root with no related live session
   (`sessionId`/`sessionCwd` null, `""` in the fingerprint); it is shown with a
   start hint, never preselected and never confirmable.
5. `needsConfirm` reason `rootMissing` when the bound root is gone or moved.
6. A tab target's fingerprint is accepted for any scope applying to
   `pagePath`; `pagePath` must equal the path of `pageUrl`; `sourceRoots` must
   equal the recomputed pair's.
7. `inspect link` without `--path-scope` uses the matched config site's
   `pathScope`, else `/`; `--path-scope` must apply to the URL's path.
8. Accepting a claim writes `source: "linked"` and re-checks the claimant is
   live with the same cwd.
