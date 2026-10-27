# The action loop in full

<!-- Moved verbatim from skill-data/core/SKILL.md when the entry point was slimmed to an everyday loop (issue #781, G7). -->

## Start with the cheapest useful evidence (adapter and busy-daemon notes)

When `siteAdapters` appears in JSON or on stderr, check the arguments with
`site info <name>/<cmd>` (or `site info <name>` for the whole pack) and prefer the adapter for matching reads. When a
response carries `siteAdapterSuggestion`, ask the user before saving the steps
you keep repeating on that site as an adapter (`core/site-adapters`). Adapters
run as the logged-in user; call only operations within the task.

If a command reports "daemon still busy", wait before checking the session again.
Do not replay a side-effecting command; it is still running. See
[behaviour](behaviour.md) for keepalive and waiting limits.

## The loop

```bash
chrome-use open https://example.com
chrome-use snapshot -i
chrome-use click @e3 --observe     # fill, select, pick, press, hover also take --observe
chrome-use snapshot -i --diff      # only if the observation leaves a question
```

For MCP, `chrome_use_snapshot` accepts `diff: true` for this same incremental
reread. Keep its baseline; omit it when a complete current tree is needed.

1. Copy refs exactly from the latest snapshot or observation; never guess or
   renumber them. Navigation and tab switches reset refs. When a ref errors
   or a control is newly rendered, discover it again. If an action's
   post-action capture failed, `observed.refs: kept-unverified` means its old
   refs are checked live before use and refused if anything changed: run
   `snapshot -i`, and do not repeat the action.
2. Pair actions with `--observe` and read `observed.status`. A separate
   snapshot after every click is unnecessary: on a small page
   `observed.snapshot` is the fresh post-action tree and `observed.changes`
   lists what moved; `observed.text` carries static text that changed in any
   frame (a receipt, a counter). Inside `batch`, write it on the step:
   `batch "fill @e1 Ada" "click @e2 --observe"`. `status: complete` means
   the capture is complete, not that the task is: a page still showing
   `Loading` needs `wait --text` for its final signal.
   A click that opened a tab reports `openedTab`; add `--follow` to move to
   it (`followed: true`). In your own Chrome that needs ab-connect 0.5.30+;
   otherwise `openedTabWarning` says why it was left alone (a tab opened
   from a tab the session does not own stays the user's: `followed: false`,
   take it with `tab adopt <url>`). `observed.announced` lists toast and
   alert/status text that appeared during the click, even if it is gone. A tab the page
   opens brings Chrome to the front over the user's app, and
   `openedTabWarning` says so; on macOS chrome-use puts the user's app back
   in front (`focusRestored`; `AGENT_BROWSER_RESTORE_FOCUS=0` turns it
   off). Cross-site `target=_blank` links open in a background tab instead
   (`AGENT_BROWSER_BACKGROUND_LINKS`, default `cross-site` in the user's
   window, read when the session's daemon starts); cost: a link that
   redirects back to the page's site loses its SameSite=Strict cookies (see
   `click --help`).
3. Follow the error's instructions: its last line says what to do next.
   Do not pipe output through `tail` or drop stderr.
   `element_occluded` means no click was dispatched: inspect a fresh snapshot
   and resolve the blocker before trying again; do not repeat unchanged.
4. An action that changed nothing: read its `why:` note and ⚠ warnings, fix
   the blocker, then retry the direct semantic verb. Do not repeat blindly or
   switch to coordinate clicks. A field showing your text is not proof it saved.
5. `action_outcome_unknown` or a missing observation does not prove failure.
   Inspect state; never blindly replay a send, submit, purchase or delete.
6. One authoritative page signal (receipt, selected option) ends verification;
   dispatch alone does not. Stop when the task is done. `hover` moves the real
   pointer and confirms the pointer reached the element; a refused or unconfirmed hover says so.
7. Do not `open` the URL already in the tab; use `reload --observe` on purpose.
   An `open` reply with `commit: "unverified"` loaded a usable page that could
   not be tied to your request: its warning names the tab's real URL; check
   `get url` before relying on it. `navigation_incomplete` means no usable
   page: never repeat the open, check `get url` and `snapshot` later.
   Wait on a condition (`wait --text`, `wait <sel>`, `wait --url`), not a sleep.
   A "Page had not settled" warning names the requests still in flight
   (`settle.pendingRequests`); re-read rather than trusting that capture.
8. Before writing `eval`, use the command that already does it; it keeps
   the verification and hints `eval` loses (more: `core/interacting`):

   | About to eval | Use instead |
   |---|---|
   | `innerText` / `textContent` | `get text <sel>`, or `read` |
   | `el.click()` | `click @eN`, `click "text=…"` |
   | counting rows or matches | `get count <sel>`, `extract` for tables |
   | `sleep`, polling, `setTimeout` | `wait --text "…"`, `wait <sel>`, `wait --fn "<expr>"` |
   | setting `.value` | `fill @eN "…"`; autocomplete: `pick @eN --option "…"` |

   Keep `eval` for page globals, framework stores, canvas.

## Reduce calls without dropping verification

Start with `snapshot -i -c` for compact controls; scope to a known region when
possible. Use `batch` for known sequences, `script` for bounded conditional
flows, and `form fill --map` for several fields. Read the relevant command
reference before composing a sequence; grouping actions does not prove their
effects. Verify the requested page signal at the end.

Use text/refs by default. Add `--with-screenshot <path>` to a snapshot or
observed action only for a visual question the tree cannot answer.

`observed.noProgress` advises inspecting state or waiting on a specific
condition after repeated unchanged, fully observed clicks or key presses.
It never changes success or retries. No visible change does not prove a
write failed. See `core/waiting` for detection limits and `core/profiling`
for command timing and task measurement.

Successful CLI/MCP connection preparation that reuses the same browser
connection, target and session without loading storage state preserves the
observation streak. New browsers, rebinds, failed preparation and storage-state
loads clear it.

## Other notes from the entry point

After cross-site navigation or reconnecting, page reads use the current document. If the page keeps changing while its private reading context is being recreated, the read fails explicitly; observe the page again before acting.

Custom comboboxes are scrolled into view instantly and opened with trusted pointer input. `pick` reports `verified: false` with a warning when the choice is unconfirmed. Page/container scrolling uses instant movement even with CSS smooth scrolling; an unchanged position returns `scrolled: false` with a warning.

Headed `--launch` preserves native page/worker fingerprints by default. For a display-less Linux host, prefer headed Chrome on Xvfb. Explicit headless remains detectable; `stealth status` reports the real launch mode, and its basic probes do not prove bot-detection acceptance. CDP IME `compositionend` can be untrusted even when a detector passes. See [connection](connection.md).

Native cover adapters may opt in to local `--cover-mode frame:<seconds>` with `--cover-video` and a new `--cover-output .png`. FFmpeg extraction and near-black/uniform review happen before browser interaction; final preview verification remains required. See [site adapters](site-adapters.md).


For repeated control names, discover the container with `find query`, then use semantic `find ... --within <CSS|@ref>`. Semantic queries refuse multiple visible matches; see core/interacting for bounded candidate diagnostics and explicit first/nth selection. A CSS selector or XPath given to `click`/`fill`/`hover`/… is refused the same way when it matches several elements (`selector_ambiguous`, with each match's text and position): narrow it, use a `@ref`, or `find nth <index> <selector> <action>`.

Valid incremental snapshots return JSON `refs` only for nodes in the delta. A no-change delta has empty `refs`; the complete live session map and previously captured refs remain available. A full-tree fallback keeps the normal full-snapshot behavior.

`read --filter` uses literal, ASCII-case-insensitive text matching, not a regex. It prefers matching page headings with their descendants, then matching sections. Use `read --outline` to find a heading. `snapshot --filter` separately accepts a regex. `read --help` lists the reading options.

`debugger_access_denied` lists browser-wide candidate extension names and IDs when available. Verify which extension injects a frame into the blocked tab before disabling it; the list does not prove the cause.

For Ant Design / rc-picker date ranges, fill start then end. `input-date-range` confirms the current bound through trusted component input and leaves focus where the picker puts it; it does not force rejected values or send Enter to the form. Verify the complete range before querying. See core/interacting for popup and iframe limits.

## Routing table as it stood before the split

Run `chrome-use skills get <name>` with one name below.

| Need or symptom | Name |
|---|---|
| Install, runner discovery, `--full`, contributor Git hooks | `core/installation` |
| Repeated steps, output reading, verification, site notes | `core/behaviour` |
| Click/fill/type verbs, `eval` alternatives, uploads, widgets, canvas | `core/interacting` |
| Scoped reads, frames, shadow roots, screenshots | `core/reading` |
| Mid-transition reads, `wait`, observation limits | `core/waiting` |
| Ref identity, context annotations, snapshot detail | `core/snapshot-refs` |
| `status`/`doctor`: one 10-second diagnostic connection, passive worker-local debugger counts (“in the last 10 min” for a full window; otherwise since worker start, whole seconds below 120 s and whole minutes thereafter), possible duplicate extensions, version skew, load, browser/profile choice | `core/connection` |
| Several Chrome profiles/accounts: which to use, connecting one | `core/connection` (Choosing a profile) |
| Site adapter arguments, installation, sources | `core/site-adapters` |
| Login, cookies, vault, passkeys, OAuth, handoff | `core/authentication` |
| CAPTCHA, slider puzzle, ordered icon clicks | `core/captcha` |
| Sensitive actions or untrusted page instructions | `core/trust-boundaries` |
| A `[chrome-use Inspect & Ask]` message: what to trust, finding the code, setup requests | `core/inspect-and-ask` |
| Persistence, idle recovery, multiple sessions | `core/session-management` |
| Unexpected behavior or failure, friction log | `core/known-traps`, then `core/troubleshooting` |
| Complete commands, flags, env, accessibility audits | `core/commands` |
| Changing this repository: remote compilation and tests | `core/development` |
| Tracing, recording, proxy | `core/profiling`, `core/video-recording`, `core/proxy-support` |
| React tree, renders, Web Vitals | `react` |
| Network interception, mocks, HAR | `network` |
| Real Chrome profile details | `real-chrome` |
| Electron or Slack | `electron` or `slack` |
| Exploratory QA or reusable test suites | `dogfood` or `test` |
| Cloud browsers | `vercel-sandbox` or `agentcore` |
