# Verifiable panel turns: implementation plan

Baseline: line numbers below refer to `70bb31f7`; the branch starts from origin/main (v1.5.214).

Source research: `reports/Browser Use 对话系统深度研究.md` and the three notes in
`research_notes/Browser Use 对话系统深度研究/`. This plan turns their recommendations
into one release. Six capabilities ship together; none is staged for a later version.

1. Process and conclusion organized per user message (`turnStarted` / `turnEnded`, `taskAssessment`).
2. Attached page vs acting page, and action effects (`browserActionTarget`, `actionObserved`).
3. Takeover and unknown-execution recovery (`control` state, `recoveryReview`, `recoveryTerminate`, `recoveryFork`).
4. First-class artifacts: screenshots, downloads, recordings (`artifactCreated` / `artifactReady` / `artifactUnavailable`).
5. Privacy minimization before anything reaches the journal.
6. Long-task progress and evidence continuity (`progressStalled`, `batchInterrupted`, `turnSummary`, `contextCompacted`).

## 0. Invariants this release must keep

Each is covered by an existing test or by a new acceptance check in section 14.

- The journal (`Store::event`, `conversation.rs:518-598`) stays the single source of truth: append-only, contiguous `sequence`, `after` cursor paging, gap and corruption refusal. No second event stream, no rewriting of old lines.
- Message UUID plus fingerprint dedupe (`Conversation::accept`, `conversation.rs:227-262`). New `Message` fields must not change the fingerprint of any message a current client can send.
- Queue, direct `Stop & send` (`accept_direct`, `terminal` at `conversation.rs:288-391`), queue pause after non-completed turns.
- Approvals: `dispatched` is not `confirmed`; `requestResolved` or a matching tool result releases a request (`service.rs:593-613`, `1386-1422`).
- Unknown-delivery protection in the side panel (`pendingMessages`, `sidepanel.js:427-473`) and `RecoveryRequired` refusing new messages and `begin_next`.
- Native document/ref invalidation and no blind replay of non-idempotent actions (`actions.rs:11809-11886`, `15865-15960`).
- Stealth: no page DOM markers, no `Runtime.enable`, no console capture, no continuous recording for progress UI. The backend prompt line forbidding them (`backend.rs:109`) stays verbatim.
- Old journals open. Anything a journal did not record is shown as "not recorded", never inferred.

## 1. Decisions that shape everything else

**D1. Browser evidence travels on a host side channel, not inside the backend's tool result.**
The chrome_use MCP server already runs inside the conversation's environment and calls the
service directly for panel tools (`panel_mcp_context` / `call_panel_mcp`, `agent_panel.rs:32-72`).
Every MCP browser tool call in panel context mints an `actionId`, and the MCP server reports
the evidence to the service itself before returning the tool result. The backend's tool event
only has to carry the `actionId` back for linking. Reasons: `short()` (`backend.rs:52-59`)
replaces any item over 24 KB with a 6000-character preview, so a screenshot result with its
base64 block always loses its structure; and no backend is verified to preserve
`structuredContent`. If a backend drops the `actionId`, the evidence stays as an unlinked
record. Linking is by `actionId` only, never by time or adjacency.

**D2. Acting-page identity comes from native in-memory state, stamped on every panel command.**
Today `observed.target` exists only on `--observe` replies (`actions.rs:3068-3086`), and no
MCP tool sets `observe`. The daemon will stamp an `actionContext` block (target before and
after) on every command that carries `_panelActionId`, read from `BrowserManager` state
(`observed_target()`, `browser.rs:7096`) plus the Chrome tab id from the relay session id
(`relay_chrome_tab_id`, `browser.rs:2834`). No CDP call, no new domain.

**D3. Observation is not turned on by default.** `--observe` costs a baseline and a post
snapshot, a settle wait, and turns on `Network.enable` (`enable_request_tracking`,
`actions.rs:20504`). Enabling it for every panel action would change latency and the page's
observable surface for a UI feature. Instead the acting MCP tools (click, fill, type, press, select,
hover, open, back, forward, reload) gain an optional
`observe` boolean that maps to the existing `--observe` flag. The prompt asks the agent to use
it on submit/save/pay steps. When used, `actionObserved` can cite `changed`, `noProgress` and
`humanCheck`. This is the same path a CLI user gets with `--observe`; it adds no
`Runtime.enable`, console capture, overlay or recording.

**D4. Task assessment is a tool call, not text parsing.** A new panel tool
`chrome_use_task` (action `assess`) records the agent's own verdict, bound server-side to the
caller's active message. The host validates cited evidence and downgrades unsupported claims.
No assessment means `unverified` (source `host`, reason `notAssessed`); no event at all
(old journals) means "not recorded".

**D5. Redaction happens once, in the service, before `Store::event`.** A `Redacted` newtype is
the only thing `Store::event` accepts, so no write path can bypass it. The in-memory approval
original stays unredacted for validation and for building the backend reply.

**D6. No `Conversation` struct changes.** `Conversation` is `deny_unknown_fields`
(`conversation.rs:130`); a new field would make state written by this build unreadable by an
older one. Everything new is either a new journal event kind, derived at load from the
journal, or a field on `Message` (which is not `deny_unknown_fields`). `schema_version` stays 1.

## 2. Journal contract

### 2.1 Envelope

`Event { sequence, at, kind, data }` is unchanged. Every new kind carries these fields in
`data`:

| field | meaning |
|---|---|
| `v` | event-data version, `1` for every kind added here |
| `messageId` | the turn it belongs to; `null` only when no message was active |
| `generation` | backend process generation the evidence came from |
| `privacyClass` | `public`, `pageContent`, `userInput` or `masked` (section 7) |
| `actionId` / `artifactId` | when the event is about one action or artifact |

`conversationId` is not repeated per event: the journal file is per conversation and
`conversationRead` returns it in its envelope. `sequence` is the journal's own.

Existing kinds keep their shapes and gain only optional fields: `tool` gains `toolCallId`,
`actionIds`, `privacyClass`, `redacted: true`; `textFinal` gains `role`
(`final` / `commentary`, absent when unknown); `ready` gains `pgid` and `processStartTime`;
`running` already carries `turnId`.

### 2.2 New kinds

```
turnStarted        {v, messageId, generation, direct, continuation: null|"afterStop"|"afterRecovery"}
turnEnded          {v, messageId, generation, outcome: "completed"|"failed"|"interrupted"|"unknown",
                    error, emptyAnswer, finalItemIds: [itemId], sourceSequences: [first, last]}
taskAssessment     {v, messageId, source: "agent"|"host", status: "verified"|"partial"|"unverified"|"blocked",
                    declaredStatus, reason, requirements: [{id, text, declared: "met"|"unmet"|"unknown",
                    computed: "met"|"unmet"|"claimed"|"unknown", evidence: [{kind:"action"|"artifact", id, verdict}]}],
                    downgradedBecause: [string], blocker: null|{kind, text}}
browserActionTarget{v, messageId, generation, actionId, tool, toolClass: "act"|"observe"|"capture",
                    requested: null|{tabHandle, url},
                    observed: null|{before: Target|null, after: Target|null},
                    outcome: "ok"|"error"|"timeout", errorCode, durationMs, resultDigest,
                    afterRecoveryRequired: bool}
                    Target = {chromeTabId|null, tabRef, targetId, url, title|null}
actionObserved     {v, messageId, actionId, verdict: "confirmed"|"failed"|"uncertain",
                    evaluator: "browser"|"host"|"none", basis: "expect"|"readBack"|"navigation"|"tabVerified"|
                    "download"|"capture"|"toolError"|"pageChanged"|"none", detail, possiblyExecuted: bool}
artifactCreated    {v, messageId, actionId, artifactId, kind: "screenshot"|"download"|"recording",
                    name, state: "pending"|"capturing"}
artifactState      {v, artifactId, state: "finalizing"}            (recordings only)
artifactReady      {v, artifactId, mime, declaredExtension, typeMatches: true|false|null, bytes,
                    digest: "sha256:<hex>"}
artifactUnavailable{v, artifactId, reason: "cancelled"|"interrupted"|"timeout"|"failed"|"tooLargeToRetain"|
                    "quotaExceeded"|"outsideAllowedRoots"|"missing"|"changed"|"encoderMissing",
                    detail, bytes|null, digest|null, detectedBy: "report"|"read"|"turnEnd"}
controlChanged     {v, messageId, from, to, reason}
progressStalled    {v, messageId, kind: "repeatedIdenticalResult"|"repeatedFailure"|"noActivity"|"nativeNoProgress",
                    count|null, quietMs|null, actionIds, sourceSequences}
progressResumed    {v, messageId, sourceSequence}
batchInterrupted   {v, messageId, outcome, completed: [toolCallId],
                    unresolved: [{toolCallId, name, actionId|null, executed: "reported"|"unknown"}]}
turnSummary        {v, messageId, sourceSequences: [first, last], steps, failedSteps,
                    actions: {confirmed, failed, uncertain, unlinked}, artifacts: {ready, unavailable},
                    assessment: status, stalls}
contextCompacted   {v, messageId, backend, trigger|null, sourceSequences: [first, last]}
recoveryTerminated {v, messageId, pid, confirmed, detail}
recoveryForked     {v, messageId, forkedTo}                         (original conversation)
recoveryContext    {v, fromConversationId, fromMessageId, fromSequence, outcome: "unknown",
                    possiblyExecuted: [{actionId, tool, target, verdict}], lastTargets: [Target],
                    artifacts: [{artifactId, name, kind}]}          (first event of the fork)
```

Bounds: every new event is built to stay under `MAX_EVENT` (64 KiB, `conversation.rs:18`):
strings capped (URL 2048, title 256, text 500, detail 1000), lists capped (requirements 12,
evidence per requirement 16, unresolved tools 64, possiblyExecuted 20). Over-cap lists record
`truncated: n`.

Evidence-only kinds (`browserActionTarget`, `actionObserved`, `artifact*`, `progress*`) are
journaled without the `state.json` rewrite that `Engine::record` does for other kinds
(`service.rs:552-559`). That is safe because `Store::load` already recomputes
`next_sequence` from the journal and only refuses state that is ahead of it
(`conversation.rs:489-496`).

## 3. Evidence path: native → MCP → agent_panel

### 3.1 Native (stamp the acting target)

- `cli/src/connection.rs` `send_command` (`2264-2310`): when env `CHROME_USE_PANEL_ACTION_ID`
  is a UUID, stamp `_panelActionId` on the command, next to `_cbSkip` / `_clickMode`.
- `cli/src/native/browser.rs`: add `pub fn acting_target(&self) -> Option<ActingTarget>`
  returning `observed_target()` fields plus `chrome_tab_id: relay_chrome_tab_id(&page.session_id)`
  and the cached page title if `PageInfo` has one. Pure field reads.
- `cli/src/native/daemon.rs` (`671-701`): if the command carries `_panelActionId`, take
  `acting_target()` before `execute_command_recovering`, again after, and insert
  `actionContext: {actionId, before, after}` into the reply. Top-level commands only; script
  and batch steps are not stamped.
- `cli/src/native/actions.rs` `handle_download` (`14103-14263`) and the extension
  download-url path: give the three failures machine codes in the reply (`code`:
  `download_canceled`, `download_timeout`, `download_interrupted`) beside the existing messages.
  Panel conversations are bound to the relay, so their downloads go through
  `extension_download_url` and `download-manager.js`, which throws
  `downloadUrl: interrupted (<reason>)`; `USER_CANCELED` maps to `download_canceled`, other
  reasons to `download_interrupted`, the extension's timeout to `download_timeout`. The CDP
  `canceled` state in the launched-browser loop maps to `download_canceled` as well.
- `relay_chrome_tab_id` (`browser.rs:2834`) becomes `pub(crate)`.

### 3.2 MCP (mint, report, return)

New module `cli/src/panel_evidence.rs`, used by `cli/src/mcp.rs` only when
`crate::agent_panel::panel_mcp_context()` is `Some`.

- `call_tool` (`mcp.rs:732-790`): for every chrome_use browser tool, mint `actionId`
  (UUIDv4), pass it to `run_cli` as env `CHROME_USE_PANEL_ACTION_ID` (add an `env` parameter to
  `run_tool` / `run_cli`, `mcp.rs:1781-1795`, `1982-2038`).
- After the CLI returns, `panel_evidence::derive(tool, arguments, &CliRun, parsed)` builds the
  report. Verdict rules (all from the native reply, nothing guessed), tool-specific rules first
  so an `expect` failure reads basis `expect`, not `toolError`:
  - Generic, after the tool-specific rules: `isError` → `failed`, basis `toolError`,
    `errorCode` from the reply's `code`.
  - CLI killed on timeout → outcome `timeout`, `uncertain`, `possiblyExecuted = !safe_to_repeat(action)`.
  - `chrome_use_expect` success → `confirmed`, basis `expect`; expect failure → `failed`.
  - `fill` / `type`: compare `readBack` with the requested value inside the MCP process;
    equal → `confirmed` / `readBack`; different → `failed`; masked or absent → `uncertain`.
    Only the boolean leaves the MCP process, never the read-back text.
  - `open` / `back` / `forward` / `reload`: `after.url` equals the requested URL after
    normalization → `confirmed` / `navigation`; a different origin → `uncertain` with the
    landing origin in `detail`.
  - `tabs select` with `verified: "confirmed"` → `confirmed` / `tabVerified`.
  - `download`, `download_url`, `screenshot`: `confirmed` / `download` or `capture` only after
    the service reports the artifact `Ready` (it answers synchronously, 3.3).
  - With `observe: true`: `observed.changed == true` → `uncertain` / `pageChanged` (a change is
    not the requested effect); `noProgress` present → feeds `progressStalled`
    (`nativeNoProgress`); `humanCheck` → `uncertain` and turn blocker hint.
  - Everything else that acts (click, press, select, hover, scroll, eval, upload) → `uncertain`,
    basis `none`, `possiblyExecuted = true`.
  - read, snapshot, extract, find, a11y, tabs list, downloads list → `toolClass: "observe"`,
    no verdict, `resultDigest` = SHA-256 of the result text with the marker and timing removed.
- Report with `crate::agent_panel::report_panel_evidence(report)`: a new function next to
  `call_panel_mcp` that uses a new `service::call_existing` (endpoint + health only, no
  `ensure()`, 2 s connect/answer budget for non-artifact reports, 30 s when artifacts are
  attached). A failed report never fails the tool; the MCP appends a text note
  "browser evidence was not recorded" so the transcript does not imply evidence exists.
- Tool result: prepend a text block `chrome-use action <actionId>` as the first content block
  and add `structuredContent.chromeUseAction = {actionId}`. First position survives any later
  truncation in the adapters; the adapters extract it before `short()` anyway (3.4).
- New tool arguments: optional `observe` on the acting tools listed in D3
  (`core_tools` / `extended_tools`, `mcp.rs:315-676`), mapped to `--observe`. Non-panel MCP
  clients see the same optional argument; behaviour without it is unchanged.
- New panel tools in `panel_tools` (`mcp.rs:282-289`) and `is_known_tool` (`686-691`):
  `chrome_use_task` (`assess`, section 4.3) and `chrome_use_record` (`start` / `stop`,
  section 6.4).

### 3.3 Service ops for the side channel

Added to `agent_panel.rs` `handle_for_profile` (`574-630`) as agent-only operations reached
through `call_panel_mcp`/`report_panel_evidence`, to `service::call`'s existing-service list
(`service.rs:456-470`) so a tool call can never bootstrap a second service, and to
`operation_seconds` (`service.rs:151-158`).

- `agentActionReport {callerConversation, generation, actionId, tool, arguments, outcome,
  errorCode, actionContext, verdict, resultDigest, durationMs, artifacts: [{kind, path, name}]}`
  (30 s). Under the engine lock: check the caller exists, `actionId` is a UUID not yet seen in
  this conversation (dedupe set rebuilt from the journal at load), and take `messageId` from
  `chat.active`. If `generation` is not the live job's generation or the conversation is
  `RecoveryRequired`, still record it with `afterRecoveryRequired: true`: a late action from an
  unconfirmed process is exactly the evidence recovery needs. Release the lock to copy and hash
  artifacts (section 6), re-take it to journal `browserActionTarget`, `actionObserved` and the
  artifact events. Reply `{artifacts: [{artifactId, state}]}` so the MCP can finish the verdict.
- `agentTaskAssess` (10 s), `agentRecordingAuthorize` (10 s): sections 4.3 and 6.4.

### 3.4 Backend adapters (link tool events to actions)

In each adapter, before `short()`: extract `toolCallId` and any `chrome-use action <uuid>`
markers / `chromeUseAction.actionId` from the raw item, strip `image` blocks' `data`
(keep `{type:"image", mimeType, bytes}`), then emit
`notice("tool", {completed, toolCallId, actionIds, item: short(item)})`.

- Codex, `backend.rs:407-408`: `item.id`; markers in `item.result.content` / `structuredContent`.
- Claude, `backend.rs:471-472`: `tool_use.id` / `tool_result.tool_use_id`; markers in `content`.
- Pi, `backend.rs:666-671`: `toolCallId`; markers in `result.content`.
- OpenCode ACP, `acp.rs:132-149`: `toolCallId`; markers in `rawOutput` / `content`.

Only items whose tool name is a chrome_use tool are scanned
(`^(?:mcp__chrome_use__)?chrome_use[_.][a-z_]+$`, mirroring `browserTool` in `panel-state.js`),
so a shell transcript that happens to print a UUID cannot claim an action. An `actionId`
claimed by two tool items keeps the first link; the second is recorded as `duplicateClaim`.

Also in the adapters:
- Codex `agentMessage`: pass `item.phase` through as `role` when the CLI emits it
  (`final_answer` → `final`, `commentary` → `commentary`); otherwise no role.
- Claude `result` (`backend.rs:474`): when `result` is a string, emit
  `notice("textRole", {itemId: claude_message, role: "final", source: "claude.result"})`.
- Compaction: Codex `contextCompaction` item and `thread/compacted`; Claude
  `system` / `compact_boundary`; Pi `auto_compaction_end` → `notice("contextCompacted", {...})`.
  OpenCode ACP: none.
- `ready`: add `pgid` and `processStartTime` read right after spawn in `drive`
  (`backend.rs:224-228`; macOS `proc_pidinfo(PROC_PIDTBSDINFO)`, Linux
  `/proc/<pid>/stat` field 22, Windows `GetProcessTimes`).
- `Setup` gains `generation`; `drive` adds `CHROME_USE_PANEL_GENERATION` to `browser_env`
  (`backend.rs:97`) so the MCP can report it.

### 3.5 Prompt (`backend.rs:109`)

Append, keeping every existing sentence:
"Each chrome_use browser result starts with `chrome-use action <id>`. Pass `observe: true`
when an action should submit, save, pay or send, then check its effect. Before your final
answer call `chrome_use_task` with action `assess`: list each thing the user asked for, mark it
met, unmet or unknown, and cite the action or artifact ids that show it. Do not mark anything
met that you did not observe. If you cannot continue, assess as blocked and say why."

## 4. Turns, final text and assessment

### 4.1 Turn boundaries

- `Engine::begin(id) -> Option<Message>` wraps `chat.begin_next()` and journals `turnStarted`
  in the same lock. All four call sites switch to it: `notice` (`service.rs:714`),
  `conversationSend*` (`1200`), `conversationResumeQueue` (`1362`), `run_schedule` (`2187`).
  `run_schedule` records its `user` event after `begin_next` (`2202`); it moves the `user`
  record ahead of `Engine::begin` so every journal has `user` before `turnStarted`. The
  projection still creates a turn on whichever of the two it sees first, for robustness.
- `turnEnded` is journaled in `Engine::notice` right after the `terminal`, `exited` or
  `processUnconfirmed` record: outcome from the terminal status, `failed`/`interrupted` from
  `exited` (as `chat.terminal` already decides, `service.rs:675-696`), `unknown` for
  `processUnconfirmed`. `emptyAnswer` is true when no `textFinal` with this `messageId`
  carried non-whitespace text. `finalItemIds` lists text items with `role: "final"`; empty when
  the backend does not say.
- On `turnEnded`, in order: `batchInterrupted` if any open tool (4.4), a host
  `taskAssessment` if the agent did not assess, `turnSummary`, and the recording stop (6.4).

### 4.2 Message continuation (no auto-redo)

`Message` gains `continuation: Option<Continuation>` (`serde(default, skip_serializing_if)`).
It is set in `Engine::begin` when `begin_next` pops the message, not at accept: a message
queued during a running turn is accepted before that turn's outcome is known. `afterStop` when
the conversation's last `turnEnded` outcome was `interrupted` (this includes the direct message
of Stop & send, which only runs after the confirmed interrupt); `afterRecovery` for the first
message of a fork. It is excluded from `fingerprint()` because the host derives it after
acceptance. `edit_queued` (`conversation.rs:332-334`) carries `record` (6.4) into the
replacement with attachments, page and skill; `continuation` is never set on queued messages.
`prompt_text()`
(`conversation.rs:64-72`) prepends:

- `afterStop`: "[The user stopped the previous request and may have changed the pages. Observe
  the current page before acting. Do not repeat actions from the stopped request unless this
  message asks for them.]"
- `afterRecovery`: the fork preface in 5.4.

### 4.3 `chrome_use_task` → `agentTaskAssess`

Arguments: `status`, `requirements: [{text, status, evidence: [id]}]` (≤12, ≤16 ids each),
optional `blocker {kind: "humanCheck"|"login"|"permission"|"missingInput"|"siteError"|"other", text}`.
The service binds it to `chat.active` (refused with no active message or a stale generation)
and computes, per requirement:

- `met` only if a cited action has `actionObserved.verdict == "confirmed"` with evaluator
  `browser`, or a cited artifact is `Ready` (a ready artifact proves existence and type, not
  that it satisfies the request; the card says so).
- `unmet` if the agent says so, or every cited action is `failed`.
- `claimed` if the agent says met without such evidence; `unknown` for unknown ids.

Status: `blocked` if the agent declares a blocker; `verified` only if every requirement is
`met` and the turn outcome is `completed`; `partial` if at least one is `met` (or the agent
declared partial); otherwise `unverified`. Any lowering is listed in `downgradedBecause`.
Evidence ids may come from earlier turns of the same conversation; never from another
conversation. Compaction does not change any of this: verdicts come from journaled evidence,
not from the model's memory, so an `uncertain` action stays uncertain after `contextCompacted`.

### 4.4 Open tools and `batchInterrupted`

`Engine` keeps `open_tools: HashMap<conversation, BTreeMap<toolCallId, OpenTool>>` from the
`tool` notices (started without completion). At `turnEnded`, any left open produce
`batchInterrupted`; `executed` is `reported` when a `browserActionTarget` for its `actionId`
exists, else `unknown`. This covers Claude's several `tool_use` blocks in one message and
Codex's parallel items when a stop lands mid-batch. The native CLI `batch` command is not
reachable from the panel MCP surface, so it has no producer here.

## 5. Control, takeover and recovery

### 5.1 Control state

Derived in `Conversation::summary` (`conversation.rs:213-215`) as `control`, no struct field:

| phase / condition | control |
|---|---|
| `Starting` with active `continuation` | `resuming` |
| `Starting`, `Running`, `WaitingInput` | `agent` |
| `Stopping` | `stopRequested` |
| `Idle`, `Interrupted`, `Failed` | `human` |
| `RecoveryRequired` | `unknown` |

`controlChanged` is journaled when it changes. `human` is only reached through a confirmed
terminal (`terminal`/`exited` with exit confirmed); a stop that times out goes to `unknown` if
`processUnconfirmed`, so the UI never says "you have control" before the process confirmed it.

### 5.2 Approvals during stop

- `approve_pending` (`service.rs:503-527`) skips conversations in `Stopping`; for full-access
  conversations a request arriving during stop is answered with the method's decline/cancel.
- `conversationRespond` (`1386-1422`) in `Stopping` accepts only decline/cancel results.
- Both close the window where an approval could start an action after the user pressed Stop.

### 5.3 Process record

The last `ready` without a later confirmed `exited` gives `{generation, pid, pgid,
processStartTime}`; it is rebuilt from the journal at load (no sidecar, no struct field).
Liveness probe (`agent_panel/recovery.rs`, new):
- Unix: `kill(-pgid, 0)`; `ESRCH` = gone. If the leader still exists, its start time must
  match before it is treated as ours (PID reuse).
- Windows: the backend job uses `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
  (`windows.rs:70`), so the tree died with the old service; the leader PID is checked with
  `OpenProcess` + creation time. In-lifetime cases keep using `confirm_empty`.

### 5.4 Operations

- `conversationRecoveryReview {conversationId}` (15 s, read-only, journals nothing): phase,
  last error, the active message, process `exited|alive|unknown`, the turn's actions with
  verdicts and `possiblyExecuted`, unresolved tools, artifacts, last observed targets. It does
  not touch the browser.
- `conversationRecoveryTerminate {conversationId}` (15 s): only in `RecoveryRequired` with an
  alive, identity-matched process. `SIGKILL` to the group (Windows: `TerminateProcess` on the
  identity-matched leader), wait up to 5 s for `ESRCH`, journal `recoveryTerminated`.
- `conversationRecoveryFork {conversationId}` (10 s): only when the probe says `exited`.
  Creates a new conversation (same agent, requested model, options, executable, full access,
  `site` so it stays in the per-site list; no backend session, so nothing is resumed mid-turn), journals `recoveryContext` as its first
  event and `recoveryForked` in the original. The original stays `RecoveryRequired` and
  read-only. The fork does not send anything; its first user message gets
  `continuation: afterRecovery`, whose preface lists up to 20 possibly executed actions (tool,
  redacted target, verdict) and the last observed tabs as `chrome-tab:<id>` handles:
  "[This conversation continues one whose last request ended with an unknown outcome. These
  actions may already have taken effect: … Observe the pages before acting. Do not redo them
  unless this message asks you to.]"

The delivery-unknown path (`pendingMessages`, `conversationMessageStatus`) is unchanged; the
panel shows it on the turn card when the message has a `user` row, otherwise in the existing
`pendingRecovery` section, and its text keeps saying that clearing the warning does not cancel
execution.

## 6. Artifacts

### 6.1 Store

New `cli/src/agent_panel/artifacts.rs`, modelled on `attachments.rs`:
- Blobs: `<conversations>/<id>/artifacts/<sha256>.<ext>`, outside `workspace` (the agent's cwd),
  private tree, `atomic_write`, Windows `protect`.
- Index: in memory, rebuilt from the journal's artifact events at load, keyed by `artifactId`.
- Caps: screenshot 16 MiB, download 100 MiB, recording 256 MiB, 1 GiB per conversation.
  Over a per-file cap: hash while streaming, keep no copy, `artifactUnavailable
  {reason: "tooLargeToRetain", bytes, digest}`. Over the conversation quota: `quotaExceeded`.
- `conversationDelete` already removes the whole conversation folder.

### 6.2 Ingestion (`agentActionReport`)

- Source path must be absolute after canonicalization, a regular file (not a symlink), owned
  by the current user, and either under the conversation workspace, the screenshot directory
  (`screenshot.rs:652-656`), or the exact `path` the native reply returned for this action.
  Anything else: `outsideAllowedRoots`, name kept, no copy.
- Copy and hash in one pass, outside the engine lock, during the MCP call. A second download to
  the same name runs as a later tool call, so it cannot overwrite the first before its copy;
  same-name files become two artifacts with different digests.
- Type: magic-byte sniffing (PNG, JPEG, WebP, GIF, PDF, ZIP/OOXML, WebM, MP4, HTML, plain
  text). `typeMatches` compares with the file extension; an HTML error page saved as `.pdf`
  becomes `typeMatches: false`. Zero bytes is `Ready` with `bytes: 0` and the card says empty.
- Failures from the native codes: `download_canceled` → `cancelled`, `download_interrupted` →
  `interrupted`, `download_timeout` → `timeout`.

### 6.3 Restricted read

`conversationArtifactRead {conversationId, artifactId, offset, length}` (10 s). The artifact
must be in that conversation's index (cross-conversation ids are refused with the same error as
unknown ids). Chunks of at most 384 KiB raw, so base64 plus envelope stays under the service
frame (`MAX_FRAME` 768 KiB, `service.rs:23`) and the native-messaging reply limit. The final
chunk call re-hashes the blob; a missing or changed blob returns `artifact_unavailable` and
journals `artifactUnavailable {detectedBy: "read"}` once. The panel never receives a path.

### 6.4 Recordings

- Consent per message: the composer gets a "Record this task" toggle, sent as `record: true`
  in `conversationSend` (`Send` struct, `service.rs:753-764`), stored as `Message.record`
  (in `fingerprint()` only when true, so existing IDs are unchanged).
- `chrome_use_record start` → `agentRecordingAuthorize` refuses unless the active message has
  `record`, else returns `{artifactId, path: <conversation>/artifacts/incoming/<artifactId>.webm}`
  and journals `artifactCreated {state: "capturing"}`. The MCP runs the existing
  `record start <path>` (`commands.rs:3501-3523`).
- `chrome_use_record stop`, or the host at `turnEnded` if still capturing (runs
  `chrome-use record stop` for the `panel-<id>` session the way `close_browser_session` runs
  `close`, `service.rs:1862-1897`): `artifactState {finalizing}`, then ingest →
  `artifactReady`, or `artifactUnavailable` (`encoderMissing` when ffmpeg is absent,
  `recording.rs:135-140`). A finished answer never implies a ready recording.
- Recording is never started for progress display; no message flag, no recording.

## 7. Privacy minimization

### 7.1 Location

New `cli/src/agent_panel/redact.rs`. `Store::event` changes its `data: Value` parameter to
`data: Redacted` (constructed only by `redact::event`). `Engine::record` calls
`redact::event(kind, data, &mut self.secrets[id])` for every write, including `user`,
`queueEdited`, `request` and all new kinds. `conversationRead` applies `redact::legacy` to
events without `v` / `redacted` (display and export minimization for old journals; the file
is not rewritten).

### 7.2 Rules

Schema-driven by event kind, tool name and field, not by keyword-blacking all text:

- Field values masked as `{"redacted": true, "kind": "<k>", "length": n}` (no hash; a hash of
  a short password is guessable):
  - `fill`/`type` `value`/`text` when the selector, label or name matches the sensitive lists
    in `native/sensitive.rs` (made `pub(crate)` and shared, not copied), when the command is
    `secret`, or when the value has a secret shape.
  - Argument or JSON keys: `password`, `passwd`, `pwd`, `secret`, `token`, `apiKey`,
    `api_key`, `authorization`, `cookie`, `set-cookie`, `otp`, `cvv`, `cvc`.
  - Headers: `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`,
    `x-auth-token`, `x-csrf-token` (network_route arguments, results).
- URLs everywhere (arguments, results, targets, `PageContext.url`): userinfo removed; query and
  fragment values masked for `token`, `access_token`, `id_token`, `refresh_token`, `code`,
  `key`, `api_key`, `apikey`, `sig`, `signature`, `secret`, `password`, `session`,
  `sessionid`, `sid`, `auth`, `jwt`, `otp`, `ticket`, `state`, `X-Amz-Signature`,
  `X-Amz-Credential`, `X-Amz-Security-Token`, `X-Goog-Signature`. `PageContext.url` is
  redacted in `conversationSend` before the `Message` is built, so the fingerprint, state,
  journal and prompt all see one deterministic value; the agent still has the exact tab id.
- Secret shapes in free text: JWT, `Bearer <token>`, `sk-…`, `gh[pousr]_…`, `xox[abpr]-…`,
  `AKIA…`, PEM private keys, cookie strings (`name=value; …` with values of 8+ characters).
- Known secrets: every value masked above is added to a per-conversation in-memory set (≤64
  values, length ≥4) and scrubbed as an exact substring from every later event: tool results,
  approvals, assistant text, assessments.
- Streaming text: a per-item `TextScrubber` holds back the trailing partial word (bounded 512
  characters; an unbroken run longer than that is replaced by `[long token omitted]`), so a
  secret split across `textDelta` chunks is still caught. `textFinal` is scrubbed whole. The
  held-back tail is flushed, scrubbed, as a final `textDelta` on `terminal`, `exited` and
  `processUnconfirmed`; OpenCode ACP only ever sends `agent_message_chunk` deltas
  (`acp.rs:129-131`), so without the flush its last word would never be written.
- Free text caps: eval result 2000 characters, command output 4000, page text in results 6000
  (existing preview size), each after scrubbing.
- Image blocks: data removed before `short()`; pixels live only in artifacts.
- Kept on purpose, because approvals and review need them: URL host and path, amounts and
  numbers, recipient and destination fields that are not secret keys, exit codes, error codes,
  tool names, durations, digests, sizes, tab ids.
- Not redacted: the user's own message text (it is the user's record and is already in the
  CLI's own session files). Assistant text is redacted only by the known-secret and shape
  rules above. Documented in the panel's privacy note; the backend CLIs' own session files are
  outside this service's control.

`privacyClass` is the strongest class present: `masked` if anything was masked, else
`userInput` for fill/type values, `pageContent` for page-derived text, else `public`.

### 7.3 Approval reply substitution (highest-risk change)

The panel builds `can_use_tool` replies from the journal's `params.input`
(`sidepanel.js:369-371`, `345`). Once the journal holds redacted input, echoing it would send
`"[redacted]"` to Claude as `updatedInput`. Change:
- `Engine.requests` keeps the unredacted original (as now); the `request` event is redacted.
- Panel replies `{behavior: "allow"}` without `updatedInput` (and for AskUserQuestion,
  `{behavior: "allow", answers}`).
- `conversationRespond` builds `updatedInput` from the stored original (merging `answers` for
  AskUserQuestion) before `validate_reply` (`service.rs:1464-1544`) and dispatch.
  `automatic_approval` already uses the original.
- ACP replies only carry an `optionId` validated against the original options; Codex replies
  carry decisions. Neither echoes input.

## 8. Progress and continuity

New `cli/src/agent_panel/progress.rs`; advisory only (like `native/progress.rs`): it never
stops, retries or changes success.

- `repeatedIdenticalResult`: 4 consecutive chrome_use actions in one turn with the same
  fingerprint = (tool, canonical arguments without `timeoutMs`/`session`, `after.targetId`,
  redacted `after.url`, `resultDigest`). Paginated reading changes the result digest, so it
  never counts; `chrome_use_wait` is excluded.
- `repeatedFailure`: 3 consecutive failed chrome_use actions with the same tool and `errorCode`.
- `nativeNoProgress`: a report carrying native `noProgress` (only with `observe: true`).
- `noActivity`: `Running`, no journaled backend event for 180 s, no pending request, no open
  tool still inside its own `timeoutMs` + 30 s. Checked by a `progress_tick` added to the
  service's 1 s scheduler loop (`service.rs:1786-1795`).
- One `progressStalled` per episode; the next action with a different fingerprint, or any new
  backend event for `noActivity`, journals `progressResumed`.
- `turnSummary` (4.1) records `sourceSequences` and counts only from journaled events; it
  replaces nothing in the journal. `contextCompacted` records the backend signal and the
  sequence range since the previous compaction or turn start; the UI notes "The agent's
  memory was summarized here; the history below is complete."

## 9. Side panel

### 9.1 Projection (`panel-state.js`)

`PanelTimeline` keeps `cursor`, `rows`, `items`, `requests` and the gap/order checks, and adds
`turns: Map<messageId, Turn>` plus `turnOrder`:

```
Turn {messageId, user: row, attachedPage, state: "queued"|"running"|"ended"|"legacy",
      outcome, emptyAnswer, steps: [stepKey], texts: [itemKey], roles: Map<itemKey, role>,
      actions: Map<actionId, {target, observed, linkedStep}>, actingPages: [Target],
      artifacts: Map<artifactId, Artifact>, assessment, stalls, batchInterrupted,
      compactions, control, recovery, legacy: {grouping, verification, pages}}
```

- A turn exists from its `user` event (state `queued`), starts at `turnStarted`, ends at
  `turnEnded`. `queueEdited` moves the turn to `replacementId` (as rows do today), so replay
  gives one turn. `tool`/`text` events join the turn named by their `messageId`.
- Evidence and tool events merge in either order: a `browserActionTarget` creates the action;
  a `tool` event with `actionIds` links it; whichever arrives second completes the link.
- Legacy journals: no `turnStarted` → the turn starts at the first event with its
  `messageId`; events without `messageId` (oldest journals) attach to the preceding user row
  with `legacy.grouping = true` and the card says "Step grouping was not recorded". No
  `taskAssessment` → "Verification not recorded". chrome_use steps without actions → "Page
  not recorded". No artifact cards are created from paths in tool text.
- Rendering-only: no field is filled from text heuristics.

### 9.2 Turn card (`sidepanel.js`, `sidepanel.css`, `sidepanel.html`)

`renderTimeline` (`sidepanel.js:220-280`) renders turns instead of a flat row list, keeping
the keyed incremental DOM (`rendered`), scroll-follow (`follow`, `latest`) and per-group
`touched` state. Per card:

1. User message, attachments (existing `conversationAttachmentRead` images), and
   **Sent with**: the attached page's title and host (from `user.page`).
2. **Acted on**: distinct `after` targets from the turn's actions, first three plus "+n";
   `chromeTabId` equal to the attached tab says "the page you sent"; a requested/observed
   mismatch says "asked for tab X, worked on tab Y". Pages the user only switched to never
   appear, because nothing but evidence writes this line.
3. Steps: the existing `toolGroup` now scoped to the turn (not to adjacency). Each step shows
   its action verdict chip (Confirmed / Failed / Not checked / No browser record). A folded
   group lists failed step names in its summary, so a failure is never hidden by folding.
   Unresolved steps from `batchInterrupted` say "did not finish; may have run" or "did not
   run".
4. Answer: assistant text through `renderMarkdown`; with `role` known, commentary is shown
   smaller above the final answer; without it, all text is "Response". `emptyAnswer` shows
   "The agent returned no answer text."
5. Result cards per artifact: screenshot thumbnail (chunked read → `Blob` → object URL,
   revoked on removal, SHA-256 checked with `crypto.subtle` before display); download name,
   size, type and "matches .pdf: yes/no"; recording with capturing/finalizing/ready. Three
   lines, separately: "File exists", "Type matches", "Meets the request: per assessment / not
   assessed". Unavailable cards give the reason. Save uses an `<a download>` on the blob.
6. Assessment badge: Verified by browser checks / Partly done / Not verified / Blocked /
   Verification not recorded. Expanding lists requirements, computed status, cited evidence,
   and `downgradedBecause`.
7. Outcome line: Completed / Failed / Interrupted / Outcome unknown; stall banner while
   `progressStalled` is open (Stop, Keep going (local dismiss), no automatic action).
8. Recovery block on the unknown-outcome turn: "Review what may have happened"
   (`conversationRecoveryReview`), "End the old agent process" when alive
   (`conversationRecoveryTerminate`), "Continue in a new conversation" only when the review says
   exited (`conversationRecoveryFork`, then `select(newId)`). Forked conversations show a link
   both ways.

The status line (`#status`) shows `control` alongside the phase: "Agent working", "Stop
requested, waiting for confirmation", "You have control", "Resuming", "Agent state unknown".
Approval cards (`requestView`, `333-379`) disable Allow while `control` is `stopRequested`
and send the new reply shapes (7.3).

### 9.3 Bridge (`agent-panel-bridge.js`)

Add `conversationArtifactRead`, `conversationRecoveryReview`, `conversationRecoveryTerminate`,
`conversationRecoveryFork` to the allowlist, `operationTimeouts` (12 s, 17 s, 17 s, 12 s) and
`validResult` (artifact chunk: matching `artifactId`, integer `offset`, string `data`,
`total`, `digest`; review: `process.state` in `exited|alive|unknown`; terminate: boolean
`confirmed`; fork: new `conversation.id` and `forkedFrom === conversationId`). The agent-only
ops are not in the panel allowlist. `capabilities()` (`agent_panel.rs:570-572`) adds
`turnEvidence: 1`, `artifacts: 1`, `recovery: 1`, `recording: 1`; the panel renders the legacy
view when they are absent (old CLI with new extension).

## 10. Migration and compatibility

- Old journals: no rewrite. New kinds are additive; `PanelTimeline.apply` already ignores
  unknown kinds while advancing the cursor, so an older panel on a newer journal still works.
- Old CLI with new panel: capability flags absent → legacy rendering, no new ops sent.
- New CLI with old panel: new events ignored; approval replies from an old panel still carry
  `updatedInput`; `conversationRespond` accepts that shape only when it equals the redacted
  projection of the original, then substitutes the original (an old panel never sends
  `[redacted]` to the agent).
- `Conversation` unchanged; `Message` gains `continuation` and `record`, both skipped when
  empty, `record` in the fingerprint only when true.
- Process records, evidence dedupe sets and artifact indexes are rebuilt from the journal in
  the same pass `Store::load` already makes over it (`last_sequence`, `conversation.rs:692-728`).
- Running old service: `ensure()` replaces idle services of another build and refuses while
  one is active (`service.rs:211-298`); the panel's capability flags come from the running
  host, so a mixed state shows the legacy view until the new service is live.

## 11. Backend capability matrix

"Host" means the service produces it independently of the backend. Items marked *verify*
are confirmed in implementation step 1 against the CLI versions on the build host by
capturing raw protocol fixtures; the code reads fields and never sniffs versions, and the UI
shows "unknown" whenever an event lacks the field.

| capability | Codex app-server | Claude Code stream-json | Pi JSONL RPC | OpenCode ACP |
|---|---|---|---|---|
| Turn start/end per message | host | host | host | host |
| Backend turn id | `turn/started` id | none (null) | none (null) | prompt request id |
| Terminal outcome | `turn/completed.status` | `result.subtype`/`is_error` | `agent_settled` + `stopReason` | `stopReason` |
| Tool call id | `item.id` | `tool_use.id` / `tool_use_id` | `toolCallId` | `toolCallId` |
| Tool error flag | `status`/`error` | `is_error` | `isError` | `status: failed` |
| actionId survives in tool result | *verify* (`content`, `structuredContent`) | *verify* (`content`) | *verify* (`result.content`) | *verify* (`rawOutput`/`content`) |
| Browser target and effect | host (MCP side channel) | host | host | host |
| Artifacts | host | host | host | host |
| Final vs commentary text | `agentMessage.phase` *verify*; else unknown | `result.result` → final | unknown | unknown |
| Context compaction signal | `contextCompaction` item / `thread/compacted` *verify* | `system/compact_boundary` *verify* | `auto_compaction_end` *verify* | unknown |
| Approval requests | yes | yes (`can_use_tool`) | none (Pi never asks) | yes |
| Approval resolution confirm | `serverRequest/resolved` | matching `tool_use_id` result | n/a | tool completion / prompt end |
| Interrupt confirmation | `turn/completed interrupted` | `result` while stopping | `agent_settled aborted` | `stopReason cancelled` |
| Task assessment | `chrome_use_task` (agent compliance) | same | same | same |
| Recording | host via `chrome_use_record` | same | same | same |
| Exit confirmation | host process group / job | host | host | host |

If the actionId does not survive for a backend, its browser evidence still appears in the turn
(the report carries `messageId`) as "browser record not matched to a step", and steps show
"no browser record". No adjacency matching.

## 12. Files touched

Rust (`cli/src/`):
- `agent_panel/redact.rs` (new): `Redacted`, `PrivacyClass`, `Secrets`, `TextScrubber`, `event`, `legacy`, `url`.
- `agent_panel/artifacts.rs` (new): store, ingest, sniff, read chunk, index rebuild.
- `agent_panel/recovery.rs` (new): process record, probe, terminate, fork summary.
- `agent_panel/progress.rs` (new): stall detector and tick.
- `agent_panel/conversation.rs`: `Message.continuation`, `Message.record`, `prompt_text`, `fingerprint`, `summary.control`, `Store::event(Redacted)`, load-time rebuild hooks.
- `agent_panel/service.rs`: `Engine::begin`, `turnEnded` and follow-ups in `notice`, `record` split (evidence kinds skip state save), `approve_pending` and `conversationRespond` stop rules, reply substitution, new ops, `call_existing`, `operation_seconds`, `progress_tick`, recording stop at turn end.
- `agent_panel/backend.rs`: tool notice `toolCallId`/`actionIds`/image stripping, Claude `textRole`, Codex `phase`, compaction notices, `ready.pgid/processStartTime`, `Setup.generation`, env `CHROME_USE_PANEL_GENERATION`, prompt.
- `agent_panel/acp.rs`: tool notice fields.
- `agent_panel.rs`: `report_panel_evidence`, panel tool routing for `chrome_use_task`/`chrome_use_record`, new ops in `handle_for_profile`, `capabilities()`.
- `panel_evidence.rs` (new) and `mcp.rs`: actionId, env to CLI, verdict derivation, report, marker, `observe` argument, panel tools.
- `connection.rs`: `_panelActionId`.
- `native/daemon.rs`, `native/browser.rs`: `actionContext`, `acting_target`.
- `native/actions.rs`: download error codes.
- `native/sensitive.rs`: share the sensitive lists `pub(crate)`.

Extension (`extensions/ab-connect/`): `panel-state.js` (turn projection), `sidepanel.js`
(turn cards, result cards, recovery, control line, approval reply shapes, record toggle),
`sidepanel.html` (record toggle, status control span), `sidepanel.css` (turn card, chips,
result cards, banners), `agent-panel-bridge.js` (ops, timeouts, validators).

Docs: `docs/` panel page (privacy note, what "Verified" means, recording consent); `CHANGELOG`.

## 13. Implementation order (one release, one branch)

Each step lands with its unit tests; the release is cut only after section 14 passes on the
build host.

1. **Protocol fixture capture on the build host** (`leo@192.168.0.190`): run each installed
   backend once with a chrome_use MCP that returns a marker block; save raw protocol lines as
   test fixtures under `cli/src/agent_panel/fixtures/`. Settles the *verify* cells of
   section 11 before anything depends on them.
2. **Redaction gate**: `redact.rs`, `Store::event(Redacted)`, legacy read redaction, approval
   reply substitution, stop-time approval rules. Everything after this writes through it. The
   existing `Store::event` tests (`conversation.rs:811-1057`) move to a test-only `Redacted`
   constructor.
3. **Turn lifecycle**: `Engine::begin`, `turnStarted`, `turnEnded`, `toolCallId`, open tools,
   `batchInterrupted`, `emptyAnswer`, `continuation`, `control` in summary, `textRole`/`phase`.
4. **Native stamping**: `_panelActionId`, `acting_target`, `actionContext`, download codes.
5. **MCP side channel**: `panel_evidence.rs`, report op, `call_existing`, marker,
   `observe` argument, adapter extraction, `browserActionTarget`/`actionObserved`.
6. **Artifacts**: store, ingestion in the report op, chunked read, recordings and consent.
7. **Assessment**: `chrome_use_task`, `agentTaskAssess`, host default, prompt text.
8. **Recovery**: process record and probe, review/terminate/fork, fork preface.
9. **Progress and continuity**: detector, tick, `turnSummary`, `contextCompacted`.
10. **Side panel**: bridge, projection, turn cards, result cards, recovery UI, legacy view.
11. **Acceptance on the build host**, docs, release notes, release.

## 14. Acceptance

### 14.1 Unit tests (per rule)

Rust, in the module that owns the rule:
- `redact`: each masked key; each header; each URL parameter including fragments and userinfo;
  each secret shape; cookie strings; known-secret exact scrub across later events; streaming
  scrub of a secret split across 2 and 3 `textDelta` chunks; long unbroken run; caps; kept
  fields (amount, recipient, host, path, exit code) survive; `privacyClass` per case;
  `Store::event` cannot be called with a raw `Value` (compile-time, plus a doc test);
  legacy read redaction leaves the file bytes unchanged.
- Approval substitution: allow with no `updatedInput` sends the original; AskUserQuestion merges
  answers into the original; an old-panel reply equal to the redacted projection is accepted
  and substituted; any other `updatedInput` is refused; ACP and Codex shapes unchanged.
- Stop rules: request during `Stopping` with full access is declined, not approved;
  `conversationRespond` accept during `Stopping` is refused, decline passes.
- Turns: each of the four `begin_next` sites journals exactly one `turnStarted`; `turnEnded`
  outcome for completed, failed, interrupted, exited-while-stopping, processUnconfirmed;
  `emptyAnswer`; `batchInterrupted` with reported vs unknown execution; continuation set in
  `Engine::begin` after an interrupted turn (including a message queued before the interrupt
  and the Stop & send direct message) and not after completed; `edit_queued` keeps `record`;
  schedule journals `user` before `turnStarted`; scrubber tail flushed on each terminal kind
  (ACP delta-only fixture); fingerprint of messages without new fields unchanged (golden
  hashes from `70bb31f7`).
- Download codes: relay `downloadUrl: interrupted (USER_CANCELED)` → `download_canceled`,
  other reasons → `download_interrupted`, extension timeout → `download_timeout`, CDP
  `canceled` → `download_canceled`.
- Evidence: verdict derivation table (every rule in 3.2), timeout → possiblyExecuted for
  non-idempotent actions; duplicate `actionId` refused; stale generation recorded with
  `afterRecoveryRequired`; report failure leaves the tool result successful with the note;
  adapters extract actionIds from each backend fixture (step 1) and ignore markers in non
  chrome_use items; image data stripped before `short()`.
- Native: `actionContext` present only with `_panelActionId`, no CDP traffic added (assert on
  the mock client's sent-command log), chromeTabId from `cb-tab-<id>` and null when launched.
- Assessment: verified needs all requirements met by browser-confirmed evidence; claimed-only
  becomes unverified; partial; blocked; cross-conversation evidence ids unknown; compaction
  between an uncertain action and an assess keeps it unverified; non-completed outcome caps at
  partial.
- Artifacts: content addressing; two same-name sources → two artifacts; caps →
  `tooLargeToRetain` with digest; quota; symlink and outside-root refusal; sniffing incl. HTML
  saved as PDF and zero bytes; chunk bounds; final chunk digest check; deleted blob → one
  `artifactUnavailable`; cross-conversation read refused; recording refused without
  `record`; encoder missing.
- Recovery: probe with ESRCH, live group, PID reuse (start-time mismatch treated as gone for us
  but not killed); terminate only identity-matched; fork refused while alive or unknown; fork
  creates no queued message and no backend session; preface content bounded.
- Progress: four identical reads stall; six paginated reads with differing digests never stall;
  `wait` excluded; repeated failure; noActivity suppressed by an open long tool and by a pending
  approval; resumed episode.
- Store: new event kinds within `MAX_EVENT` at their caps; evidence kinds skip state save and
  reload still succeeds.

JS (`node --test`):
- `panel-state.test.js`: turn per messageId; queueEdited replacement; evidence before/after
  tool event links identically; unlinked evidence and unlinked steps; legacy journal fixtures
  (taken from a `70bb31f7` build) render "not recorded" labels and never a verdict;
  replay from 0 and incremental replay produce deep-equal turns; unknown kinds advance cursor.
- `sidepanel.test.js`: acting line excludes pages only switched to; folded group names failed
  steps; approval Allow disabled while `stopRequested`; new reply shapes; recovery buttons gated
  by review state; artifact digest mismatch shows "changed".
- `agent-panel-bridge.test.js`: allowlist, timeouts, validators for the four new ops.

### 14.2 Live checks on the build host

Run on `leo@192.168.0.190` against a local fixture site (`scripts/panel-acceptance/site/`):
a form whose save is rejected server-side, a paginated list, same-name and slow/aborting
download endpoints, a password form, a page that sets a cookie and has `?access_token=` links.
Deterministic scenarios use fixture backends (`scripts/panel-acceptance/backends/`, one per
protocol, launched through the conversation `executable` override) that drive the real
`chrome-use mcp` server from the config the host writes, so evidence flows through the real
MCP, daemon and Chrome. Each installed real backend also runs scenarios 1, 4 and 12 once.
Each check asserts on `conversationRead` output and on the files in the conversation folder.

1. **Tool failure then recovery**: a click fails, a second selector succeeds; the failed step
   stays visible in the folded group; outcome completed; assessment per evidence.
2. **Empty answer**: backend completes with no text; `emptyAnswer`, host assessment
   `unverified/notAssessed`.
3. **Partial completion**: two requirements, one confirmed by `expect`, one only claimed →
   `partial`; a "verified" declaration is downgraded with a reason.
4. **A, B, C pages**: send from tab A; the agent opens B and acts there; the user activates C
   during the turn. Sent with A; acted on B; no event mentions C; the chip shows C for the next
   message only.
5. **Stop timeout with process alive**: a fixture backend ignores interrupt; with the e2e
   fault switch that skips the kill (`#[cfg(feature = "e2e-tests")]`), the service reports
   `processUnconfirmed`; `control` is `unknown`, never `human`; review says `alive`; fork is
   refused; terminate confirms; fork then succeeds and sends nothing. Without the switch, the
   10 s timeout kills the group and `control` becomes `human` only after `exited`.
6. **Approval/stop interleave**: a pending approval, then Stop: Allow is disabled and refused
   by the service, full access does not auto-approve a request arriving during stop, the turn
   ends interrupted, no action runs after Stop (no `browserActionTarget` after the stop
   sequence).
7. **Same-name downloads**: two different files saved as `report.pdf`; two artifacts, two
   digests, both readable and matching after the source path was overwritten.
8. **Interrupted download**: the aborting endpoint on the relay path (the one panel
   conversations use) → `interrupted`, no Ready card. User cancellation on the relay path
   (`downloadUrl: interrupted (USER_CANCELED)`) → `download_canceled` → `cancelled` is covered
   by a unit test of the mapping, not live: the harness cannot press Chrome's cancel without
   driving the user's download UI.
9. **Deleted artifact file**: delete a blob; the next read reports unavailable; exactly one
   `artifactUnavailable {detectedBy: "read"}` across repeated reads and two open panels.
10. **Cross-conversation artifact read**: reading conversation B's artifact through A is
    refused; so are non-UUID ids.
11. **Synthetic secrets never persisted**: fill `SYNTH_PW_…` into the password field, eval
    `document.cookie` containing `SYNTH_COOKIE_…`, open a link with `access_token=SYNTH_TOK_…`,
    route with `Authorization: Bearer SYNTH_BEARER_…`, have the backend echo the cookie in its
    answer, and approve one of these steps with approvals on. `grep -r SYNTH_` over the
    conversation folder (journal, state, index, artifact index) and over `conversationRead`
    output finds nothing; the approval still executed with the real value.
12. **Stalled progress**: four reads of an unchanged page raise one `progressStalled`; reading
    six pages through "next" raises none; a 200 s silent fixture raises `noActivity`; a 60 s
    `diag pages --watch` raises none.
13. **Refresh resume consistency**: during a running turn, close and reopen the panel and open
    a second panel; both projections deep-equal a fresh read from 0; no duplicate cards.
14. **Replay producing one turn**: drop the native reply after the host stored the message
    (bridge fault injection), retry the same id; one `user`, one `turnStarted`, one turn card.
15. **Old journal**: a conversation folder created by a `70bb31f7` build opens, shows its
    history with "not recorded" labels, accepts a new message, and its old lines are byte-identical
    afterwards.
16. **Stealth**: during scenarios 1 and 4 the CDP command log for the session contains no
    `Runtime.enable`, no `Console.*`/`Log.enable`, and no new `Network.enable` unless a step
    passed `observe: true`; the page DOM has no added attributes or nodes (MutationObserver
    fixture on the site); no recording without the message flag.
17. **Recording**: with "Record this task", capturing → finalizing → ready; without ffmpeg on a
    second run, `encoderMissing`; without the flag, `chrome_use_record start` is refused.

## 15. Risks

1. **Approval reply substitution (7.3).** Getting it wrong sends redacted input to the agent
   or lets an old panel's echo through. Mitigated by server-side substitution, strict equality
   for the legacy shape, and tests 14.1 and live 11.
2. **Image blocks and `short()`.** Stripping must happen before `short()` in all four adapters,
   or screenshot results keep being cut to an opaque preview and lose the marker. Covered by
   adapter fixtures.
3. **actionId survival per backend.** If a CLI drops tool result text, linking degrades to
   unlinked records. Step 1 measures it before the rest is built.
4. **Evidence authenticity.** The report comes from a process the agent's tool runtime started;
   an agent with shell access (same user) could post a forged report. Labels say "reported by
   chrome-use's browser tools", not "proven". Evidence from actions run outside the MCP (shell
   `chrome-use`, forbidden by the prompt) is absent, shown as no browser record.
5. **Engine lock and file copies.** Hashing a 100 MiB download under the engine mutex would
   stall every conversation; ingestion runs outside the lock with the artifact id reserved
   first.
6. **Journal write volume.** Two extra fsynced events per action; state saves skipped for
   evidence kinds. Measure on scenario 12 (≥50 actions) and keep per-action overhead under
   20 ms on the build host.
7. **Redaction misses free text.** Shape and exact-secret rules do not catch every secret in
   page text. Page text is capped and labelled; the privacy note says so. The known-secret set
   is in memory: after a service restart, a password filled before the restart is no longer
   scrubbed by exact value from later text (shape rules still apply). The turn that filled it
   is `RecoveryRequired` after a restart, so no later text of that turn is written.
8. **Process identity.** PID reuse and processes that leave the group (`setsid`) are not caught
   by the group probe on Unix; the start-time check handles reuse, `setsid` escapes remain a
   documented gap. Windows relies on `KILL_ON_JOB_CLOSE`.
9. **Stall false positives.** Advisory only, conservative thresholds, excluded tools; the live
   pagination check guards it.
10. **Mixed versions.** An old service or old extension during upgrade; capability flags and
    additive events keep both directions working.
11. **`observe` adds `Network.enable`** when the agent opts in per step. Accepted because it is
    the existing `--observe` path, scoped to steps the agent marks, and never used for progress
    display; checked by live check 16.

## 16. Deliberately excluded (not deferred)

- Automatic `--observe` on every panel action: changes latency and the page-observable surface
  for a UI feature (D3).
- A live page preview or periodic screenshots in the panel: would be continuous capture for
  progress display.
- Judging "meets the request" by a second model: the research shows a same-model judge is not
  ground truth; the card reports assessment and evidence instead.
- Rewriting or back-filling old journals: they are evidence as recorded.
