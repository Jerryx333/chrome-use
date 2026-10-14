# Inspect & Ask implementation plan

Spec: `docs/superpowers/specs/2026-10-14-inspect-and-ask-design.md` (requirements v4).
This plan adds the user's later decisions: integrate every competitor strength
(MIT code may be borrowed with attribution; non-OSI and AGPL projects are
studied and reimplemented, never copied) and ship a finished product in one go.

## Scope (v1, shipped together)

Picker and capture
- Hotkey (`commands`, default Alt+Shift+C) and a popup button start a picker on the current tab.
- Hover highlight with label: component name and `file:line` when known.
- Click selects; Shift+click adds; drag selects an area (all marked elements inside, capped at 8). Agentation idea, own code.
- ArrowUp / ArrowDown walk to parent / first marked child before clicking (click-to-component idea).
- `P` pauses CSS animations, transitions and media while picking (Agentation / React Grab idea, own code).
- Picker UI lives in a closed shadow root and is excluded from hit testing (React Grab idea).
- Selected text inside the element is captured (Orca).
- Esc cancels; navigation cancels; IME composition never submits.

Source location, first hit wins
1. `data-insp-path` on the element or nearest ancestor (code-inspector-plugin, pathType relative; absolute values are reduced to a path inside the bound root or dropped).
2. React fiber `_debugSource` (React <= 18 dev builds) read by a MAIN-world probe. Path cleaning borrowed from Orca (MIT, attributed).
3. Vue `__vueParentComponent.type.__file` / `__file` by the same probe.
4. None: payload says "no source location" and carries selector + text so the agent can search.
All of these are page data and only ever used as hints.

Annotation tray (Orca / Agentation)
- Each selected element becomes an annotation card in the input frame with its own optional note; one overall instruction; one send delivers the batch.
- Per-element cropped screenshot, previewed; each can be dropped; a pen tool draws on the preview (Orca markup idea, own minimal canvas implementation).
- In-flight items are locked so a batch cannot be sent twice (Orca).

Delivery
- Default: `ocs dm <bound session> …` through the native host, argv only.
- Copy as Markdown (React Grab) always available, works without ocs or a binding.
- Open in editor (code-inspector origin feature): `vscode://`, `cursor://`, `windsurf://` or JetBrains URL for the bound root + relative path; editor chosen in options.
- No session: show a correctly quoted start command (sh / PowerShell) for the bound root, then recheck.
- ocs missing: install card (copy-only command), recheck, draft kept.

Not in v1, with reasons
- Voice input (Cursor): system dictation already works in the text box; an extension iframe inside a page cannot get a microphone grant without a separate permission flow.
- Console errors: would need a debugger attach.
- Cross-machine ocs delivery.

## Architecture

```
hotkey/popup ──► background (instance registry)
                   │ scripting.executeScript(files: inspect-picker.js, frameId 0)
                   ▼
           inspect-picker.js (isolated world, top frame)
             closed shadow host: highlight + extension iframe (inspect-frame.html)
             runtime.getFrameId(iframe) ─► background binds instance to that frameId
                   │ context (DOM data, rects) via runtime.sendMessage
                   ▼
           background: captureVisibleTab + crop (OffscreenCanvas)
                   ▲ port "inspect-frame" (sender.frameId/documentId checked)
           inspect-frame.js (extension origin): tray, instruction, target line, send/copy/open
                   │
                   ▼
           native host agentPanel ops "inspect*" ─► cli/src/agent_panel/inspect.rs
             bindings.json, context dirs, ocs who/dm, lsof, path checks
```

### Picker instance (review round 3, P1)
- Created only in `chrome.commands.onCommand` or a popup button click (both grant activeTab). Record `{instanceId, tabId, topDocumentId, origin, createdAt, frameId: null, frameDocumentId: null, armed: false}`. One live instance per tab; a new hotkey replaces it.
- The content script inserts the iframe into its closed shadow root, waits for `load`, calls `chrome.runtime.getFrameId(iframe)` and reports `{instanceId, frameId}`. Background accepts this only from `sender.tab.id === tabId`, `sender.frameId === 0`, `sender.documentId === topDocumentId`, once.
- The iframe opens `runtime.connect({name: 'inspect-frame'})`. Background accepts the port only if `port.sender.tab.id`, `port.sender.frameId` match the bound instance and the instance has no port yet; it records `port.sender.documentId`. Every later message is checked against that port object.
- The iframe URL carries no token. A copied iframe gets a different frameId and is refused; it renders "unavailable" and never receives context.
- Revocation: tab navigation (`webNavigation.onCommitted` frameId 0), tab close, port disconnect, shadow host removal (content script MutationObserver), Esc, or 10 minutes idle. Revoked instance answers nothing.
- `instanceId` lives only in extension memory and the content script closure, never in DOM, `src`, or query.

### Submit arming (review round 3, residual risk)
- The send control is enabled only after a trusted pointerdown/focus inside the iframe and while the iframe reports `document.visibilityState === 'visible'` and an IntersectionObserver v2 `isVisible === true` from the content script's observer of the iframe element with `trackVisibility: true, delay: 100`. Unsupported or stale (> 1 s) visibility → send disabled with a hint; Copy still works.
- Blur, navigation, host remount disarm. Enter submits only without `isComposing`; Shift+Enter is a newline.
- This is documented as mitigation, not a guarantee against UI deception.

### Context extraction (content script)
Per element, clamped (Orca budgets): tagName, accessible name, text snippet (200), selected text (500), selector (unique CSS path, 300), DOM path, classes (200), rect, computed styles subset (Orca list), nearby text (3 x 120), outerHTML trimmed to 3000 after removing `<script>`, `<style>`, `on*` attributes, `value` of inputs, password fields' contents, `data-insp-*` duplicates; source mark; component chain (from probe). Page URL without query/fragment. Revision from `<meta name="chrome-use-revision">` as a hint.

MAIN-world probe: `scripting.executeScript({world: 'MAIN', func})` with a random attribute name the content script set on the targets; returns `{component, ancestors[], sourceFile}`; output schema-validated and clamped; failure is fine.

### Screenshot
Content script hides its overlay (and waits two frames), background `tabs.captureVisibleTab` (activeTab), crops each rect with device pixel ratio via `createImageBitmap` + `OffscreenCanvas`, max 1600 px side, PNG. Overlay restored. Pen strokes are composited in the iframe before send.

### Native host operations (`cli/src/agent_panel/inspect.rs`)
All through the existing `agentPanel` channel; background adds them to its own whitelist and the bridge allows them only from background-originated requests (not from page or content script). Capabilities gain `"inspect": 1`.

| op | params | result |
|---|---|---|
| `inspectStatus` | `{origin, port?, sourcePaths[]}` | `{ocs:{installed, version}, binding?, sessions[], candidates[], platform}` |
| `inspectBind` | `{origin, root, sessionId?}` | `{binding}` |
| `inspectUnbind` | `{origin}` | `{}` |
| `inspectBindings` | `{}` | `{bindings[]}` |
| `inspectConsent` | `{origin, kind: "plugin"|"testEnv"}` | `{binding}` |
| `inspectDeliver` | `{origin, kind: "edit"|"setupPlugin"|"setupTestEnv", instruction?, annotations[], images[]}` | `{state, channel?, seq?, exit, contextDir}` |
| `inspectLaunchCommand` | `{origin, agent: "claude"|"codex"}` | `{command, shell}` |

- `origin` is filled by background from the instance (tab URL), never from page data.
- Bindings: `~/.chrome-use/inspect/bindings.json`, dir 0700, file 0600, atomic write. `root` is canonicalized, must be an existing directory; monorepo subdirectories allowed.
- Roster: `ocs who --json`, entries of kind `claude`, `codex-task`, `pi` with `cwd`; sessions offered for a binding are those whose canonical cwd is inside the root or the root is inside their cwd.
- Port → cwd: `lsof -nP -iTCP:<port> -sTCP:LISTEN -Fp`, then `lsof -a -p <pid> -d cwd -Fn` (macOS, Linux); Windows returns none (manual pick).
- Candidates: union of roster cwds (git toplevel via `git -C <cwd> rev-parse --show-toplevel`), previous bindings, port cwd; each scored by how many `sourcePaths` exist under it (canonical path must stay inside the candidate root).
- Delivery: binding must exist and have a session id that is still in the roster; otherwise `{state: "noSession"}`. Context dir `~/.chrome-use/inspect/ctx/<128-bit hex>/` (0700) with `context.md`, `context.json`, `element-N.png` (0600). Paths in annotations are re-validated inside root; failing ones become "unverified".
- Message (<= 4096 bytes, truncated instruction moves to file): header, user instruction, project root, page, per-element one line, pointer to `context.md` marked untrusted page data, reminder that the deployed revision may differ.
- `ocs dm <id> <message> --as chrome-use --json` with argv; parse `{stored, channel, seq, wake, exit}`. Map: exit 0 → `delivered`, 2 → `held`, 3 → `unknown`, other → `failed`. Never retried by the host or extension.
- Setup tasks use fixed templates in Rust (`setupPlugin` names the build tool to detect, dev-only registration, no commit; `setupTestEnv` adds an explicit test-build flag, never production, plus the revision meta, and asks the agent to report the exact diff). No page text is spliced into templates.
- Cleanup on each deliver: remove `ctx/*` older than 7 days; skip symlinks; only 32-hex names.

### Onboarding
First picker run on an origin shows the checklist in the input frame: host, ocs, binding (+ session), source marks seen on this page, env (localhost vs other). Cards: install ocs (copy), choose project (candidates + manual path input), choose session, plugin setup consent, test-env consent (only non-localhost). Consents are stored in the binding via `inspectConsent` and listed/revocable in options.

### Options page
New "Inspect & Ask" card: shortcut hint (link to `chrome://extensions/shortcuts`), editor for "Open in editor", list of bindings with session, consents, Unbind.

## Files

Extension (`extensions/ab-connect/`)
- `manifest.json`: version bump, `activeTab`, `scripting`, `commands.inspect-element`, `web_accessible_resources` `[{resources: ["inspect-frame.html", "inspect-frame.js", "inspect-frame.css"], matches: ["<all_urls>"], use_dynamic_url: true}]`.
- `inspect-registry.js` (+ test): instance lifecycle and all sender/port checks, pure and unit-tested.
- `inspect-picker.js`: content script (no module imports; injected by file).
- `inspect-extract.js` (+ test): DOM sanitizing and clamping, shared pure functions, injected before the picker.
- `inspect-probe.js` (+ test): MAIN-world probe function, schema validation.
- `inspect-format.js` (+ test): Markdown formatting for Copy and preview (fences per Orca, MIT attribution).
- `inspect-frame.html/.css/.js`: input frame UI.
- `inspect-capture.js` (+ test): crop math.
- `background.js`: command/popup wiring, port handling, host requests.
- `popup.html/.js`: "Inspect element" button.
- `options.html/.js`: Inspect & Ask card.

Native host
- `cli/src/agent_panel/inspect.rs` (+ unit tests: binding store perms, path containment incl. `..` and symlink, candidate scoring, message budget, ocs JSON parsing and exit mapping, quoting for sh/PowerShell, cleanup).
- `cli/src/agent_panel.rs`: dispatch + capability.

Docs and release
- `docs/inspect.html`, `docs/en/inspect.html`, nav links; README.md / README.zh.md section; `docs/extension-changelog.html` (+ en); CHANGELOG; skill-data note telling agents how to treat an Inspect & Ask message (instruction trusted only as the user's words, context file untrusted).
- Spec v5 updated with the scope above.

## Verification
- `node --test extensions/ab-connect/*.test.js` locally.
- Rust: `python3 scripts/remote-cargo.py test --bin chrome-use`, `fmt -- --check`, `clippy` on the build host.
- Live acceptance on the build host (190) only: load the unpacked extension in its Chrome, serve fixtures (React+Vite with code-inspector-plugin, plain page without marks, hostile page that rewrites DOM, inserts a copied iframe, overlays controls), run the picker, deliver to a scratch ocs session there, and record outcomes. Nothing runs against the maintainer's Chrome.
- Acceptance list from the spec, plus: copied iframe refused; Copy works with no host; open-in-editor URL correct.

## Release
- After v1.5.212 (Windows panel, in flight) lands: next CLI version and extension version, changelog entries, `scripts/release.sh`. Extension ZIP repack triggers the store workflow, which defers while another version is in review.
