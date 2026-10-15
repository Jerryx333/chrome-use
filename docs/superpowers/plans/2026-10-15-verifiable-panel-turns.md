# Verifiable panel turns: implementation plan

Baseline: the branch starts from origin/main at v1.5.214 (`39a6e6ac`). Line numbers were
re-checked on that base. Every cited file is identical to `70bb31f7` except
`cli/src/agent_panel.rs` (`handle_for_profile` moved to 579) and `agent-panel-bridge.js`.

Source research: `reports/Browser Use 对话系统深度研究.md` and the three notes in
`research_notes/Browser Use 对话系统深度研究/`. This plan turns their recommendations
into one release. Six capabilities ship together; none is staged for a later version.

1. Process and conclusion organized per user message (`turnStarted` / `turnEnded`, `taskAssessment`).
2. Attached page vs acting page, and action effects (`browserActionTarget`, `actionObserved`).
3. Takeover and unknown-execution recovery (`control` state, `recoveryReview`, `recoveryTerminate`, `recoveryFork`).
4. First-class artifacts: screenshots, downloads, recordings (`artifactCreated` / `artifactReady` / `artifactUnavailable`).
5. Privacy minimization before anything reaches the journal.
6. Long-task progress and evidence continuity (`progressStalled`, `batchInterrupted`, `turnSummary`, `contextCompacted`).

## Review response (seq 24)

| review point | verdict | where |
|---|---|---|
| P1 a delayed `agentActionBegin` from turn A is admitted under turn B | Agreed. Begin had no turn identity of its own and took `chat.active` at processing time. Fixed by making the generation a turn epoch: the MCP sends the generation it was spawned with (`CHROME_USE_PANEL_GENERATION`, set per process in `drive`, `backend.rs:97`), every turn that ends other than cleanly retires its generation and the next message gets a new process, and Begin carries `notAfter`. The same rules bind `agentTaskAssess` and `agentRecordingAuthorize`. One code fact the design has to handle: `start()` hands the next message to any existing job (`service.rs:1585-1593`), so a retired job must go through the existing replace path (`service.rs:1556-1583`) instead. One case where a turn can complete while an MCP call still runs: the backend's own MCP client timeout. The plan sets it above the Begin window and falls back to retiring after every turn where that cannot be set. | D1, 3.2, 3.3, 4.1, 4.3, 6.4, 14.1, 14.2.20, risk 13 |
| P2-1 projection fallback weakens different-payload rejection | Agreed. The projection match applies only when the retried message is already its own canonical projection. | 7.2, 14.1 |
| P2-2 per-lease output silently replaces the caller's path | Agreed. The capture and the delivery to the requested path are two effects, recorded and checked separately. | 6.2, 2.2, 14.1 |
| P2-3 crash points and concurrent duplicate reports | Agreed. A capture verdict is now written last, after the artifact outcome; duplicate reports during ingest get `pending`. | 3.3, 6.1, 14.1 |
| P2-4 pidfd kill and macOS proof | Agreed. Kill loops until the group is empty; the macOS loop test is observation only; proof comes from kernel source or the probe answers `unknown`. | 5.3, 5.4, risk 8 |
| P2-5 naming | Agreed: `sentProjection`; the `sent` dispatched flag keeps its meaning. | 7.3 |

## Review response (seq 22)

Every point was checked against the code on the branch. Two points are partly wrong in their
premise; the plan still changes in both places, and says why.

| review point | verdict | where |
|---|---|---|
| P1-1 evidence attributed to the wrong turn | Agreed. `generation` is per backend process (`service.rs:1651-1655`) and a live process is reused for the next turn (`service.rs:1585-1593`), so two turns share a generation. Attribution now comes only from an action lease taken before the action runs. | D1, 2.2, 3.3, 4.1, 14.1, 14.2.6, 14.2.18 |
| P1-2 "confirmed evidence means met" | Agreed. Requirements are checked only through a declared, machine-checkable predicate; existence, type and page changes never satisfy a requirement. The badge never says "Verified". | D4, 4.3, 9.2, 14.1, 14.2.3, 14.2.19 |
| P1-3 exit proof and control scope | Agreed. One factual addition: on Windows the service does not hold the job today. `owned_job` is a local in `drive` (`backend.rs:222`) and is dropped on return, which fires `KILL_ON_JOB_CLOSE` (`windows.rs:70`) without confirmation. The plan keeps the handle. Unix leader is reaped before the group check (`backend.rs:281-310`), so "leader dead, group alive" is the normal unconfirmed case. | 5.1, 5.3, 5.4, 14.2.5, 14.2.6 |
| P1-4 fingerprint changes under redaction | Agreed. `fingerprint()` includes `page` when attachments or a skill are present (`conversation.rs:73-86`) and `accepted` hashes persist in `state.json`. The legacy rule now runs on the raw input; only the stored projection is redacted. | D5, 7.2, 10, 14.1, 14.2.15 |
| P1-5 artifact source integrity | Agreed. `download_url` with a path moves Chrome's file to a caller-chosen destination (`actions.rs:14703-14715`), so two parallel same-name calls race on one path. Outputs are now allocated per lease. | 6.1-6.3, 14.1, 14.2.7 |
| 7.3 approval binding | Agreed. Requests are keyed only by `(conversation, requestId)` (`service.rs:633-646`, `1391-1396`, `panel-state.js:40`); Codex JSON-RPC ids restart with each process, and the old panel builds a card once per key (`sidepanel.js:383`), so a stale card can answer a newer request with the same id. `can_use_tool` accepts any `updatedInput` today (`service.rs:1503-1506`). | 7.3, 14.1 |
| P2 D2 target provenance | Agreed. | D2, 2.2, 9.2 |
| P2 D3 `observe` and `Network.enable` | Partly disagree. `Network.enable` is already sent on every attach, relay path included (`enable_domains`, `browser.rs:4895-4897`; `adopt_relay_popup` calls it at `browser.rs:7447`). `enable_request_tracking` (`actions.rs:20504-20522`) adds no new CDP domain; it flips `DaemonState.request_tracking`, after which every request's headers and post data are kept in an unbounded `Vec` (`actions.rs:1537-1560`) for the daemon's life. That is a memory and privacy lifetime problem, not a page-observable one. The plan scopes it per panel action. Live check 16's old wording ("no new `Network.enable` unless observe") was wrong and is replaced. | D3, 14.1, 14.2.16, risk 11 |
| P2 streaming scrub | Agreed. | 7.2, 14.1 |
| P2 recording container and async finalize | Agreed. ffmpeg picks the codec from the extension (`recording.rs:107-111`) and `recording_start` refuses while one is active (`recording.rs:38-40`). | 6.4, 14.2.17 |
| P2 docs per AGENTS.md | Agreed (`AGENTS.md:20-29`). | 12, 13 step 11 |
| P2 old panel `[object Object]` | Agreed. The old panel calls `String(...)` on journal fields (`panel-state.js:59`, `82`). Masks keep the field's JSON type. | 7.2, 10, 14.1 |
| P2 compat with real old fixtures | Agreed. | 10, 14.1, 14.2.15 |

## 0. Invariants this release must keep

Each is covered by an existing test or by a new acceptance check in section 14.

- The journal (`Store::event`, `conversation.rs:518-598`) stays the single source of truth: append-only, contiguous `sequence`, `after` cursor paging, gap and corruption refusal. No second event stream, no rewriting of old lines.
- Message UUID plus fingerprint dedupe (`Conversation::accept`, `conversation.rs:227-262`). The fingerprint rule (`conversation.rs:73-86`) is applied to the raw input exactly as today; a message accepted by v1.5.214 and retried after the upgrade with the same id is a duplicate, never a resend and never a conflict.
- Queue, direct `Stop & send` (`accept_direct`, `terminal` at `conversation.rs:288-391`), queue pause after non-completed turns.
- Approvals: `dispatched` is not `confirmed`; `requestResolved` or a matching tool result releases a request (`service.rs:593-613`, `1386-1422`).
- Unknown-delivery protection in the side panel (`pendingMessages`, `sidepanel.js:427-473`) and `RecoveryRequired` refusing new messages and `begin_next`.
- Native document/ref invalidation and no blind replay of non-idempotent actions (`actions.rs:11809-11886`, `15865-15960`).
- Stealth: no page DOM markers, no `Runtime.enable`, no console capture, no continuous recording for progress UI, no CDP domain beyond what `enable_domains` already sends. The backend prompt line forbidding them (`backend.rs:109`) stays verbatim.
- Old journals open. Anything a journal did not record is shown as "not recorded", never inferred.
- Nothing the host cannot check is shown as checked.

## 1. Decisions that shape everything else

**D1. Browser evidence travels on a host side channel, bound by an action lease.**
The chrome_use MCP server already runs inside the conversation's environment and calls the
service directly for panel tools (`panel_mcp_context` / `call_panel_mcp`, `agent_panel.rs:32-72`).
Before every browser tool call in panel context the MCP asks the service for a lease
(`agentActionBegin`). The service admits it only while the conversation has an active message
in `Starting`, `Running` or `WaitingInput` and the caller's generation is the live job's. The
lease binds `{conversation, messageId, generation, actionId}`, is journaled as `actionAdmitted`,
and carries the action's allocated output paths (section 6). The evidence report
(`agentActionReport`) can only complete a lease; the service never looks at `chat.active` to
decide where a report belongs. A report with no lease, a closed lease of another
conversation, or a stale generation goes to `unboundActionReport`, which no turn, assessment or
summary reads.

A refused lease means the tool does not run: the MCP returns a tool error ("the user stopped
this request" / "the conversation needs recovery"). This is what makes Stop a hard boundary for
browser actions. If the service cannot be reached within 5 s the MCP also refuses to run the
action. Running it unleased would produce an action that Stop cannot gate and no turn can
claim; the backend is a child of the service, so an unreachable service already means the turn
is ending as unknown.

Reasons for the side channel at all: `short()` (`backend.rs:52-59`) replaces any item over
24 KB with a 6000-character preview, so a screenshot result with its base64 block always loses
its structure; and no backend is verified to preserve `structuredContent`. The backend's tool
event only has to carry the `actionId` back for linking a step to its evidence. Linking is by
`actionId` only, never by time or adjacency.

**D2. Acting-page identity comes from native state, with its provenance recorded.**
Today `observed.target` exists only on `--observe` replies (`actions.rs:3068-3086`), and no
MCP tool sets `observe`. The daemon will stamp an `actionContext` block (target before and
after) on every command that carries `_panelActionId`. Each target records where it came from:
- `actionReply`: the command's own reply named the page (navigation result URL after redirects,
  the popup tab adopted by `adopt_relay_popup`, `browser.rs:7283`, `tabs select` with
  `verified: "confirmed"`).
- `daemonState`: `BrowserManager`'s cached active target (`observed_target()`, `browser.rs:7096`)
  plus the Chrome tab id from the relay session id (`relay_chrome_tab_id`, `browser.rs:2834`).
  This is where chrome-use was pointed, not proof of the effect.
No CDP call, no new domain. The panel says "Acted on" only for `actionReply` targets; a
`daemonState` target is shown as "Browser tools pointed at"; a requested/after mismatch is
described only when both sides are `actionReply`. Redirects show "asked for X, landed on Y".

**D3. Observation is opt-in per action, and its request tracking is scoped to that action.**
`--observe` costs a baseline and a post snapshot and a settle wait. It sends no new CDP domain:
`Network.enable` is already on from attach (`browser.rs:4895-4897`). What it changes is
`DaemonState.request_tracking` (`enable_request_tracking`, `actions.rs:20504-20522`), which
afterwards keeps every request's URL, headers and post data in daemon memory for the daemon's
life (`actions.rs:1537-1560`, no cap). The acting MCP tools (click, fill, type, press, select,
hover, open, back, forward, reload) gain an optional `observe` boolean mapped to `--observe`.
For commands carrying `_panelActionId`, the daemon records whether tracking was already on;
if it was off, it turns it on for that command only and on completion turns it off and
truncates `tracked_requests` back to the mark taken before the command. No `Network.disable`
is sent. When tracking was already on (a user's `requests --clear`, the stream server,
`actions.rs:837`, `17150`) nothing changes; HAR uses its own `har_recording` flag
(`actions.rs:1478`) and is untouched. Plain CLI `--observe` keeps today's behaviour.

**D4. Task assessment is a tool call whose requirements carry checkable predicates.**
A new panel tool `chrome_use_task` (action `assess`) records the agent's verdict, bound to the
caller's lease context (active message and generation). Each requirement the agent marks met
must name a check from a closed set (section 4.3). The host verifies that the cited evidence
recorded exactly that check, on a target in this turn, and that it is still fresh. The host
can prove that a predicate held on a page; it cannot prove that the predicate expresses what
the user meant, so the card prints each predicate beside its requirement and the badge says
"Agent's browser checks passed", never "Verified" or "Done". A requirement without a check
stays "Agent claimed". The assessment is provisional until `turnEnded`; the host then journals
the final one.

**D5. Redaction happens once, in the service, before `Store::event`, and never touches identity.**
A `Redacted` newtype is the only thing `Store::event` accepts, so no write path can bypass it.
Request identity (message fingerprint, `actionId`, `requestId`, cursors) is computed from raw
input before redaction and is never altered by it. The in-memory approval original stays
unredacted for validation and for building the backend reply. Masks keep each field's JSON
type, so an old panel renders them as text.

**D6. No `Conversation` struct changes.** `Conversation` is `deny_unknown_fields`
(`conversation.rs:130`); a new field would make state written by this build unreadable by an
older one. Everything new is either a new journal event kind, derived at load from the
journal, or a field on `Message` (which is not `deny_unknown_fields`). `schema_version` stays 1.
The `accepted` map keeps its value format (legacy fingerprint hex), so a downgrade still dedupes.

## 2. Journal contract

### 2.1 Envelope

`Event { sequence, at, kind, data }` is unchanged. Every new kind carries these fields in
`data`:

| field | meaning |
|---|---|
| `v` | event-data version, `1` for every kind added here |
| `messageId` | the turn it belongs to, taken from the lease or from the message snapshot at terminal; absent on `unboundActionReport` |
| `generation` | backend process generation the evidence came from |
| `privacyClass` | `public`, `pageContent`, `userInput` or `masked` (section 7) |
| `actionId` / `artifactId` | when the event is about one action or artifact |

`conversationId` is not repeated per event: the journal file is per conversation and
`conversationRead` returns it in its envelope. `sequence` is the journal's own.

Existing kinds keep their shapes and gain only optional fields: `tool` gains `toolCallId`,
`actionIds`, `privacyClass`, `redactions`; `textFinal` gains `role`
(`final` / `commentary`, absent when unknown); `ready` gains `pgid`, `processStartTime`,
`serviceSid`; `request` gains `generation` and `requestRevision`; `running` already carries
`turnId`.

### 2.2 New kinds

```
turnStarted        {v, messageId, generation, direct, continuation: null|"afterStop"|"afterRecovery"}
turnEnded          {v, messageId, generation, outcome: "completed"|"failed"|"interrupted"|"unknown",
                    error, emptyAnswer, finalItemIds: [itemId], sourceSequences: [first, last],
                    openLeases: [actionId]}
actionAdmitted     {v, messageId, generation, actionId, tool, toolClass: "act"|"observe"|"capture",
                    outputs: [{artifactId, kind, name}], quotaReserved}
browserActionTarget{v, messageId, generation, actionId, tool, toolClass,
                    requested: null|{tabHandle, url},
                    observed: null|{before: Target|null, after: Target|null},
                    outcome: "ok"|"error"|"timeout", errorCode, durationMs, resultDigest,
                    afterStop: bool, afterTurnEnd: bool, afterRecoveryRequired: bool}
                    Target = {chromeTabId|null, tabRef, targetId, url, title|null,
                              provenance: "actionReply"|"daemonState"}
actionObserved     {v, messageId, actionId, verdict: "confirmed"|"failed"|"uncertain",
                    evaluator: "browser"|"host"|"none", basis: "expect"|"readBack"|"navigation"|"tabVerified"|
                    "download"|"capture"|"toolError"|"pageChanged"|"none", detail, possiblyExecuted: bool,
                    check: null|{kind, predicate, target: Target, observedSequence}}
unboundActionReport{v, generation|null, actionId|null, tool, reason: "noLease"|"staleGeneration"|
                    "otherConversation"|"malformed"}                (quarantine, never cited)
taskAssessment     {v, messageId, phase: "provisional"|"final", source: "agent"|"host",
                    status: "checksPassed"|"someChecksPassed"|"notChecked"|"blocked",
                    declaredStatus, reason,
                    requirements: [{id, text, declared: "met"|"unmet"|"unknown",
                    check: null|{kind, predicate, evidence: id},
                    computed: "checked"|"checkFailed"|"stale"|"claimed"|"unmet"|"unknown",
                    because}], downgradedBecause: [string], blocker: null|{kind, text}}
artifactCreated    {v, messageId, actionId, artifactId, kind: "screenshot"|"download"|"recording",
                    name, state: "pending"|"capturing"}
artifactState      {v, artifactId, state: "finalizing"}            (recordings only)
artifactReady      {v, artifactId, mime, declaredExtension, typeMatches: true|false|null, bytes,
                    digest: "sha256:<hex>"}
artifactUnavailable{v, artifactId, reason: "cancelled"|"interrupted"|"timeout"|"failed"|"tooLargeToRetain"|
                    "quotaExceeded"|"pathMismatch"|"notRegularFile"|"notOwned"|"linked"|"missing"|
                    "changed"|"encoderMissing"|"encoderFailed",
                    detail, bytes|null, digest|null, detectedBy: "report"|"read"|"turnEnd"|"load"}
controlChanged     {v, messageId, from, to, reason}
progressStalled    {v, messageId, kind: "repeatedIdenticalResult"|"repeatedFailure"|"noActivity"|"nativeNoProgress",
                    count|null, quietMs|null, actionIds, sourceSequences}
progressResumed    {v, messageId, sourceSequence}
batchInterrupted   {v, messageId, outcome, completed: [toolCallId],
                    unresolved: [{toolCallId, name, actionId|null, executed: "reported"|"admitted"|"unknown"}]}
turnSummary        {v, messageId, sourceSequences: [first, last], steps, failedSteps,
                    actions: {confirmed, failed, uncertain, unlinked, admittedNotReported},
                    artifacts: {ready, unavailable}, assessment: status, stalls}
contextCompacted   {v, messageId, backend, trigger|null, sourceSequences: [first, last]}
recoveryTerminated {v, messageId, method: "jobObject"|"processGroup", confirmed, detail}
recoveryForked     {v, messageId, forkedTo}                         (original conversation)
recoveryContext    {v, fromConversationId, fromMessageId, fromSequence, outcome: "unknown",
                    possiblyExecuted: [{actionId, tool, target, verdict}], lastTargets: [Target],
                    artifacts: [{artifactId, name, kind}]}          (first event of the fork)
```

Bounds: every new event is built to stay under `MAX_EVENT` (64 KiB, `conversation.rs:18`):
strings capped (URL 2048, title 256, text 500, predicate 500, detail 1000), lists capped
(requirements 12, evidence per requirement 16, unresolved tools 64, possiblyExecuted 20,
openLeases 64). Over-cap lists record `truncated: n`.

Evidence-only kinds (`actionAdmitted`, `browserActionTarget`, `actionObserved`,
`unboundActionReport`, `artifact*`, `progress*`) are journaled without the `state.json` rewrite
that `Engine::record` does for other kinds (`service.rs:552-559`). That is safe because
`Store::load` already recomputes `next_sequence` from the journal and only refuses state that
is ahead of it (`conversation.rs:489-496`). Each event is one line; a torn last line already
makes the journal refuse to load (`conversation.rs:717-721`), so "partially written" means some
of a report's events are present and others are not, which 3.3 handles.

## 3. Evidence path: native → MCP → agent_panel

### 3.1 Native (stamp the acting target)

- `cli/src/connection.rs` `send_command` (`2264-2310`): when env `CHROME_USE_PANEL_ACTION_ID`
  is a UUID, stamp `_panelActionId` on the command, next to `_cbSkip` / `_clickMode`.
- `cli/src/native/browser.rs`: add `pub fn acting_target(&self) -> Option<ActingTarget>`
  returning `observed_target()` fields plus `chrome_tab_id: relay_chrome_tab_id(&page.session_id)`
  and the cached page title if `PageInfo` has one, with `provenance: "daemonState"`. Pure field
  reads.
- `cli/src/native/daemon.rs` (`671-701`): if the command carries `_panelActionId`, take
  `acting_target()` before `execute_command_recovering`, again after, and insert
  `actionContext: {actionId, before, after}` into the reply. When the command's own reply names
  the resulting page (navigate result URL, popup adoption, verified tab select), `after` is
  built from that reply with `provenance: "actionReply"`. Top-level commands only; script and
  batch steps are not stamped.
- Scoped request tracking for `_panelActionId` + observe commands (D3): save
  `request_tracking` and `tracked_requests.len()` before, restore both after, on success and
  on error.
- `cli/src/native/actions.rs` `handle_download` (`14103-14263`) and the extension
  download-url path: give the three failures machine codes in the reply (`code`:
  `download_canceled`, `download_timeout`, `download_interrupted`) beside the existing messages.
  Panel conversations are bound to the relay, so their downloads go through
  `extension_download_url` and `download-manager.js`, which throws
  `downloadUrl: interrupted (<reason>)`; `USER_CANCELED` maps to `download_canceled`, other
  reasons to `download_interrupted`, the extension's timeout to `download_timeout`. The CDP
  `canceled` state in the launched-browser loop maps to `download_canceled` as well.
- `record stop` gains an optional `expectPath`: the daemon refuses to stop when the active
  recording's `output_path` differs (section 6.4).
- `relay_chrome_tab_id` (`browser.rs:2834`) becomes `pub(crate)`.

### 3.2 MCP (lease, run, report, return)

New module `cli/src/panel_evidence.rs`, used by `cli/src/mcp.rs` only when
`crate::agent_panel::panel_mcp_context()` is `Some`.

- `call_tool` (`mcp.rs:732-790`): for every chrome_use browser tool, mint `actionId`
  (UUIDv4) and call `agentActionBegin` (3.3). On refusal or no answer, return a tool error and
  do not run the CLI. On admission, replace any output `path` argument of `download`,
  `download_url`, `screenshot` with the lease's allocated path (section 6.2), then pass the
  `actionId` to `run_cli` as env `CHROME_USE_PANEL_ACTION_ID` (add an `env` parameter to
  `run_tool` / `run_cli`, `mcp.rs:1781-1795`, `1982-2038`).
- After the CLI returns, `panel_evidence::derive(tool, arguments, &CliRun, parsed)` builds the
  report. Verdict rules (all from the native reply, nothing guessed), tool-specific rules first
  so an `expect` failure reads basis `expect`, not `toolError`:
  - Generic, after the tool-specific rules: `isError` → `failed`, basis `toolError`,
    `errorCode` from the reply's `code`.
  - CLI killed on timeout → outcome `timeout`, `uncertain`, `possiblyExecuted = !safe_to_repeat(action)`.
  - `chrome_use_expect` success → `confirmed`, basis `expect`, `check = {kind: "expect",
    predicate: "<expect kind> <argument>", target: after}`; failure → `failed` with the same
    `check`. The predicate is the expect's own arguments (redacted per 7.2), so the host can
    later compare it with what an assessment claims.
  - `fill` / `type`: compare `readBack` with the requested value inside the MCP process;
    equal → `confirmed` / `readBack`, `check = {kind: "readBack", predicate: "<selector> has the
    typed value"}`; different → `failed`; masked or absent → `uncertain`. Only the boolean
    leaves the MCP process, never the read-back text.
  - `open` / `back` / `forward` / `reload`: `check = {kind: "urlMatch", predicate: <landing URL,
    redacted>}` from an `actionReply` target; `confirmed` / `navigation` when it equals the
    requested URL after normalization; a different origin → `uncertain` with the landing origin
    in `detail`.
  - `tabs select` with `verified: "confirmed"` → `confirmed` / `tabVerified`.
  - `download`, `download_url`, `screenshot`: `confirmed` / `download` or `capture` only after
    the service reports the artifact `Ready` (it answers synchronously, 3.3). This confirms that
    a file was captured, nothing about its content.
  - With `observe: true`: `observed.changed == true` → `uncertain` / `pageChanged` (a change is
    not the requested effect); `noProgress` present → feeds `progressStalled`
    (`nativeNoProgress`); `humanCheck` → `uncertain` and turn blocker hint.
  - Everything else that acts (click, press, select, hover, scroll, eval, upload) → `uncertain`,
    basis `none`, `possiblyExecuted = true`.
  - read, snapshot, extract, find, a11y, tabs list, downloads list → `toolClass: "observe"`,
    no verdict, `resultDigest` = SHA-256 of the result text with the marker and timing removed.
- Report with `crate::agent_panel::report_panel_evidence(report)`: a new function next to
  `call_panel_mcp` that uses a new `service::call_existing` (endpoint + health only, no
  `ensure()`). Budget 5 s for non-artifact reports, 60 s with artifacts. A lost reply is
  retried once with the same `actionId`; the service replays its first answer (3.3). A failed
  report never fails the tool; the MCP appends "browser evidence was not recorded" so the
  transcript does not imply evidence exists.
- Tool result: prepend a text block `chrome-use action <actionId>` as the first content block
  and add `structuredContent.chromeUseAction = {actionId}`. First position survives any later
  truncation in the adapters; the adapters extract it before `short()` anyway (3.4).
- New tool arguments: optional `observe` on the acting tools listed in D3
  (`core_tools` / `extended_tools`, `mcp.rs:315-676`), mapped to `--observe`. Non-panel MCP
  clients see the same optional argument; behaviour without it is unchanged.
- New panel tools in `panel_tools` (`mcp.rs:282-289`) and `is_known_tool` (`686-691`):
  `chrome_use_task` (`assess`, section 4.3) and `chrome_use_record` (`start` / `stop`,
  section 6.4). These take no browser lease of their own.

### 3.3 Service ops for the side channel

Added to `agent_panel.rs` `handle_for_profile` (`579-635`) as agent-only operations reached
through `call_panel_mcp`/`report_panel_evidence`, to `service::call`'s existing-service list
(`service.rs:456-470`) so a tool call can never bootstrap a second service, and to
`operation_seconds` (`service.rs:151-158`).

`Engine` gains `leases: HashMap<conversation, HashMap<actionId, Lease>>`,
`Lease {messageId, generation, admittedSequence, afterStop: bool, state: Open|Reported(reply),
outputs, quotaReserved}`. Leases are rebuilt at load from `actionAdmitted` and the report events
in the same pass that computes `last_sequence` (`conversation.rs:692-728`). At most 256 open
leases per conversation; beyond that new leases are refused.

- `agentActionBegin {callerConversation, generation, actionId, tool, arguments}` (5 s). Under
  the engine lock: the conversation exists; the job is live with this `generation`; `chat.active`
  is `Some`; the phase is `Starting`, `Running` or `WaitingInput`; `actionId` is a UUID not seen
  in this conversation. Otherwise refused with a reason the MCP shows the agent. On admission:
  allocate outputs and reserve quota (6.1, 6.2), journal `actionAdmitted` with
  `messageId = chat.active.id`, reply `{messageId, outputs: [{artifactId, path}]}`.
- `agentActionReport {callerConversation, generation, actionId, tool, arguments, outcome,
  errorCode, actionContext, verdict, check, resultDigest, durationMs, artifacts: [{artifactId,
  path}]}` (60 s).
  - Lock 1: look up the lease in this conversation. None → journal `unboundActionReport`
    (`noLease`, or `otherConversation` when the id is leased elsewhere, or `staleGeneration`),
    reply `{bound: false}`. Lease `Reported` → reply the stored answer unchanged (idempotent
    retry; an artifact's recorded terminal state is returned, never recomputed or dropped).
    Lease `Open` → mark it `Reporting`, journal `browserActionTarget` and `actionObserved` with
    the lease's `messageId` and `generation`. Flags: `afterStop` when the lease was admitted
    before a stop that has since been requested, `afterTurnEnd` when the lease's turn has a
    `turnEnded`, `afterRecoveryRequired` when the conversation is `RecoveryRequired`. A late
    action from an unconfirmed process is evidence recovery needs; it is kept, under its own
    turn.
  - Off lock: ingest each reported artifact whose path is the lease's allocated output (6.2).
  - Lock 2: journal each artifact's terminal event under the lease's `messageId`, set the lease
    to `Reported(reply)`, reply `{bound: true, artifacts: [{artifactId, state}]}`.
  - Partial completion: a lease left `Reporting` (service died between lock 1 and lock 2) is
    rebuilt at load as reported-without-artifacts; every allocated artifact without a terminal
    event gets `artifactUnavailable {reason: "interrupted", detectedBy: "load"}`. A retry after
    that gets the recorded states. Terminal artifact states never change afterwards.
- `agentTaskAssess` (10 s), `agentRecordingAuthorize` (20 s): sections 4.3 and 6.4. Both bind to
  the caller's active message only when the caller's generation is the live one and the phase
  admits leases.

### 3.4 Backend adapters (link tool events to actions)

In each adapter, before `short()`: extract `toolCallId` and any `chrome-use action <uuid>`
markers / `chromeUseAction.actionId` from the raw item, strip `image` blocks' `data`
(keep `{type:"image", mimeType, bytes}`), then emit
`notice("tool", {completed, toolCallId, actionIds, item: short(item)})`. Extraction happens
before redaction, so a known secret that happens to be a fragment of a UUID cannot break a link.

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
- `ready`: add `pgid`, `processStartTime` and `serviceSid` read right after spawn in `drive`
  (`backend.rs:224-228`; macOS `proc_pidinfo(PROC_PIDTBSDINFO)`, Linux `/proc/<pid>/stat`
  field 22, Windows `GetProcessTimes`; `serviceSid` is `getsid(0)`, the session the service
  created with `setsid`, `service.rs:315`).
- `Setup` gains `generation`; `drive` adds `CHROME_USE_PANEL_GENERATION` to `browser_env`
  (`backend.rs:97`) so the MCP can send it with leases and reports.

### 3.5 Prompt (`backend.rs:109`)

Append, keeping every existing sentence:
"Each chrome_use browser result starts with `chrome-use action <id>`. Pass `observe: true`
when an action should submit, save, pay or send, then check its effect with chrome_use_expect.
Before your final answer call `chrome_use_task` with action `assess`: list each thing the user
asked for and mark it met, unmet or unknown. For each one marked met, name the check that shows
it (an expect, a read-back, a URL match or a download type) and the action or artifact id that
ran it. Do not mark anything met that no check showed. If you cannot continue, assess as
blocked and say why."

## 4. Turns, final text and assessment

### 4.1 Turn boundaries

- `Engine::begin(id) -> Option<Message>` wraps `chat.begin_next()` and journals `turnStarted`
  in the same lock. All four call sites switch to it: `notice` (`service.rs:714`),
  `conversationSend*` (`1200`), `conversationResumeQueue` (`1362`), `run_schedule` (`2187`).
  `run_schedule` records its `user` event after `begin_next` (`2202`); it moves the `user`
  record ahead of `Engine::begin` so every journal has `user` before `turnStarted`. The
  projection still creates a turn on whichever of the two it sees first, for robustness.
- `turnEnded` is journaled in `Engine::notice` right after the `terminal`, `exited` or
  `processUnconfirmed` record. Its `messageId` is the snapshot `notice` already takes from
  `chat.active` before the match (`service.rs:590-592`), not a read after `chat.terminal()`
  has cleared `active` (`conversation.rs:388`). Outcome from the terminal status,
  `failed`/`interrupted` from `exited` (as `chat.terminal` already decides,
  `service.rs:675-696`), `unknown` for `processUnconfirmed`. `emptyAnswer` is true when no
  `textFinal` with this `messageId` carried non-whitespace text. `finalItemIds` lists text items
  with `role: "final"`; empty when the backend does not say. `openLeases` lists leases of this
  turn still `Open`.
- On `turnEnded`, in order: `batchInterrupted` if any open tool (4.4), the final
  `taskAssessment` (4.3), `turnSummary`, and the recording stop (6.4).
- Leases of an ended turn stay open for late reports, which keep their own turn's `messageId`
  and carry `afterTurnEnd: true`. They are shown under that turn as "reported after the
  request ended". They never change the next turn and never change the final assessment.

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

Arguments: `status`, `requirements: [{text, status, check?: {kind, predicate, evidence: id}}]`
(≤12), optional `blocker {kind: "humanCheck"|"login"|"permission"|"missingInput"|"siteError"|"other", text}`.
The service binds it to the caller's active message (refused with no active message, a stale
generation or a phase that admits no leases) and journals `taskAssessment {phase:
"provisional"}`.

Check kinds and what the host verifies. The cited evidence must be leased under this same
`messageId`; evidence from earlier turns is shown as context and never computes `checked`.

| kind | the agent supplies | the host requires | proves only |
|---|---|---|---|
| `expect` | the expect's predicate text and the action id | that action's `actionObserved.check.kind == "expect"`, `verdict == "confirmed"`, and its recorded predicate equals the supplied one | the page showed that predicate at that time |
| `readBack` | selector and action id | `readBack` check confirmed on that action | the field held the typed value |
| `urlMatch` | a URL or host+path prefix and action id | an `actionReply` target on that action whose redacted URL matches | the browser landed there |
| `downloadType` | artifact id, expected MIME | artifact `Ready`, `bytes > 0`, `typeMatches == true`, sniffed MIME equals the expected one | a non-empty file of that type was saved |

Screenshots and recordings can be cited for display but are never a check. Observing,
reading, or a page that changed are not checks.

Per requirement:
- `checked`: declared met, a check is supplied, the host verified it as above, and it is fresh:
  no later acting action in this turn ran on the same `targetId` after the check's
  `observedSequence` (otherwise `stale`).
- `checkFailed`: the cited check exists but its verdict is `failed`, or its recorded predicate
  differs from the supplied one.
- `claimed`: declared met with no check, or a check whose evidence is unknown, from another turn,
  `unbound`, or of the wrong kind.
- `unmet` / `unknown`: as declared.

Status: `blocked` if the agent declares a blocker; `checksPassed` only if every requirement is
`checked` and the turn outcome is `completed`; `someChecksPassed` if at least one is
`checked`; otherwise `notChecked`. Every lowering from the agent's declared status is listed in
`downgradedBecause`.

Final assessment: at `turnEnded` the host recomputes the latest provisional assessment against
the journal as it stands (freshness against actions reported after the assessment, the outcome
cap) and journals `taskAssessment {phase: "final"}`. With no provisional assessment the final
one is `{source: "host", status: "notChecked", reason: "notAssessed"}`. The panel shows a
provisional assessment as "Assessment pending" and only the final one as the badge. Compaction
does not change any of this: checks come from journaled evidence, not from the model's memory.

### 4.4 Open tools and `batchInterrupted`

`Engine` keeps `open_tools: HashMap<conversation, BTreeMap<toolCallId, OpenTool>>` from the
`tool` notices (started without completion). At `turnEnded`, any left open produce
`batchInterrupted`; `executed` is `reported` when a `browserActionTarget` for its `actionId`
exists, `admitted` when only an `actionAdmitted` exists (the action may have run), else
`unknown`. This covers Claude's several `tool_use` blocks in one message and Codex's parallel
items when a stop lands mid-batch. The native CLI `batch` command is not reachable from the
panel MCP surface, so it has no producer here.

## 5. Control, takeover and recovery

### 5.1 Control state

Derived in `Conversation::summary` (`conversation.rs:213-215`) as `control`, no struct field:

| phase / condition | control |
|---|---|
| `Starting` with active `continuation` | `resuming` |
| `Starting`, `Running`, `WaitingInput` | `agent` |
| `Stopping` | `stopRequested` |
| `Idle`, `Interrupted`, `Failed` | `stopped` |
| `RecoveryRequired` | `unknown` |

`control` describes this conversation's agent process only. It says nothing about other
conversations, a CLI session or another automation driving the same Chrome, so the panel never
says "you have control"; it says "This conversation's agent has stopped". `controlChanged` is
journaled when it changes. `stopped` is only reached through a confirmed terminal
(`terminal`/`exited` with exit confirmed); a stop that times out goes to `unknown` if
`processUnconfirmed`.

Stop boundary: from the lock acquisition that sets `Stopping`, `agentActionBegin` refuses.
Leases admitted earlier may still report; those reports are kept under their turn with
`afterStop: true` and `possiblyExecuted` as derived. Nothing is dropped or described as "did
not happen" because it arrived after Stop.

### 5.2 Approvals during stop

- `approve_pending` (`service.rs:503-527`) skips conversations in `Stopping`; for full-access
  conversations a request arriving during stop is answered with the method's decline/cancel.
- `conversationRespond` (`1386-1422`) in `Stopping` accepts only decline/cancel results. The
  phase check and the dispatch happen under the same engine lock that sets `Stopping`, so the
  order of Stop and Allow is the order the lock was taken.
- Both close the window where an approval could start an action after the user pressed Stop.

### 5.3 Process record and exit proof

The last `ready` without a later confirmed `exited` gives `{generation, pid, pgid,
processStartTime, serviceSid}`; it is rebuilt from the journal at load (no sidecar, no struct
field). The probe (`agent_panel/recovery.rs`, new) answers `exited`, `alive` or `unknown`, and
`exited` requires proof that the whole tree is gone, not just the leader.

Windows:
- Today `drive` owns the job as a local (`backend.rs:222`); when `confirm_empty` fails
  (`backend.rs:320`, `windows.rs:138-165`) `drive` returns and the dropped handle fires
  `KILL_ON_JOB_CLOSE` with nobody checking the result. Change: on a failed `confirm_empty`,
  `drive` hands the `ProcessJob` back with the error; `run` passes it with the
  `processUnconfirmed` notice (a non-serialized `Notice` field, `cfg(windows)`), and `Engine`
  keeps it in `held_jobs: HashMap<conversation, (generation, ProcessJob)>`.
- Probe while the service holds the job: `QueryInformationJobObject` active process count, as
  `confirm_empty` does. Zero → `exited`; otherwise `alive`.
- After a service restart the handle is gone; job objects here are unnamed and cannot be
  reopened. The probe answers `unknown`. Leader PID and creation time are shown for
  information but never turn `unknown` into `exited`.

Unix:
- The backend leader is reaped by `child.wait()` before the group check (`backend.rs:281-310`),
  so the usual unconfirmed case is a dead leader with a live group. A dead leader is never
  treated as an exited tree.
- `kill(-pgid, 0)` returning `ESRCH` → `exited`.
- Otherwise ownership must be proven before the group is called ours: enumerate the group's
  members (Linux `/proc/*/stat` fields 5 and 6; macOS `proc_listpids(PROC_PGRP_ONLY)` plus
  `proc_pidinfo` for the session) and require every member's session id to equal the recorded
  `serviceSid`. A process cannot join a process group in another session (`setpgid` returns
  `EPERM`), so members with that pgid inside that session are descendants of our backend. This
  depends on the kernel not reusing a PID while it is a live pgid or sid; Linux guarantees it,
  and step 1 runs a test on macOS that holds a group and cycles PIDs. Where the test fails, the
  probe answers `unknown` for any live group. Proven → `alive`; members in another session, an
  empty enumeration racing a live `kill(-pgid, 0)`, or any read failure → `unknown`.
- The same rule applies after a service restart: the recorded `serviceSid` is the old
  service's PID, and members still carrying it prove the old session.

### 5.4 Operations

- `conversationRecoveryReview {conversationId}` (15 s, read-only, journals nothing): phase,
  last error, the active message, process `exited|alive|unknown` with the reason, the turn's
  admitted and reported actions with verdicts and `possiblyExecuted`, unresolved tools,
  artifacts, last targets with their provenance. It does not touch the browser.
- `conversationRecoveryTerminate {conversationId}` (15 s): only in `RecoveryRequired` with probe
  `alive`. Windows: `TerminateJobObject` on the held job, then `confirm_empty`. Unix: on Linux,
  `pidfd_open` each proven member, re-check its pgid and sid through `/proc`, then
  `pidfd_send_signal(SIGKILL)`; on macOS, `kill(-pgid, SIGKILL)` immediately after a fresh
  ownership proof. Wait up to 5 s for `ESRCH` / zero active processes, re-probe, journal
  `recoveryTerminated {method, confirmed}`. Never signals a group whose probe is `unknown`.
- `conversationRecoveryFork {conversationId}` (10 s): only when the probe says `exited`.
  Creates a new conversation (same agent, requested model, options, executable, full access,
  `site` so it stays in the per-site list; no backend session, so nothing is resumed
  mid-turn), journals `recoveryContext` as its first event and `recoveryForked` in the
  original. The original stays `RecoveryRequired` and read-only. The fork does not send
  anything; its first user message gets `continuation: afterRecovery`, whose preface lists up
  to 20 possibly executed actions (tool, redacted target, verdict) and the last tabs as
  `chrome-tab:<id>` handles:
  "[This conversation continues one whose last request ended with an unknown outcome. These
  actions may already have taken effect: … Observe the pages before acting. Do not redo them
  unless this message asks you to.]"
- When the probe stays `unknown` (Windows after restart, unproven Unix group), fork and
  terminate are not offered. The review says the old agent may still be running and that a new
  conversation started normally does not stop it. Starting an ordinary new conversation is
  unaffected.

The delivery-unknown path (`pendingMessages`, `conversationMessageStatus`) is unchanged; the
panel shows it on the turn card when the message has a `user` row, otherwise in the existing
`pendingRecovery` section, and its text keeps saying that clearing the warning does not cancel
execution.

## 6. Artifacts

### 6.1 Store

New `cli/src/agent_panel/artifacts.rs`, modelled on `attachments.rs`:
- Allocated outputs: `<workspace>/.chrome-use-out/<actionId>/` (the agent's cwd, so it can read
  what it downloaded), created by the service with exclusive create at lease time inside the
  private tree (`private_tree`, `config_transaction.rs:772`). Its device and inode are stored in
  the lease.
- Blobs: `<conversations>/<id>/artifacts/<sha256>.<ext>`, outside `workspace`, private tree,
  read-only after publish, Windows `protect`.
- Index: in memory, rebuilt from the journal's artifact events at load, keyed by `artifactId`.
- Caps: screenshot 16 MiB, download 100 MiB, recording 256 MiB, 1 GiB per conversation. Quota is
  reserved under the engine lock at lease time (the kind's cap, or the remaining quota when
  smaller) and settled to the real size at publish; a lease that cannot reserve its cap is
  still admitted, and its artifact ends `quotaExceeded` without copying. Over a per-file cap:
  hash while streaming, keep no copy, `tooLargeToRetain` with `bytes` and `digest`.
- `conversationDelete` already removes the whole conversation folder.

### 6.2 Ingestion (`agentActionReport`, off the engine lock)

- The reported path must equal the lease's allocated output path byte-for-byte after the MCP's
  rewrite; anything else is `pathMismatch`, name kept, nothing read. A path in the report never
  authorizes reading it.
- Open the allocated directory, check its device and inode against the lease, then open the
  file relative to it without following links (Unix `openat(dirfd, name,
  O_RDONLY|O_NOFOLLOW|O_NONBLOCK)`; Windows the existing `open_options` with reparse points not
  followed, `windows.rs:221`). On the opened handle: regular file (`notRegularFile`), owned by
  the current user (Unix `st_uid`, Windows owner SID equals the token user, `token_sid`,
  `windows.rs:168`; else `notOwned`), link count 1 (`linked`).
- Record identity (device, inode or file index), length and mtime; stream copy and hash into
  `artifacts/incoming/<artifactId>.part` (create-new, private); then `fstat` again. Any change in
  identity, length or mtime, or copied bytes different from the length → `changed`, part file
  removed. Otherwise fsync, make read-only, rename to `<sha256>.<ext>` (an existing blob with
  that digest is kept), journal `artifactReady`.
- Parallel same-name downloads land in different `<actionId>` directories, so
  `move_downloaded_file` (`actions.rs:14711`) never targets the same destination twice.
- Type: magic-byte sniffing (PNG, JPEG, WebP, GIF, PDF, ZIP/OOXML, WebM/Matroska `1A 45 DF A3`,
  MP4 `ftyp`, HTML, plain text). `typeMatches` compares with the file extension; an HTML error
  page saved as `.pdf` becomes `typeMatches: false`. Zero bytes is `Ready` with `bytes: 0` and the
  card says empty.
- Failures from the native codes: `download_canceled` → `cancelled`, `download_interrupted` →
  `interrupted`, `download_timeout` → `timeout`.

### 6.3 Restricted read

`conversationArtifactRead {conversationId, artifactId, offset, length}` (10 s). The artifact
must be in that conversation's index (cross-conversation ids are refused with the same error as
unknown ids). Chunks of at most 384 KiB raw, so base64 plus envelope stays under the service
frame (`MAX_FRAME` 768 KiB, `service.rs:23`) and the native-messaging reply limit. Every chunk
reply carries the blob's `digest` and `total`; each call opens the blob without following
links and checks identity and length against the index before reading. The final chunk call
re-hashes the blob. The panel hashes the assembled bytes with `crypto.subtle` and discards them
on mismatch, so bytes from two versions of a file are never shown as one. A missing or changed
blob returns `artifact_unavailable` and journals `artifactUnavailable {detectedBy: "read"}` once.
The panel never receives a path.

### 6.4 Recordings

- Consent per message: the composer gets a "Record this task" toggle, sent as `record: true`
  in `conversationSend` (`Send` struct, `service.rs:753-764`), stored as `Message.record`
  (in `fingerprint()` only when true, so existing IDs are unchanged).
- `chrome_use_record start` → `agentRecordingAuthorize` refuses unless the active message has
  `record`; otherwise allocates `<workspace>/.chrome-use-out/<artifactId>/recording.webm`,
  journals `artifactCreated {state: "capturing"}` and returns the path. The MCP runs the existing
  `record start <path>` (`commands.rs:3501-3523`).
- `chrome_use_record stop`, or the host after `turnEnded` if still capturing: `record stop
  --expect-path <path>` for the `panel-<id>` session, run the way `close_browser_session` runs
  `close` (`service.rs:1862-1897`), in a task spawned off the engine lock. `expectPath` means a
  late stop for turn N can never stop turn N+1's recording. `artifactState {finalizing}`, then
  ingest as in 6.2 → `artifactReady` only when the sniffed container is WebM; ffmpeg not
  spawnable (`recording.rs:135-140`) → `encoderMissing`; ffmpeg ran but failed
  (`recording.rs:194-203`, e.g. no libvpx) → `encoderFailed` with a redacted stderr tail; a file
  with another container → `typeMatches: false`.
- The next turn is never held for a finalize. Only its own `chrome_use_record start` waits, up
  to 15 s outside the lock, for the previous stop task of the same conversation, because the
  daemon refuses a second active recording (`recording.rs:38-40`); after that it returns
  "previous recording is still finishing" to the agent.
- A finished answer never implies a ready recording. Recording is never started for progress
  display; no message flag, no recording.

## 7. Privacy minimization

### 7.1 Location

New `cli/src/agent_panel/redact.rs`. `Store::event` changes its `data: Value` parameter to
`data: Redacted` (constructed only by `redact::event`). `Engine::record` calls
`redact::event(kind, data, &mut self.secrets[id])` for every write, including `user`,
`queueEdited`, `request` and all new kinds. `conversationRead` applies `redact::legacy` to
events without `v` / `redactions` (display and export minimization for old journals; the file
is not rewritten).

### 7.2 Rules

Schema-driven by event kind, tool name and field, not by keyword-blacking all text.

Mask format keeps the JSON type. A masked string becomes the string
`[redacted <kind>, <n> chars]`; inside a URL, only the value is replaced
(`?access_token=[redacted]`). The machine-readable record goes in a sibling
`redactions: [{path, kind, length}]` on the event data. No hash: a hash of a short password is
guessable. An old panel that calls `String(...)` on a field (`panel-state.js:59`, `82`) shows
the marker text, never `[object Object]`.

Masked values:
- `fill`/`type` `value`/`text` when the selector, label or name matches the sensitive lists
  in `native/sensitive.rs` (made `pub(crate)` and shared, not copied), when the command is
  `secret`, or when the value has a secret shape.
- Argument or JSON keys: `password`, `passwd`, `pwd`, `secret`, `token`, `apiKey`,
  `api_key`, `authorization`, `cookie`, `set-cookie`, `otp`, `cvv`, `cvc`.
- Headers: `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`,
  `x-auth-token`, `x-csrf-token` (network_route arguments, results).
- URLs everywhere (arguments, results, targets, stored `PageContext.url`): userinfo removed; query
  and fragment values masked for `token`, `access_token`, `id_token`, `refresh_token`, `code`,
  `key`, `api_key`, `apikey`, `sig`, `signature`, `secret`, `password`, `session`,
  `sessionid`, `sid`, `auth`, `jwt`, `otp`, `ticket`, `state`, `X-Amz-Signature`,
  `X-Amz-Credential`, `X-Amz-Security-Token`, `X-Goog-Signature`.

Message identity and the stored projection (P1-4):
- `conversationSend` builds the raw `Message` and `accept()` fingerprints it with today's rule
  (`conversation.rs:73-86`), unchanged. The raw `Message` is then projected (page URL redacted)
  before it is pushed to the queue, saved in `state.json` or journaled as `user`. The `accepted`
  map stores the legacy hash, so its format and meaning do not change.
- A retry is a duplicate when (a) its legacy hash equals the stored one, which is the normal
  case because the panel retries from `pendingMessages.params`, the raw values it sent first
  (`sidepanel.js:466`, stored at `508`); or (b) its projection equals the stored projection of that id,
  for a client that only holds the redacted form. For (b) the service keeps a lazily built map
  `messageId → hash(projection)`, filled from the journal's `user` events with `redact::legacy`
  applied, so messages accepted by v1.5.214 (whose `user` events hold the raw URL) match too.
  Neither matches → the existing "reused for different text" error.
- `edit_queued` fingerprints the replacement over the stored, projected page
  (`conversation.rs:332-336`). A replacement id is never accepted twice
  (`conversation.rs:319-321`), so there is no retry to match.
- The agent receives the projected URL in the prompt and still has the exact tab id.

Free text:
- Secret shapes: JWT, `Bearer <token>`, `sk-…`, `gh[pousr]_…`, `xox[abpr]-…`, `AKIA…`, PEM
  private keys, cookie strings (`name=value; …` with values of 8+ characters).
- Known secrets: every value masked above is added to a per-conversation in-memory set (≤64
  values, 8 to 4096 bytes) and scrubbed as an exact substring from later display text only:
  `textDelta.text`, `textFinal.text`, text blocks inside tool results, approval display fields,
  assessment texts and error/detail strings. Never from `messageId`, `actionId`, `artifactId`,
  `requestId`, `toolCallId`, `itemId`, `sessionId`, sequences, cursors or digests; ids are
  extracted before scrubbing (3.4).
- Streaming text: a per-item `TextScrubber` holds back the last `max(longest known secret − 1,
  longest shape prefix)` bytes, capped at 4096, and matches known secrets (spaces included) and
  shapes over the held window plus each new chunk, so a secret split across any number of
  `textDelta` chunks is caught. `textFinal` is scrubbed whole. The held tail is flushed,
  scrubbed, as a final `textDelta` on `terminal`, `exited` and `processUnconfirmed`; OpenCode
  ACP only ever sends `agent_message_chunk` deltas (`acp.rs:129-131`), so without the flush its
  last words would never be written. A secret first seen after some text was written cannot
  retract that text; the privacy note says so.
- Free text caps: eval result 2000 characters, command output 4000, page text in results 6000
  (existing preview size), each after scrubbing.
- Image blocks: data removed before `short()`; pixels live only in artifacts.
- Kept on purpose, because approvals and review need them: URL host and path, amounts and
  numbers, recipient and destination fields that are not secret keys, exit codes, error codes,
  tool names, durations, digests, sizes, tab ids.
- Not redacted: the user's own message text (it is the user's record and is already in the
  CLI's own session files). Assistant text is redacted only by the known-secret and shape
  rules above. Pixels in screenshots and recordings and bytes of attachments and downloads are
  not redacted. Documented in the panel's privacy note and `PRIVACY.md`; the backend CLIs' own
  session files are outside this service's control.

`privacyClass` is the strongest class present: `masked` if anything was masked, else
`userInput` for fill/type values, `pageContent` for page-derived text, else `public`.

### 7.3 Approval replies bound to the request the user saw

The panel builds `can_use_tool` replies from the journal's `params.input`
(`sidepanel.js:371`) and AskUserQuestion replies by spreading it (`sidepanel.js:345`). Once
the journal holds redacted input, echoing it would send the masked text to Claude as
`updatedInput`. Today the reply is matched only by `(conversation, requestId)`
(`service.rs:633-646`, `1391-1396`) and `can_use_tool` accepts any `updatedInput`
(`service.rs:1503-1506`).

- `Engine.requests` keeps per request: the unredacted original, `generation`, a
  `requestRevision` (engine-wide counter), and `sent`: the exact redacted projection journaled
  in that revision's `request` event, frozen at that moment. The `request` event carries
  `generation` and `requestRevision`.
- A request arriving with an id already present in the conversation (a re-sent request, or a
  new process reusing JSON-RPC ids) gets a new revision; the old revision is dead.
- New panel reply: `{conversationId, requestId, generation, requestRevision, decision}` with a
  closed decision per method: `allow`/`deny` (`can_use_tool`), `answers: {questionId: {option}
  | {custom}}` (AskUserQuestion, `item/tool/requestUserInput`), `optionId` (ACP), `decision`
  (Codex approvals), `accept` (permissions, elicitation), `decline`/`cancel`. The service builds
  the backend result from the original: `updatedInput = original.input` for allow; for
  AskUserQuestion, `original.input` with only its `answers` field set from the validated
  answers (each question present in the original, each option one of that question's labels, a
  custom answer only where allowed and ≤2000 characters, single-select gets one value). Client
  data is never merged into the original. Permissions are taken from the original.
- Refused: a stale generation or revision, unknown fields, any `updatedInput` on the new shape,
  a second allow for the same revision (the existing `sent` check, `service.rs:1397-1401`), and
  allow while `Stopping` (5.2). `dispatched` is not `confirmed`, as today.
- Old panel reply (no `requestRevision`): accepted only while that request id has had exactly
  one revision. `can_use_tool`: `updatedInput` must equal the frozen `sent.input`, then the
  original is substituted. AskUserQuestion: `updatedInput` without `answers` must equal frozen
  `sent.input` without `answers`, and `answers` is validated as above, then built from the
  original. Codex `item/permissions/requestApproval`: `permissions` must equal the frozen
  projection, then the original is used. The comparison is against the frozen projection, never
  one recomputed with a secret set that grew since.
- Raw inputs never reach disk through a side path: validation errors are fixed strings (as in
  `validate_reply` today), the service writes no request payloads to logs, and a test checks
  every file in the profile directory after a refused reply.
- ACP replies only carry an `optionId` validated against the original options; Codex replies
  carry decisions. Neither echoes input. `automatic_approval` already uses the original
  (`service.rs:1443-1462`).

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
      actions: Map<actionId, {admitted, target, observed, linkedStep, late}>, actingPages: [Target],
      artifacts: Map<artifactId, Artifact>, assessment: {provisional, final}, stalls,
      batchInterrupted, compactions, control, recovery, legacy: {grouping, verification, pages}}
```

- A turn exists from its `user` event (state `queued`), starts at `turnStarted`, ends at
  `turnEnded`. `queueEdited` moves the turn to `replacementId` (as rows do today), so replay
  gives one turn. `tool`/`text` events join the turn named by their `messageId`.
- Evidence and tool events merge in either order: `actionAdmitted` or `browserActionTarget`
  creates the action; a `tool` event with `actionIds` links it; whichever arrives second
  completes the link. Late reports join their own turn and are marked late.
- `unboundActionReport` is never attached to a turn; it appears only in the recovery review.
- Requests are keyed by `(requestId, requestRevision)`; a new revision replaces the card and
  the old card's buttons are removed.
- Legacy journals: no `turnStarted` → the turn starts at the first event with its
  `messageId`; events without `messageId` (oldest journals) attach to the preceding user row
  with `legacy.grouping = true` and the card says "Step grouping was not recorded". No
  `taskAssessment` → "Checks not recorded". chrome_use steps without actions → "Page not
  recorded". No artifact cards are created from paths in tool text.
- Rendering-only: no field is filled from text heuristics.

### 9.2 Turn card (`sidepanel.js`, `sidepanel.css`, `sidepanel.html`)

`renderTimeline` (`sidepanel.js:220-280`) renders turns instead of a flat row list, keeping
the keyed incremental DOM (`rendered`), scroll-follow (`follow`, `latest`) and per-group
`touched` state. Per card:

1. User message, attachments (existing `conversationAttachmentRead` images), and
   **Sent with**: the attached page's title and host (from `user.page`).
2. **Acted on**: distinct `actionReply` targets from the turn's actions, first three plus
   "+n"; `chromeTabId` equal to the attached tab says "the page you sent". `daemonState`
   targets are listed as "Browser tools pointed at". "Asked for X, landed on Y" only when both
   are `actionReply`. Pages the user only switched to never appear, because nothing but
   evidence writes this line.
3. Steps: the existing `toolGroup` now scoped to the turn (not to adjacency). Each step shows
   its action verdict chip (Confirmed / Failed / Not checked / No browser record). A folded
   group lists failed step names in its summary, so a failure is never hidden by folding.
   Unresolved steps from `batchInterrupted` say "did not finish; may have run" (`reported`,
   `admitted`) or "no record that it ran" (`unknown`). Late reports say "reported after the
   request ended".
4. Answer: assistant text through `renderMarkdown`; with `role` known, commentary is shown
   smaller above the final answer; without it, all text is "Response". `emptyAnswer` shows
   "The agent returned no answer text."
5. Result cards per artifact: screenshot thumbnail (chunked read → `Blob` → object URL,
   revoked on removal, SHA-256 checked with `crypto.subtle` before display); download name,
   size, type and "matches .pdf: yes/no"; recording with capturing/finalizing/ready. Lines,
   separately: "File saved", "Type matches", "Not empty". A file never says it meets the
   request. Unavailable cards give the reason. Save uses an `<a download>` on the blob.
6. Assessment badge (final only): "Agent's browser checks passed" / "Some checks passed" /
   "Not checked" / "Blocked" / "Checks not recorded"; "Assessment pending" before `turnEnded`.
   Expanding lists each requirement with its computed status and, for checked ones, the exact
   predicate, page and time ("page showed 'Order placed' on shop.example/orders"), plus
   `downgradedBecause`. A line under the badge: "These are the checks the agent chose; they do
   not prove the whole request was done."
7. Outcome line: Completed / Failed / Interrupted / Outcome unknown; stall banner while
   `progressStalled` is open (Stop, Keep going (local dismiss), no automatic action).
8. Recovery block on the unknown-outcome turn: "Review what may have happened"
   (`conversationRecoveryReview`), "End the old agent process" when the review says `alive`
   (`conversationRecoveryTerminate`), "Continue in a new conversation" only when the review says
   `exited` (`conversationRecoveryFork`, then `select(newId)`). With `unknown`, neither button
   is shown and the text says why. Forked conversations show a link both ways.

The status line (`#status`) shows `control` alongside the phase: "Agent working", "Stop
requested, waiting for confirmation", "This conversation's agent has stopped", "Resuming",
"Agent state unknown". Approval cards (`requestView`, `333-379`) disable Allow while `control`
is `stopRequested` and send the new reply shapes (7.3).

### 9.3 Bridge (`agent-panel-bridge.js`)

Add `conversationArtifactRead`, `conversationRecoveryReview`, `conversationRecoveryTerminate`,
`conversationRecoveryFork` to the allowlist, `operationTimeouts` (12 s, 17 s, 17 s, 12 s) and
`validResult` (artifact chunk: matching `artifactId`, integer `offset`, string `data`,
`total`, `digest`; review: `process.state` in `exited|alive|unknown`; terminate: boolean
`confirmed`; fork: new `conversation.id` and `forkedFrom === conversationId`). The agent-only
ops are not in the panel allowlist. `capabilities()` (`agent_panel.rs:570-577`) adds
`turnEvidence: 1`, `artifacts: 1`, `recovery: 1`, `recording: 1`, `requestRevision: 1`; the
panel renders the legacy view and sends legacy reply shapes when they are absent (old CLI with
new extension).

## 10. Migration and compatibility

- Old journals: no rewrite. New kinds are additive; `PanelTimeline.apply` already ignores
  unknown kinds while advancing the cursor, so an older panel on a newer journal still works.
- Old CLI with new panel: capability flags absent → legacy rendering, legacy reply shapes, no
  new ops sent.
- New CLI with old panel: new events ignored; masked fields are strings, so nothing renders as
  `[object Object]`; approval replies follow the legacy rules in 7.3; a re-sent request with a
  reused id is answered by an old panel only while it has one revision.
- Message identity across the upgrade: a message accepted by v1.5.214 with an `access_token`
  page URL and an attachment, whose reply was lost, retried by the panel after the upgrade with
  the same id: the legacy hash in `state.json` matches the raw retry → duplicate; a retry carrying
  the redacted projection matches through the journal-derived projection map → duplicate.
  Neither resends nor reports a conflict.
- Downgrade: state written by this build has the same `Conversation` shape and legacy hashes, so
  v1.5.214 opens it and still dedupes; queued messages hold the redacted URL, which v1.5.214 sends
  to the agent as-is.
- `Conversation` unchanged; `Message` gains `continuation` and `record`, both skipped when
  empty, `record` in the fingerprint only when true.
- Process records, leases, evidence dedupe, artifact indexes and the projection map are rebuilt
  from the journal in the same pass `Store::load` already makes over it (`last_sequence`,
  `conversation.rs:692-728`), except the projection map, which is built on first need.
- Running old service: `ensure()` replaces idle services of another build and refuses while
  one is active (`service.rs:211-298`); the panel's capability flags come from the running
  host, so a mixed state shows the legacy view until the new service is live.
- Fixtures: a v1.5.214 build on the build host produces a conversation folder (journal,
  `state.json`, attachments) and recorded `capabilities` and `conversationRead` replies; the
  v1.5.214 `panel-state.js` and `sidepanel.js` are taken from the tag. These drive the oldCLI/
  newPanel and newCLI/oldPanel tests in 14.1 and live check 15.

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
| actionId survives in tool result | observed: `item.result.content[0]` marker and `item.result.structuredContent` both kept | observed: marker text **dropped** when `structuredContent` is present (`tool_result.content` becomes its JSON; `tool_use_result.structuredContent` kept); marker kept in `content[0]` without it | observed: `result.content[0]` kept; whole MCP result under `result.structuredContent` | observed: model-dependent; MCP runs inside codemode `execute`, ACP `content` is the script's return value (`structuredContent` JSON when present), `rawOutput` has no result |
| Browser target and effect | host (MCP side channel) | host | host | host |
| Artifacts | host | host | host | host |
| Final vs commentary text | `agentMessage.phase` observed (`commentary`, `final_answer`); else unknown | `result.result` → final | unknown | unknown |
| Context compaction signal | `contextCompaction` item observed (`thread/compacted` not observed) | `system/compact_boundary` observed (`compact_metadata.trigger`) | `compaction_start`/`compaction_end` with `reason` observed (`auto_compaction_end` not observed) | unknown |
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
- `agent_panel/redact.rs` (new): `Redacted`, `PrivacyClass`, `Secrets`, `TextScrubber`, `event`, `legacy`, `url`, message projection.
- `agent_panel/artifacts.rs` (new): output allocation, quota reservation, no-follow ingest, sniff, publish, read chunk, index rebuild.
- `agent_panel/recovery.rs` (new): process record, ownership proof, probe, terminate, fork summary.
- `agent_panel/progress.rs` (new): stall detector and tick.
- `agent_panel/conversation.rs`: `Message.continuation`, `Message.record`, `prompt_text`, `fingerprint` (rule unchanged), projection after accept, `summary.control`, `Store::event(Redacted)`, load-time rebuild hooks.
- `agent_panel/service.rs`: `Engine::begin`, leases, `turnEnded` and follow-ups in `notice`, `record` split (evidence kinds skip state save), `approve_pending` and `conversationRespond` stop rules, request revisions and reply construction, `held_jobs`, new ops, `call_existing`, `operation_seconds`, `progress_tick`, recording stop after turn end.
- `agent_panel/backend.rs`: tool notice `toolCallId`/`actionIds`/image stripping, Claude `textRole`, Codex `phase`, compaction notices, `ready.pgid/processStartTime/serviceSid`, `Setup.generation`, env `CHROME_USE_PANEL_GENERATION`, prompt, Windows job hand-back on unconfirmed exit.
- `agent_panel/windows.rs`: job handle passed out of `drive`; owner SID check for ingest.
- `agent_panel/acp.rs`: tool notice fields.
- `agent_panel.rs`: `report_panel_evidence`, lease call, panel tool routing for `chrome_use_task`/`chrome_use_record`, new ops in `handle_for_profile`, `capabilities()`.
- `panel_evidence.rs` (new) and `mcp.rs`: lease, output path rewrite, env to CLI, verdict and check derivation, report with idempotent retry, marker, `observe` argument, panel tools.
- `connection.rs`: `_panelActionId`.
- `native/daemon.rs`, `native/browser.rs`: `actionContext` with provenance, `acting_target`, scoped request tracking.
- `native/actions.rs`: download error codes, scoped tracking restore.
- `native/recording.rs`, `native/commands.rs`: `record stop --expect-path`.
- `native/sensitive.rs`: share the sensitive lists `pub(crate)`.
- `output.rs`: `record stop --expect-path` in `--help`.

Extension (`extensions/ab-connect/`): `panel-state.js` (turn projection, request revisions),
`sidepanel.js` (turn cards, result cards, recovery, control line, approval reply shapes, record
toggle), `sidepanel.html` (record toggle, status control span), `sidepanel.css` (turn card,
chips, result cards, banners), `agent-panel-bridge.js` (ops, timeouts, validators).

Docs, per `AGENTS.md:20-29`, in the same tag as the feature:
- `README.md` and `README.zh.md`: side panel section (turn cards, what the check badge means and
  does not mean, recording consent, recovery), `record stop --expect-path`, MCP `observe`
  argument.
- `skill-data/core/SKILL.md` and `skill-data/core/references/` (the MCP and panel reference):
  `observe` on acting tools, the `chrome-use action <id>` marker, `chrome_use_task assess` with
  check kinds, `chrome_use_record`. `skills/chrome-use/SKILL.md` stays a stub.
- `docs/*.html` and `docs/en/*.html`: the side panel page and the privacy page, both languages.
- `PRIVACY.md`: what the journal stores, what is masked, what is not (user text, pixels, file
  bytes, backend CLI session files), and that later-discovered secrets cannot retract text.
- `CHANGELOG.md`, `docs/changelog.html`, `docs/en/changelog.html`.

## 13. Implementation order (one release, one branch)

Each step lands with its unit tests; the release is cut only after section 14 passes on the
build host.

1. **Protocol fixture capture on the build host** (`leo@192.168.0.190`): run each installed
   backend once with a chrome_use MCP that returns a marker block; save raw protocol lines as
   test fixtures under `cli/src/agent_panel/fixtures/`. Settles the *verify* cells of
   section 11 before anything depends on them. Also capture the v1.5.214 compatibility fixtures
   (section 10) and run the macOS PID-reuse test for 5.3.
2. **Redaction gate**: `redact.rs`, `Store::event(Redacted)`, type-preserving masks, message
   projection after fingerprint, legacy read redaction, request revisions and reply
   construction, stop-time approval rules. Everything after this writes through it. The
   existing `Store::event` tests (`conversation.rs:811-1057`) move to a test-only `Redacted`
   constructor.
3. **Turn lifecycle**: `Engine::begin`, `turnStarted`, `turnEnded` from the message snapshot,
   `toolCallId`, open tools, `batchInterrupted`, `emptyAnswer`, `continuation`, `control` in
   summary, `textRole`/`phase`.
4. **Native stamping**: `_panelActionId`, `acting_target`, `actionContext` with provenance,
   scoped request tracking, download codes, `record stop --expect-path`.
5. **Leases and the MCP side channel**: `agentActionBegin`, `panel_evidence.rs`, report op with
   idempotent replay and unbound bucket, `call_existing`, marker, `observe` argument, adapter
   extraction, `browserActionTarget`/`actionObserved` with checks.
6. **Artifacts**: allocation, quota reservation, no-follow ingest, publish, chunked read,
   recordings and consent.
7. **Assessment**: `chrome_use_task`, `agentTaskAssess`, check verification, freshness,
   provisional and final, prompt text.
8. **Recovery**: process record, Windows job hand-back, Unix ownership proof, probe,
   review/terminate/fork, fork preface.
9. **Progress and continuity**: detector, tick, `turnSummary`, `contextCompacted`.
10. **Side panel**: bridge, projection, turn cards, result cards, recovery UI, legacy view,
    revision-keyed request cards.
11. **Acceptance on the build host**, docs (section 12 list), release notes, release.

## 14. Acceptance

### 14.1 Unit tests (per rule)

Rust, in the module that owns the rule:
- `redact`: each masked key; each header; each URL parameter including fragments and userinfo;
  each secret shape; cookie strings; masks are strings of the same JSON type plus `redactions`;
  known-secret exact scrub across later events in display fields only; a known secret equal to a
  fragment of a `messageId`, `actionId` and `requestId` leaves those ids and their links intact;
  streaming scrub of a secret with spaces split across 2, 3 and 7 `textDelta` chunks at every
  split offset; held window never exceeds its cap; long unbroken run; caps; kept fields (amount,
  recipient, host, path, exit code) survive; `privacyClass` per case; `Store::event` cannot be
  called with a raw `Value` (compile-time, plus a doc test); legacy read redaction leaves the
  file bytes unchanged.
- Message identity: fingerprints of messages without new fields equal golden hashes from
  v1.5.214; a raw retry and a projected retry of a message accepted by the v1.5.214 fixture
  (token URL + attachment) are duplicates; a changed text under the same id is still refused;
  `state.json` written by this build is read by the v1.5.214 `Conversation` type and dedupes.
- Approvals: new shape allow sends the original; AskUserQuestion sets only `answers` from
  validated options; a custom answer where not allowed, an unknown question, an unknown option,
  two values for single-select, unknown fields, an `updatedInput` on the new shape, a stale
  generation, a superseded revision, a second allow and allow during `Stopping` are refused;
  legacy `can_use_tool` echo equal to the frozen projection is accepted and substituted; a legacy
  echo equal to a projection recomputed after the secret set grew but different from the frozen
  one is refused; legacy AskUserQuestion with spread input plus answers is accepted only when the
  rest equals the frozen projection; a legacy reply for a request id with two revisions is
  refused; ACP and Codex shapes unchanged; after a refused reply carrying a `SYNTH_` value, no
  file under the profile directory contains it.
- Stop rules: request during `Stopping` with full access is declined, not approved;
  `conversationRespond` accept during `Stopping` is refused, decline passes.
- Leases: begin refused in `Stopping`, `RecoveryRequired`, `Idle` and for a stale generation;
  same generation, turn A then turn B: an action leased in A and reported during B is journaled
  under A with `afterTurnEnd`; a stale-generation report and a report with no lease go to
  `unboundActionReport` and never appear in a turn, summary or assessment; an artifact copy
  that spans A's `turnEnded` and B's `turnStarted` lands under A; a report whose reply was lost
  and is retried returns the first answer, including an artifact's `Ready` or `Unavailable`
  state; a lease left `Reporting` across a simulated crash reloads with `interrupted` artifacts
  and a retry gets those states; `turnEnded` carries the active message id captured before
  `terminal()` cleared it.
- Turns: each of the four `begin_next` sites journals exactly one `turnStarted`; `turnEnded`
  outcome for completed, failed, interrupted, exited-while-stopping, processUnconfirmed;
  `emptyAnswer`; `batchInterrupted` with reported, admitted and unknown execution; continuation
  set in `Engine::begin` after an interrupted turn (including a message queued before the
  interrupt and the Stop & send direct message) and not after completed; `edit_queued` keeps
  `record`; schedule journals `user` before `turnStarted`; scrubber tail flushed on each terminal
  kind (ACP delta-only fixture).
- Download codes: relay `downloadUrl: interrupted (USER_CANCELED)` → `download_canceled`,
  other reasons → `download_interrupted`, extension timeout → `download_timeout`, CDP
  `canceled` → `download_canceled`.
- Evidence: verdict and check derivation table (every rule in 3.2), timeout → possiblyExecuted
  for non-idempotent actions; duplicate lease refused; report failure leaves the tool result
  successful with the note; adapters extract actionIds from each backend fixture (step 1) and
  ignore markers in non chrome_use items; image data stripped before `short()`.
- Native: `actionContext` present only with `_panelActionId`, no CDP traffic added (assert on
  the mock client's sent-command log), chromeTabId from `cb-tab-<id>` and null when launched,
  provenance `actionReply` for navigation, popup adoption and verified tab select and
  `daemonState` otherwise; panel observe with tracking off leaves `request_tracking` false and
  `tracked_requests` at its old length afterwards, on success and on error, and sends no
  `Network.disable`; with tracking already on (after `requests --clear`) nothing changes; HAR
  recording during a panel observe keeps its entries; plain CLI `--observe` unchanged.
- Assessment: `checksPassed` only with every requirement checked through a matching predicate on
  this turn's evidence; counterexamples each yield `claimed` or `checkFailed` and never
  `checksPassed`: an expect whose recorded predicate differs from the supplied one, a successful
  `open` cited for "pay", evidence from an earlier turn, an `unbound` report, a zero-byte
  download, a `.pdf` that sniffs as HTML, a screenshot cited as the check, a check followed by
  another acting action on the same target (`stale`); claimed-only becomes `notChecked`;
  partial; blocked; cross-conversation ids unknown; a non-completed outcome caps the final
  status at `someChecksPassed`; the final assessment is journaled after `turnEnded` and a late
  report does not change it.
- Artifacts: content addressing; parallel same-name downloads → two allocated directories, two
  artifacts with the right bytes each; a reported path other than the allocation →
  `pathMismatch`; symlink at the allocated name, a hard link, a FIFO, a file owned by another
  user (where the test can create one) → refused without reading; overwrite, truncate, rename
  over, and symlink swap during the copy → `changed`, never `Ready` with mixed bytes; quota
  reserved by two concurrent leases never exceeds the conversation quota; caps →
  `tooLargeToRetain` with digest; sniffing incl. HTML saved as PDF, zero bytes, WebM; chunk
  bounds; blob replaced between chunks → panel digest mismatch and server `changed`; deleted blob
  → one `artifactUnavailable`; cross-conversation read and cross-conversation ingest refused;
  recording refused without `record`; encoder missing; encoder failed.
- Recovery: Windows held job: probe counts processes, terminate empties it, after a simulated
  restart the probe is `unknown`. Unix: group gone → `exited`; leader dead, group alive with
  members in the service session → `alive`, terminate confirms; a member in another session →
  `unknown`, no signal sent; leader alive with mismatched start time → `unknown`; fork refused
  while `alive` or `unknown`; fork creates no queued message and no backend session; preface
  content bounded.
- Progress: four identical reads stall; six paginated reads with differing digests never stall;
  `wait` excluded; repeated failure; noActivity suppressed by an open long tool and by a pending
  approval; resumed episode.
- Store: new event kinds within `MAX_EVENT` at their caps; evidence kinds skip state save and
  reload still succeeds.

JS (`node --test`):
- `panel-state.test.js`: turn per messageId; queueEdited replacement; evidence before/after
  tool event links identically; late reports join their own turn; unbound reports never join a
  turn; request cards keyed by revision; unlinked evidence and unlinked steps; legacy journal
  fixtures (from the v1.5.214 build) render "not recorded" labels and never a verdict; replay
  from 0 and incremental replay produce deep-equal turns; unknown kinds advance cursor.
- `sidepanel.test.js`: acting line uses only `actionReply` targets and excludes pages only
  switched to; folded group names failed steps; approval Allow disabled while
  `stopRequested`; new reply shapes; recovery buttons gated by review state, none for
  `unknown`; artifact digest mismatch shows "changed"; the badge text never contains
  "Verified".
- Compatibility: the v1.5.214 `panel-state.js`/`sidepanel.js` render a journal written by this
  build with no `[object Object]` and no thrown error, and their approval replies pass the
  legacy rules; this build's panel against the recorded v1.5.214 `capabilities` and
  `conversationRead` replies renders the legacy view and sends legacy reply shapes.
- `agent-panel-bridge.test.js`: allowlist, timeouts, validators for the four new ops.

### 14.2 Live checks on the build host

Run on `leo@192.168.0.190` against a local fixture site (`scripts/panel-acceptance/site/`):
a form whose save is rejected server-side, a paginated list, same-name and slow/aborting
download endpoints, a password form, a page that sets a cookie and has `?access_token=` links,
a redirecting link and a link that opens a popup.
Deterministic scenarios use fixture backends (`scripts/panel-acceptance/backends/`, one per
protocol, launched through the conversation `executable` override) that drive the real
`chrome-use mcp` server from the config the host writes, so evidence flows through the real
MCP, daemon and Chrome. Each installed real backend also runs scenarios 1, 4 and 12 once.
Each check asserts on `conversationRead` output and on the files in the conversation folder.

1. **Tool failure then recovery**: a click fails, a second selector succeeds; the failed step
   stays visible in the folded group; outcome completed; assessment per checks.
2. **Empty answer**: backend completes with no text; `emptyAnswer`, final assessment
   `notChecked/notAssessed`.
3. **Partial completion**: two requirements, one checked by an `expect` with a matching
   predicate, one only claimed → `someChecksPassed`; a "met" declaration citing an unrelated
   expect is `checkFailed`; a "done" declaration is downgraded with reasons.
4. **A, B, C pages**: send from tab A; the agent opens B and acts there; the user activates C
   during the turn. Sent with A; acted on B with `actionReply` provenance; no event mentions C;
   the chip shows C for the next message only. A redirecting link shows "landed on"; a popup
   click lists the popup tab.
5. **Stop timeout with process alive**: a fixture backend ignores interrupt and spawns a child
   that survives the leader; with the e2e fault switch that skips the kill
   (`#[cfg(feature = "e2e-tests")]`), the service reports `processUnconfirmed`; `control` is
   `unknown`; review says `alive` with proven ownership; fork is refused; terminate confirms the
   whole group gone; fork then succeeds and sends nothing. After restarting the service with the
   group still alive and the ownership test available, review still proves it; on the Windows
   CI runner, after a restart review says `unknown` and offers neither terminate nor fork.
   Without the switch, the 10 s timeout kills the group and `control` becomes `stopped` only
   after `exited`.
6. **Approval/stop interleave**: a pending approval, then Stop: Allow is disabled and refused by
   the service, full access does not auto-approve a request arriving during stop, the turn ends
   interrupted. No `actionAdmitted` has a sequence after the stop; a slow action admitted before
   Stop reports afterwards and appears under its turn with `afterStop: true` and
   `possiblyExecuted`.
7. **Same-name downloads**: two parallel tool calls saving `report.pdf` with different content;
   two artifacts, two digests, each blob matching its source; both readable after the source
   files were overwritten.
8. **Interrupted download**: the aborting endpoint on the relay path (the one panel
   conversations use) → `interrupted`, no Ready card. User cancellation on the relay path
   (`downloadUrl: interrupted (USER_CANCELED)`) → `download_canceled` → `cancelled` is covered
   by a unit test of the mapping, not live: the harness cannot press Chrome's cancel without
   driving the user's download UI.
9. **Deleted artifact file**: delete a blob; the next read reports unavailable; exactly one
   `artifactUnavailable {detectedBy: "read"}` across repeated reads and two open panels.
10. **Cross-conversation artifact read**: reading conversation B's artifact through A is
    refused; so are non-UUID ids.
11. **Synthetic secrets in the journal and state**: fill `SYNTH_PW_…` into the password field,
    eval `document.cookie` containing `SYNTH_COOKIE_…`, open a link with
    `access_token=SYNTH_TOK_…`, route with `Authorization: Bearer SYNTH_BEARER_…`, have the
    backend echo the cookie in its answer split across deltas, and approve one of these steps
    with approvals on. `grep -r SYNTH_` over the journal, `state.json`, `index.json` and
    `conversationRead` output finds nothing; the approval still executed with the real value.
    This covers those files only: pixels in screenshots, artifact and attachment bytes, the
    user's own message text and the backend CLIs' session files are outside it, and the check
    does not claim them.
12. **Stalled progress**: four reads of an unchanged page raise one `progressStalled`; reading
    six pages through "next" raises none; a 200 s silent fixture raises `noActivity`; a 60 s
    `diag pages --watch` raises none.
13. **Refresh resume consistency**: during a running turn, close and reopen the panel and open
    a second panel; both projections deep-equal a fresh read from 0; no duplicate cards.
14. **Replay producing one turn**: drop the native reply after the host stored the message
    (bridge fault injection), retry the same id; one `user`, one `turnStarted`, one turn card.
15. **Old journal and upgrade retry**: a conversation folder created by the v1.5.214 build opens,
    shows its history with "not recorded" labels, accepts a new message, and its old lines are
    byte-identical afterwards. In the same folder, a message with an `access_token` URL and an
    attachment accepted by v1.5.214 is retried by the new panel with the same id: no second
    `user` event, no conflict error. The v1.5.214 extension against this build renders turns as
    rows without `[object Object]` and can answer an approval.
16. **Stealth**: during scenarios 1 and 4 the CDP command log for the session contains no
    `Runtime.enable`, no `Console.*`/`Log.enable`, and no domain enable beyond the attach-time
    set that a v1.5.214 run of the same scenario sends (`Page.enable`, `Network.enable`,
    `Target.setAutoAttach`, focus emulation); the page DOM has no added attributes or nodes
    (MutationObserver fixture on the site); no recording without the message flag; after a
    panel action with `observe: true`, the daemon's `request_tracking` is back to false.
17. **Recording**: with "Record this task", capturing → finalizing → ready and the blob sniffs as
    WebM; without ffmpeg on a second run, `encoderMissing`; with an ffmpeg lacking libvpx,
    `encoderFailed`; without the flag, `chrome_use_record start` is refused. Two consecutive
    recorded turns: the second turn starts while the first recording is finalizing, the first
    turn's late stop does not stop the second recording, and both end `Ready`.
18. **Turn attribution under load**: one backend process, turn A runs a slow download, the user
    sends B while A's copy is in progress (Stop & send); the download artifact lands under A,
    B's assessment cannot cite it, and B's card shows nothing from A.
19. **Assessment counterexamples live**: a fixture backend assesses "paid" citing a successful
    `open`, a zero-byte download and a screenshot; the final badge is "Not checked" with each
    reason listed.

## 15. Risks

1. **Approval reply construction (7.3).** Getting it wrong sends masked input to the agent or
   lets a stale card answer a newer request. Mitigated by revisions, server-built results,
   frozen-projection equality for the legacy shape, and the tests in 14.1 and live 11.
2. **Image blocks and `short()`.** Stripping must happen before `short()` in all four adapters,
   or screenshot results keep being cut to an opaque preview and lose the marker. Covered by
   adapter fixtures.
3. **actionId survival per backend.** If a CLI drops tool result text, linking degrades to
   unlinked records under the right turn (the lease still names it). Step 1 measures it.
4. **Evidence authenticity.** Leases and reports come from a process the agent's tool runtime
   started; an agent with shell access (same user) could post a forged report or write into its
   output directory. Labels say "reported by chrome-use's browser tools", not "proven", and the
   badge describes the agent's own checks. Evidence from actions run outside the MCP (shell
   `chrome-use`, forbidden by the prompt) is absent, shown as no browser record.
5. **Fail-closed leases.** A browser action does not run when the service does not answer the
   lease within 5 s. A busy engine lock could turn into tool errors; the lock is never held
   across file copies or process waits, and live check 12 (≥50 actions) measures lease latency,
   target under 20 ms.
6. **Journal write volume.** Three extra fsynced events per action (`actionAdmitted`,
   `browserActionTarget`, `actionObserved`); state saves skipped for evidence kinds. Measured in
   live check 12; per-action overhead under 30 ms on the build host.
7. **Redaction misses free text.** Shape and exact-secret rules do not catch every secret in
   page text. Page text is capped and labelled; the privacy note says so. The known-secret set
   is in memory: after a service restart, a password filled before the restart is no longer
   scrubbed by exact value from later text (shape rules still apply). The turn that filled it
   is `RecoveryRequired` after a restart, so no later text of that turn is written. Text written
   before a secret was known is not retracted.
8. **Process identity.** Unix ownership rests on the kernel not reusing a PID that is a live
   pgid or sid; Linux guarantees it, macOS is tested in step 1 and otherwise reports `unknown`.
   On macOS a group could in principle empty and be reused between the proof and `kill(-pgid)`;
   Linux avoids this with pidfds. Processes that leave both the group and the session
   (`setsid`) escape the probe, as they escape today's group kill. Windows after a service
   restart stays `unknown`, so such a conversation cannot be forked; the user can start a new
   conversation.
9. **Stall false positives.** Advisory only, conservative thresholds, excluded tools; the live
   pagination check guards it.
10. **Mixed versions.** An old service or old extension during upgrade; capability flags,
    additive events, string masks and legacy reply rules keep both directions working, checked
    with real v1.5.214 fixtures.
11. **Panel `observe` and request tracking.** No new CDP domain (Network is enabled at attach);
    the cost is a baseline and post snapshot and a settle wait on steps the agent marks, and
    request bookkeeping that the daemon now drops after each panel observe instead of keeping
    for its lifetime. Checked by live check 16.
12. **Assessment wording.** Users may still read "checks passed" as "done". The card lists each
    predicate beside its requirement and says the checks were chosen by the agent.

## 16. Deliberately excluded (not deferred)

- Automatic `--observe` on every panel action: adds snapshots and settle waits to every step
  for a UI feature (D3).
- A live page preview or periodic screenshots in the panel: would be continuous capture for
  progress display.
- Judging "meets the request" by a second model: the research shows a same-model judge is not
  ground truth; the card reports the agent's checks and the evidence instead.
- Rewriting or back-filling old journals: they are evidence as recorded.
