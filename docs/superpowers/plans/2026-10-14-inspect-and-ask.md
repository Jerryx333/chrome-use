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

### Picker instance and frame handshake (plan review P1-1, PoC verified)
Chrome has no `runtime.getFrameId(element)`, so the frame is authenticated with a one-time secret that only travels through channels the page cannot read. PoC: `docs/superpowers/plans/inspect-poc/` (run on the build host with Chrome for Testing 155, `CHROME=… node drive.mjs`).
- Created only in `chrome.commands.onCommand` or a popup button click (both grant activeTab). Background records `{instanceId, tabId, topDocumentId (webNavigation.getFrame frameId 0), origin, secret (randomUUID), secretHanded:false, used:false, frameId:null, port:null}`. One live instance per tab; a new start replaces it.
- Background injects `inspect-picker.js` into frame 0. The picker asks background for the secret with `runtime.sendMessage`; background answers once, only when `sender.frameId === 0` and `sender.documentId === topDocumentId`.
- The picker creates the iframe inside its closed shadow root (load listener attached before `src`), and on `load` posts `{type:'init'}` with a fresh `MessageChannel` port to `iframe.contentWindow`, target origin `chrome-extension://<runtime.id>` (the frame runs at the static extension origin even when loaded through the dynamic URL; PoC observed this). The secret goes over that MessagePort, which the page cannot observe.
- The frame accepts any number of init ports until authorized (a page flooding fake inits cannot lock it out), connects `runtime.connect({name:'inspect-frame'})` and presents the secret. Background accepts when `secret` matches, `used` is false, `sender.tab.id === tabId`, `sender.frameId !== 0`, path is `/inspect-frame.html`; then `used = true` and the port, frameId and frame documentId are recorded. Wrong secrets are refused without consuming the instance.
- After authorization the frame only trusts the MessagePort that delivered the valid secret.
- PoC results: copied frame created before start with a guessed secret → refused; page posting fake inits to every frame every 20 ms during start → 0 accepted, real frame authorized; copy created after authorization → refused; reload → instance revoked and port disconnected; service worker killed → frame disconnected, new worker has no instance, nothing re-authorizes.
- Revocation: top-frame navigation (`webNavigation.onCommitted` frameId 0 with a different documentId), tab close, port disconnect, shadow host removed (picker MutationObserver), Esc, 10 minutes idle, worker restart (instances are memory only, so a restarted worker has none; the frame shows "Session ended, press the shortcut again").
- Secret and instanceId never appear in DOM, `src`, query or page-visible messages.

### Submit verification (plan review P1-2, PoC verified)
- No cached visibility. Each submit (send button click or non-IME Enter, trusted event inside the frame) puts the frame in `verifying`; the frame asks the picker over the authenticated MessagePort with a nonce; the picker creates a new `IntersectionObserver({threshold:[1], trackVisibility:true, delay:100})` on the iframe element, takes the first callback, disconnects, and answers `{nonce, isVisible, ratio, documentVisibility}`; 1 s timeout.
- Send proceeds only for `isVisible === true`, `ratio >= 0.99`, `documentVisibility === 'visible'`. `isVisible` undefined → "This browser cannot confirm the box is visible; use Copy." False → "The box is covered or moved; scroll or press the shortcut again." Timeout → "Could not confirm visibility; try again." Every failure returns the frame to `editing`; nothing stays armed.
- PoC results on the build host: fresh check right after load → visible; after 10 s idle → visible; full-page transparent overlay → not visible; host opacity 0.5 → not visible; restored → visible; host translated → not visible. Our own frame therefore uses no transform or opacity.
- Blur, navigation, or host remount cancel a pending verification. This remains mitigation, not a guarantee against UI deception.
### Context extraction (content script)
Per element, clamped (Orca budgets): tagName, accessible name, text snippet (200), selected text (500), selector (unique CSS path, 300), DOM path, classes (200), rect, computed styles subset (Orca list), nearby text (3 x 120), outerHTML trimmed to 3000 after removing `<script>`, `<style>`, `on*` attributes, `value` of inputs, password fields' contents, `data-insp-*` duplicates; source mark; component chain (from probe). Page URL without query/fragment. Revision from `<meta name="chrome-use-revision">` as a hint.

MAIN-world probe: `scripting.executeScript({world: 'MAIN', func})` with a random attribute name the content script set on the targets; returns `{component, ancestors[], sourceFile}`; output schema-validated and clamped; failure is fine.

### Screenshot (plan review P1-4)
`captureVisibleTab` shoots the active tab of a window, so a capture is valid only when every check holds; otherwise screenshots are dropped and the UI says why. The extension never activates or switches tabs.
1. Background keeps a per-instance `generation`, incremented by `tabs.onActivated` (same window), `webNavigation.onCommitted` frame 0, `tabs.onZoomChange` for the tab, and `windows.onRemoved`.
2. Picker sends layout L1 `{innerWidth, innerHeight, scrollX, scrollY, devicePixelRatio, rects}`, hides its overlay, waits two animation frames.
3. Background: `tabs.get(tabId)` must be `active` and `status === 'complete'`; record windowId and generation g0; `captureVisibleTab(windowId, {format:'png'})`.
4. Background re-reads: tab still active in the same window, `webNavigation.getFrame(frameId 0).documentId === topDocumentId`, generation still g0; picker sends L2, which must equal L1 (rects within 1 CSS px).
5. Scale from the bitmap: `sx = bitmap.width / innerWidth`, `sy = bitmap.height / innerHeight`; `|sx - sy| <= 0.02` else drop. Crop rects scaled by sx/sy, clamped to the bitmap, max side 1600 px.
6. Overlay restored in every outcome.

### Native host operations (`cli/src/agent_panel/inspect.rs`)
All through the existing `agentPanel` channel; background adds them to its own whitelist and the bridge allows them only from background-originated requests (not from page or content script). Capabilities gain `"inspect": 1`.

| op | params | result |
|---|---|---|
| `inspectStatus` | `{origin, port?, sourcePaths[]}` | `{ocs:{installed, version}, binding?, sessions[], candidates[], platform}` |
| `inspectBind` | `{origin, root, sessionId?}` | `{binding}` |
| `inspectUnbind` | `{origin}` | `{}` |
| `inspectBindings` | `{}` | `{bindings[]}` |
| `inspectConsent` | `{origin, kind: "plugin"|"testEnv"}` | `{binding}` |
| `inspectDeliver` | `{deliveryId, origin, kind: "edit"|"setupPlugin"|"setupTestEnv", instruction?, pageUrl, env, revision?, annotations[], images[]}` | `{state, stored?, wake?, exit?, channel?, seq?, contextDir?}` |
| `inspectDeliveryStatus` | `{deliveryId}` | `{delivery}` or `{delivery: null}` |
| `inspectLaunchCommand` | `{origin, agent: "claude"|"codex"}` | `{command, shell}` |

- `origin` is filled by background from the instance (tab URL), never from page data.
- Bindings: `~/.chrome-use/inspect/bindings.json`, dir 0700, file 0600, atomic write. `root` is canonicalized, must be an existing directory; monorepo subdirectories allowed.
- Roster: `ocs who --json`, entries of kind `claude`, `codex-task`, `pi` with `cwd`; sessions offered for a binding are those whose canonical cwd is inside the root or the root is inside their cwd.
- Port → cwd: `lsof -nP -iTCP:<port> -sTCP:LISTEN -Fp`, then `lsof -a -p <pid> -d cwd -Fn` (macOS, Linux); Windows returns none (manual pick).
- Candidates: union of roster cwds (git toplevel via `git -C <cwd> rev-parse --show-toplevel`), previous bindings, port cwd; each scored by how many `sourcePaths` exist under it (canonical path must stay inside the candidate root).
- Delivery: binding must exist and have a session id that is still in the roster; otherwise `{state: "noSession"}`. Context dir `~/.chrome-use/inspect/ctx/<128-bit hex>/` (0700) with `context.md`, `context.json`, `element-N.png` (0600). Paths in annotations are re-validated inside root; failing ones become "unverified".
- Message (<= 4096 bytes, truncated instruction moves to file): header, user instruction, project root, page, per-element one line, pointer to `context.md` marked untrusted page data, reminder that the deployed revision may differ.
- `ocs dm <id> <message> --as chrome-use --json` with argv. Delivery records (plan review P1-3): `deliveries/<deliveryId>.json` is created with `create_new` and state `started` before ocs runs, then the receipt is written. A second call with the same id never runs ocs; it returns the record, and `started` without a receipt is reported as `unknown`. Raw `stored`, `wake` (accepted/deferred/parked/failed/unknown/skipped/none as ocs reports), `exit`, `channel`, `seq` are returned. `state`: exit 0 → `sent`; exit 2 → `stored` (wake shown separately); exit 3 → `unknown`; other → `stored` if JSON says stored else `failed`. Nothing retries.
- Extension side: the batch draft (text, notes, deliveryId; screenshots kept in memory only) is saved in `chrome.storage.local` per origin before sending. Bridge timeout (35 s for inspectDeliver), disconnect or worker restart leave the batch `unconfirmed`; the frame queries `inspectDeliveryStatus` when the host is back and never re-enables Send for that deliveryId. "Start a new batch" copies the draft under a new id only on an explicit click.
- Session identity (P2): the binding stores sessionId and the session's canonical cwd at bind time; delivery requires the same id with the same cwd in the roster, else `sessionChanged` and a re-pick. Candidate filtering is display only.
- Consent (P2): consents are stored per binding and cleared when root or session changes; the host refuses setup deliveries without the matching consent for the current root.
- Files (P2): bindings read-modify-write under a lock; data dirs/files created with `create_new` and `O_NOFOLLOW`, every component checked with `symlink_metadata`; cleanup never follows links.
- Setup tasks use fixed templates in Rust (`setupPlugin` names the build tool to detect, dev-only registration, no commit; `setupTestEnv` adds an explicit test-build flag, never production, plus the revision meta, and asks the agent to report the exact diff). No page text is spliced into templates.
- Cleanup on each deliver: remove `ctx/*` older than 7 days; skip symlinks; only 32-hex names.

### Pausing (P2)
Pause records each animation's and media element's prior state (`Element.getAnimations()` play state, `media.paused`) and restores exactly that on resume, Esc, send or teardown. The style override is removed on teardown.

### Onboarding
First picker run on an origin shows the checklist in the input frame: host, ocs, binding (+ session), source marks seen on this page, env (localhost vs other). Cards: install ocs (copy), choose project (candidates + manual path input), choose session, plugin setup consent, test-env consent (only non-localhost). Consents are stored in the binding via `inspectConsent` and listed/revocable in options.

### Options page
Feature toggle, default off; the first shortcut press while off opens the frame on its consent card ("Enable Inspect & Ask for this browser"), which turns it on. New "Inspect & Ask" card: shortcut hint (link to `chrome://extensions/shortcuts`), editor for "Open in editor", list of bindings with session, consents, Unbind.

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
- `cli/src/output.rs` help text (the host ops have no CLI flags; the help gains a short Inspect & Ask note pointing to the docs page), `skill-data/core/SKILL.md` overview line and `skill-data/core/references/inspect-and-ask.md` (how an agent should treat an Inspect & Ask message).
- Borrowed code: each file that adapts Orca or React Grab code names the source repo, commit and path in its header, and `extensions/ab-connect/THIRD_PARTY_NOTICES.md` carries the full MIT license texts.
- Spec v5 updated with the scope above.

## Verification
- `node --test extensions/ab-connect/*.test.js` locally.
- Rust: `python3 scripts/remote-cargo.py test --bin chrome-use`, `fmt -- --check`, `clippy` on the build host.
- Live acceptance on the build host (190) only: load the unpacked extension in its Chrome, serve fixtures (React+Vite with code-inspector-plugin, plain page without marks, hostile page that rewrites DOM, inserts a copied iframe, overlays controls), run the picker, deliver to a scratch ocs session there, and record outcomes. Nothing runs against the maintainer's Chrome.
- `scripts/stealth-bench.sh` on the build host (this release injects into pages).
- Live acceptance includes React and Vue test builds with the plugin, a production build with zero marks, two sessions in one repo, copied frame, and unknown-delivery recovery (host killed after ocs stores).
- Acceptance list from the spec, plus: copied iframe refused; Copy works with no host; open-in-editor URL correct.

## Release
- After v1.5.212 (Windows panel, in flight) lands: next CLI version and extension version, changelog entries, `scripts/release.sh`. Extension ZIP repack triggers the store workflow, which defers while another version is in review.
