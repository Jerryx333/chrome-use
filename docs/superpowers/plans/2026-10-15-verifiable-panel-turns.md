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
| P2-2 per-lease output silently replaces the caller's path | Agreed. The capture and the delivery to the requested path are two effects, recorded and checked separately. | 3.2, 6.2a, 2.2, 14.1, risk 14 |
| P2-3 crash points and concurrent duplicate reports | Agreed. A capture verdict is now written last, after the artifact outcome; duplicate reports during ingest get `pending`. | 3.2, 3.3, 14.1 |
| P2-4 pidfd kill and macOS proof | Agreed. Kill loops until the group is empty; the macOS loop test is observation only; proof comes from kernel source or the probe answers `unknown`. | 5.3, 5.4, risk 8 |
| P2-5 naming | Agreed: `sentProjection`; the `sent` dispatched flag keeps its meaning. | 7.3 |
| Approval (seq 26) implementation P2s | Folded in: an explicit lease state machine with a one-time `LateReported` transition; `bootId` plus a capped `notAfter`, monotonic values never stored or used as proof; a dequeued message is kept through a failed retirement (`turnStartFailed`), and non-resumable backends refuse to continue instead of starting a history-less session; generation reuse after clean completion only for fixture-backed CLI versions; step 0 marked landed (`ea4dd857`); fixtures kept as regressions. | D1a, 2.2, 3.3, 11, 13 step 0, 14.1 |
| Fixture findings (step 1, `cli/src/agent_panel/fixtures/README.md`) | Claude drops all text blocks of an MCP result that has `structuredContent`: the actionId and every note now live in `structuredContent.chromeUseAction`, which chrome-use already returns. OpenCode runs MCP calls inside a codemode `execute` wrapper: a step is the wrapper, linked to several actionIds when they survive, otherwise unlinked; the lease still puts the evidence in the right turn. Pi provider failure ends `agent_settled {aborted: false}` after three errored retries: outcome follows the last assistant message. The Codex full-access approval bug is a prerequisite fix. | 3.2, 3.4, 4.4, 11, 13 steps 0 and 3 |

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

**D1a. A generation is a turn epoch; a lease request can only be admitted into its own turn.**
Lease admission must not depend on which message happens to be active when the service gets
the engine lock: a Begin from turn A delayed past Stop could otherwise be admitted under turn B
on the same process. Four rules close that:

1. The generation comes from process context, never from service state. `drive` already builds
   `browser_env` per process (`backend.rs:97`); it adds `CHROME_USE_PANEL_GENERATION`. The MCP
   server is a child of that process and sends this value with every Begin, assess and
   recording authorize.
2. A generation is retired when a turn ends in anything but `completed` with no open chrome_use
   tool call (4.4): `interrupted`, `failed`, `unknown`, or `completed` with an open chrome_use
   tool. Retiring sets `Job.retiring` and sends `Shutdown`, as the options-change path does
   (`service.rs:1556-1583`). `start()` today hands the next message to any existing job
   (`service.rs:1585-1593`); it changes so a retiring job is waited out on that same path and the
   next message runs on a new process with a new generation, resumed through the backend session
   (Codex `thread/resume`, `backend.rs:429`; Claude `--resume`, `backend.rs:206-207`; Pi's session
   file in the conversation folder, `backend.rs:127-129`; OpenCode `session/resume` or
   `session/load`, `acp.rs:173-177`, which already refuses when neither is offered). The generation
   counter lives in `state.json` (`Conversation.generation`) and only grows, so a retired
   generation is refused forever, including after a service restart.

   The next message is never lost on this path. `begin_next` has already moved it to
   `chat.active` and `Engine::begin` journaled `turnStarted` when `start()` waits for the old
   process. If that process does not confirm its exit within the existing 10 s
   (`service.rs:1566-1583`), or the new process fails to start, the service journals
   `turnStartFailed {messageId, retiredGeneration, reason, delivered: false}`, keeps the message as
   `chat.active` in `state.json`, and sets `RecoveryRequired` as `start()` errors do today
   (`service.rs:1696-1704`). The message was never written to any backend, so recovery review
   shows it as "not delivered" next to the old process's `alive`/`unknown` state. After a
   confirmed terminate or a fork, the panel puts its text back in the composer as a draft for the
   user to send; nothing re-sends it automatically.

   Backends that cannot resume: a new generation without the old session would silently start a
   history-less agent, so it is not done. Each `ready` records `resumable` (Codex, Claude and Pi
   always resume a session id or session file; OpenCode only when `initialize` offers
   `session/resume` or `loadSession`, `acp.rs:173-177`). When a turn on a non-resumable process
   ends in a way that retires it, the service journals `generationRetired {resumable: false}`,
   pauses the queue, and refuses further messages for this conversation with "This agent cannot
   continue after an interrupted request. Start a new conversation." The turn card shows that text
   and a "New conversation from this page" button, which creates an ordinary conversation (no
   fork context). Queued messages stay visible and can be copied out; none is sent.
3. For a turn that completed cleanly the same generation stays live, and this is safe: Begin
   runs inside the MCP tool call before the CLI, so the backend cannot report the turn completed
   until the tool call, and therefore its Begin, has returned. The open-tool guard in rule 2
   covers a backend that breaks this. The known way to break it is the backend's own MCP client
   timeout: if the client gives up on a tool call, the turn can finish while the MCP server is
   still working. The service writes that timeout to at least 30 s for the chrome_use server
   (Codex `tool_timeout_sec` in the server entry it injects, `backend.rs:426`; Claude
   `MCP_TOOL_TIMEOUT` in the spawn environment), which is longer than the Begin window in rule 4.
   Step 1 checks this for every backend with a fixture whose MCP call outlives the client; any
   backend where the timeout cannot be set, or the fixture shows a completed turn with a call
   still running, retires its generation after every turn instead.

   Binding condition (plan review seq 26): reusing a generation after a clean completion is
   allowed only for a backend CLI version that has committed protocol fixtures showing both the
   clean completion lifecycle and the client-timeout behaviour. The service reads the version the
   backend reports in its handshake (Codex `initialize` `userAgent`, Claude `system/init`
   `claude_code_version`, OpenCode `initialize` `agentInfo.version`; all three seen in the step 1
   fixtures; Pi's field is settled in step 1) and checks it against a compiled allowlist of
   fixture-backed versions. An unknown version, a missing version field, or unknown timeout
   semantics retire the generation after every turn. This is the one place the code looks at a
   version, and unknown always takes the conservative path. A single happy-path run does not
   add a version to the allowlist.
4. Begin carries `{bootId, notAfter}`. `notAfter` is the MCP's send time plus its 5 s client
   budget, read from a machine-wide monotonic clock (`CLOCK_MONOTONIC` on Unix,
   `QueryUnbiasedInterruptTime` on Windows) so a wall-clock change cannot move it. Monotonic
   values only mean something within one boot, so `bootId` names the boot (Linux
   `/proc/sys/kernel/random/boot_id`, macOS `kern.bootsessionuuid`, Windows the boot time from
   `NtQuerySystemInformation(SystemTimeOfDayInformation)`). The service refuses a Begin whose
   `bootId` differs from its own, whose `notAfter` has passed, or whose `notAfter` lies more than
   5 s plus 250 ms skew in the future, so a client cannot hand in an arbitrarily distant
   deadline. The MCP never runs a tool whose Begin reply it did not receive by `notAfter`. A Begin
   the MCP gave up on can therefore never be admitted later. Monotonic values are used only for
   this deadline: they are never stored as identity, never compared across boots, and never used
   as exit proof (5.3 uses kernel process start times and session ids for that).

Admission then requires: the job is live with exactly the caller's generation and not retiring,
the phase admits leases, `chat.active` is `Some`, and `notAfter` has not passed. With a live,
non-retired generation, the active message is the turn the calling tool belongs to. A lease that
was admitted and never reported is closed at its turn's end as `unreported` with
`executed: "unknown"`: the service cannot tell a tool that ran and lost its report from a reply
that reached the MCP after `notAfter`, so it does not call it "not run".

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
                    afterStop: bool, afterTurnEnd: bool, afterRecoveryRequired: bool,
                    lateReported: bool}
                    Target = {chromeTabId|null, tabRef, targetId, url, title|null,
                              provenance: "actionReply"|"daemonState"}
actionObserved     {v, messageId, actionId, verdict: "confirmed"|"failed"|"uncertain",
                    evaluator: "browser"|"host"|"none", basis: "expect"|"readBack"|"navigation"|"tabVerified"|
                    "download"|"capture"|"toolError"|"pageChanged"|"none", detail, possiblyExecuted: bool,
                    check: null|{kind, predicate, target: Target, observedSequence}, lateReported: bool}
unboundActionReport{v, generation|null, actionId|null, tool, reason: "noLease"|"otherConversation"|
                    "malformed"}                                    (quarantine, never cited)
fileDelivered      {v, messageId, actionId, artifactId, requestedDestination, state: "delivered"|
                    "failed"|"notAttempted", digest|null, detail}   (the caller's own save path)
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
turnStartFailed    {v, messageId, retiredGeneration, reason, delivered: false}
generationRetired  {v, generation, reason: "notCompleted"|"openTool"|"openLease"|"unlistedVersion",
                    resumable: bool}
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
  (UUIDv4) and call `agentActionBegin` (3.3) with the generation from its own environment and
  `notAfter` (D1a). On refusal, or no reply by `notAfter`, return a tool error and do not run
  the CLI. On admission, record any output `path` argument of `download`, `download_url`,
  `screenshot` as `requestedDestination`, run the CLI with the lease's allocated capture path
  instead (section 6.2), deliver to `requestedDestination` afterwards as its own step, then pass the
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
  - `download`, `download_url`, `screenshot`: the verdict is computed by the service after the
    artifact outcome (3.3, lock 2): `confirmed` / `download` or `capture` only when the artifact
    is `Ready`, otherwise `failed`. This confirms that a file was captured, nothing about its
    content, and nothing about delivery to the requested path (6.2a).
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
- Tool result: the actionId lives in `structuredContent.chromeUseAction = {actionId, notes}`;
  a text block `chrome-use action <actionId>` is also prepended as the first content block.
  `structuredContent` is the carrier that every backend keeps, and the only one Claude keeps:
  the fixtures show Claude 2.1.300 drops every text block of an MCP result that has
  `structuredContent` and gives the model only `JSON.stringify(structuredContent)`
  (`fixtures/README.md`, "actionId in the tool result"). chrome-use already returns
  `structuredContent = {exitCode, stdout, stderr, response}` for every CLI-backed tool
  (`tool_result_from_run`, `mcp.rs:2057-2077`), so today Claude reads the page through
  `stdout`/`response` in that JSON, not through the pretty text block, and that stays as it is.
  Decision: keep `structuredContent` and mirror into it everything this plan adds for the model.
  Every note the MCP appends ("browser evidence was not recorded", "the requested location was
  not written", the delivered path, a refused lease's reason) goes into
  `chromeUseAction.notes` as well as the text block. Dropping `structuredContent` for Claude
  was rejected: the MCP does not know which backend called it, and screenshot inlining
  (`attach_screenshot_image`, `mcp.rs:1222-1235`) and existing MCP clients read it. Step 1
  re-runs the Claude fixture with this exact result shape and checks that the tool result the
  model sees contains `stdout`, `chromeUseAction.actionId` and the notes.
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
`Lease {messageId, generation, admittedSequence, afterStop: bool,
state: Open|Reporting|Reported(reply)|Unreported|LateReported(reply), outputs, quotaReserved}`.
Leases are rebuilt at load from `actionAdmitted` and the report events in the same pass that
computes `last_sequence` (`conversation.rs:692-728`). At most 256 open leases per conversation;
beyond that new leases are refused.

The lease state machine. Every transition happens under the engine lock and is recoverable from
the journal; no other transition exists.

| from | event | to | journaled | reply to this report |
|---|---|---|---|---|
| (none) | Begin admitted | `Open` | `actionAdmitted` | n/a |
| `Open` | report, lock 1 | `Reporting` | `browserActionTarget` | (after lock 2) |
| `Reporting` | report, lock 2 | `Reported(reply)` | artifact terminal events, then `actionObserved` | `reply` |
| `Reporting` | load after a crash | `Reported(reply)` | missing artifact terminals (`interrupted`, `load`), then `actionObserved` | n/a |
| `Reporting` | duplicate report | `Reporting` | nothing | `{state: "pending"}` |
| `Reporting` | `turnEnded` | `Reporting` | nothing (lock 2 completes it with `afterTurnEnd`) | n/a |
| `Open` | `turnEnded` | `Unreported` | `turnEnded.openLeases`, artifact terminals (`interrupted`, `turnEnd`) | n/a |
| `Unreported` | first late report | `LateReported(reply)` | `browserActionTarget` and `actionObserved`, both with `lateReported: true` and `afterTurnEnd: true`, under the original `messageId`; no ingest, no artifact event | `reply` (artifact states as recorded at `turnEnded`) |
| `Reported(reply)` / `LateReported(reply)` | any later report | unchanged | nothing | the stored `reply` |

So a lease's first report is recorded once; a late report is recorded once and always stays in
the original turn; artifact terminal states are written once and never changed; repeated
reports, late or not, add nothing. At load, `LateReported` is recognised by `lateReported: true`
on its `actionObserved`, and the stored reply is rebuilt from those events.

- `agentActionBegin {callerConversation, generation, bootId, notAfter, actionId, tool,
  arguments}` (5 s). Under the engine lock, in this order: `bootId` is this boot's and
  `notAfter` is neither passed nor beyond the cap (D1a rule 4); the conversation exists;
  the job is live with exactly this `generation` and not retiring (D1a); `chat.active` is
  `Some`; the phase is `Starting`, `Running` or `WaitingInput`; `actionId` is a UUID not seen in
  this conversation. Otherwise refused with a reason the MCP shows the agent. On admission:
  allocate outputs and reserve quota in memory (6.1, 6.2), journal `actionAdmitted` with
  `messageId = chat.active.id` and `quotaReserved`, reply `{messageId, outputs: [{artifactId,
  path}]}`.
- `agentActionReport {callerConversation, generation, actionId, tool, arguments, outcome,
  errorCode, actionContext, verdict, check, resultDigest, durationMs, artifacts: [{artifactId,
  path}], delivery}` (60 s). The report's `generation` is recorded but never used for
  attribution; the lease decides.
  - Lock 1: look up the lease in this conversation. None → journal `unboundActionReport`
    (`noLease`, or `otherConversation` when the id is leased elsewhere), reply `{bound: false}`.
    `Reported` or `LateReported` → reply the stored answer unchanged (idempotent retry; recorded
    terminal states are returned, never recomputed or dropped). `Unreported` → the first late
    report, per the table above. `Reporting` → reply
    `{state: "pending"}` without touching anything; the MCP polls with the same `actionId` until
    its budget ends, so a concurrent duplicate never starts a second ingest. `Open` → mark it
    `Reporting` and journal `browserActionTarget` (target and outcome, no verdict) with the
    lease's `messageId` and `generation`. Flags: `afterStop` when the lease was admitted before a
    stop that has since been requested, `afterTurnEnd` when the lease's turn has a `turnEnded`,
    `afterRecoveryRequired` when the conversation is `RecoveryRequired`. A late action from an
    unconfirmed process is evidence recovery needs; it is kept, under its own turn.
  - Off lock: ingest each reported artifact whose path is the lease's allocated output (6.2).
  - Lock 2: journal each artifact's terminal event under the lease's `messageId`, settle quota,
    then journal `actionObserved`. For capture tools its verdict is computed here, after the
    artifact outcome: `confirmed` only if every artifact is `Ready`; otherwise `failed` with the
    artifact reason. The MCP's own verdict for a capture tool is advisory and never journaled
    first. Set the lease to `Reported(reply)`, reply `{bound: true, verdict, artifacts:
    [{artifactId, state}]}`.
- Crash points. `actionObserved` is the last event of a report, so the journal says how far a
  report got:
  - Before `actionAdmitted` is written: nothing was admitted; the MCP had no reply and did not
    run the tool. The in-memory reservation dies with the process.
  - After `actionAdmitted`, before the reply reached the MCP: the lease reloads `Open`; the tool
    may or may not have run.
  - After `browserActionTarget`, before `actionObserved`: the lease reloads `Reporting`. On the
    first engine access after load the service journals, for every allocated artifact without
    a terminal event, `artifactUnavailable {reason: "interrupted", detectedBy: "load"}`, then
    `actionObserved {verdict: "uncertain", basis: "none", detail: "report interrupted"}` (for
    capture tools `failed`), and sets the lease `Reported`. A retry gets those states.
  - Leftover `artifacts/incoming/*.part` files are removed at load.
  - Quota at load is recomputed from the journal: `quotaReserved` of every lease without
    terminal artifact events, plus the size of every `artifactReady`. A reservation is released
    when its artifact ends unavailable and settled to the real size at `Ready`.
- At `turnEnded`, every lease of that turn still `Open` becomes `Unreported` (journaled in
  `turnEnded.openLeases`, `executed: "unknown"`), its reservations are released, and allocated
  artifacts get `artifactUnavailable {reason: "interrupted", detectedBy: "turnEnd"}`. A later
  report for that lease follows the `Unreported` row of the state machine.
- `agentTaskAssess {callerConversation, generation, bootId, notAfter, ...}` (10 s) and
  `agentRecordingAuthorize {callerConversation, generation, bootId, notAfter, ...}` (20 s): sections 4.3
  and 6.4. Same admission rules as Begin: generation from the MCP's environment, live and not
  retiring, `notAfter` not passed, phase admits leases.

### 3.4 Backend adapters (link tool events to actions)

In each adapter, before `short()`: extract `toolCallId` and any `chrome-use action <uuid>`
markers / `chromeUseAction.actionId` from the raw item, strip `image` blocks' `data`
(keep `{type:"image", mimeType, bytes}`), then emit
`notice("tool", {completed, toolCallId, actionIds, item: short(item)})`. Extraction happens
before redaction, so a known secret that happens to be a fragment of a UUID cannot break a link.

Where each backend keeps the id (all observed in the step 1 fixtures):
- Codex, `backend.rs:407-408`: `item.id`; `item.result.structuredContent.chromeUseAction`, with
  the text marker in `item.result.content[0]` as a second source.
- Claude, `backend.rs:471-472`: `tool_use.id` / `tool_result.tool_use_id`;
  `tool_use_result.structuredContent.chromeUseAction` (a top-level field of the `user` line).
  The text marker is not there whenever `structuredContent` is present, which for chrome-use is
  always.
- Pi, `backend.rs:666-671`: `toolCallId`; `result.structuredContent.structuredContent.chromeUseAction`
  (Pi nests the whole MCP result), with `result.content[0]` as a second source.
- OpenCode ACP, `acp.rs:132-149`: OpenCode 2.0.25 does not expose MCP tools directly. The model
  calls one codemode tool, `execute`, whose script calls `tools.chrome_use.*`; the ACP
  `toolCallId` is the wrapper's, one wrapper may make several MCP calls,
  `rawOutput.metadata.toolCalls` lists `{tool, status}` per inner call with no ids and no
  results, and the ACP `content` is whatever the script returned. So for OpenCode a step is the
  wrapper, linked to `[actionId]`: the ids found in its `content` (as `chromeUseAction.actionId`
  in returned structured JSON, or as text markers), at most as many as `metadata.toolCalls` lists
  chrome_use calls. When the script did not return them, the wrapper stays unlinked. An inner
  failure comes back as the wrapper's `status: completed` with `metadata.error: true`, so step
  verdicts for OpenCode come only from the evidence reports, never from the wrapper's status.

Only chrome_use items are scanned, so a shell transcript that happens to print a UUID cannot
claim an action: for Codex, Claude and Pi the tool name must match
`^(?:mcp__chrome_use__)?chrome_use[_.][a-z_]+$` (mirroring `browserTool` in `panel-state.js`);
for OpenCode, an `execute` wrapper whose `metadata.toolCalls` names a `chrome_use.*` tool. An id
links only when a lease with that id exists in this conversation; an `actionId` claimed by two
tool items keeps the first link and the second is recorded as `duplicateClaim`. Linking only
attaches a step to its evidence for display. Which turn the evidence belongs to is decided by
the lease (D1), so an unlinked wrapper's actions still appear under the right turn.

Also in the adapters:
- Codex `agentMessage`: pass `item.phase` through as `role` when the CLI emits it
  (`final_answer` → `final`, `commentary` → `commentary`); otherwise no role.
- Claude `result` (`backend.rs:474`): when `result` is a string, emit
  `notice("textRole", {itemId: claude_message, role: "final", source: "claude.result"})`.
- Compaction, from what the fixtures showed: Codex `contextCompaction` item (`thread/compacted`
  was not observed and is not relied on); Claude `system/compact_boundary` with
  `compact_metadata.trigger`; Pi `compaction_end` with `reason` when not `aborted` and without
  `errorMessage`; OpenCode `session_info_update` with
  `_meta["opencode/compaction"].status == "completed"` → `notice("contextCompacted", {...})`.
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
- On `turnEnded`, in order: `batchInterrupted` if any open tool (4.4), open leases closed as
  `Unreported` (3.3), the final `taskAssessment` (4.3), `turnSummary`, the recording stop
  (6.4), and generation retirement (D1a) when the outcome is not `completed` or a chrome_use
  tool is still open. Retirement happens in the same lock as `turnEnded`, so no Begin can be
  admitted on that generation afterwards.
- A lease of an ended turn still accepts its late report, which keeps its own turn's `messageId`
  and carries `afterTurnEnd: true`. They are shown under that turn as "reported after the
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
The service binds it to the caller's active message under the D1a rules (generation from the
MCP's environment, live and not retiring, `notAfter` not passed, a phase that admits leases;
refused otherwise) and journals `taskAssessment {phase:
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

"Open chrome_use tool" for generation retirement (D1a) means: an open item whose name matches
the chrome_use pattern (Codex, Claude, Pi), or any open OpenCode `execute` wrapper, because
which MCP tools a wrapper calls is only known when it completes. Leases are counted too: a turn
that completes with a lease still `Open` retires its generation, whatever the backend reported.

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
  depends on the kernel not handing out a PID that is still in use as a pgid or sid. The proof
  is the kernel's PID allocator, cited by source for the OS versions we support: Linux
  `alloc_pid` keeps a `struct pid` alive while any task references it as pgid or sid; for macOS
  step 1 reads the XNU `forkproc` PID allocation for the shipped versions and records whether it
  skips live pgrp and session ids. A test that holds a group and cycles PIDs is run on both as
  an observation only; it can contradict the source reading but never replaces it. Without a
  source-backed guarantee for the running OS, the probe answers `unknown` for any live group. Proven → `alive`; members in another session, an
  empty enumeration racing a live `kill(-pgid, 0)`, or any read failure → `unknown`.
- The same rule applies after a service restart: the recorded `serviceSid` is the old
  service's PID, and members still carrying it prove the old session.

### 5.4 Operations

- `conversationRecoveryReview {conversationId}` (15 s, read-only, journals nothing): phase,
  last error, the active message, process `exited|alive|unknown` with the reason, the turn's
  admitted and reported actions with verdicts and `possiblyExecuted`, unresolved tools,
  artifacts, last targets with their provenance. It does not touch the browser.
- `conversationRecoveryTerminate {conversationId}` (15 s): only in `RecoveryRequired` with probe
  `alive`. Windows: `TerminateJobObject` on the held job, then `confirm_empty`. Unix, repeated
  until the group is empty or 5 s pass: re-enumerate the group and re-prove ownership; on Linux
  `pidfd_open` each member, re-check its pgid and sid through `/proc` after opening, then
  `pidfd_send_signal(SIGKILL)`; on macOS `kill(-pgid, SIGKILL)` right after the fresh proof. A
  member forked between enumeration and signal is caught by the next round. One enumeration is
  never taken as the whole tree: `confirmed` needs `kill(-pgid, 0)` to return `ESRCH` after the
  last round. If any round finds a member outside the recorded session, or the proof fails,
  the loop stops and the probe becomes `unknown`. Journal `recoveryTerminated {method,
  confirmed}`. Never signals a group whose probe is `unknown`.
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

### 6.2a Delivery to the caller's path (a second, separate effect)

The capture path replaces the caller's `path` only for the capture. Saving to the place the
agent asked for is a second effect with its own record:
- Begin records `requestedDestination` (the agent's `path`, or none) next to the allocated
  `effectiveCapturePath`; both appear in `actionAdmitted` (the destination redacted per 7.2).
- After the report returns `Ready`, the MCP, which runs with the same rights the CLI would have
  used for that path, copies the capture to `requestedDestination` with today's destination
  rules (`prepare_download_destination`, `actions.rs:14265`): write a temporary file in the
  destination folder, fsync, rename over the target, read the result back and hash it.
- The MCP reports that with `agentDeliveryReport {actionId, state, digest}` (5 s, idempotent per
  `actionId`), which journals `fileDelivered` under the lease's turn. `delivered` requires the
  read-back digest to equal the artifact digest; anything else is `failed` with the reason.
  No `requestedDestination` → `notAttempted` and no event.
- The tool result says "saved to <path>" only for `delivered`. Otherwise it says the requested
  location was not written and gives the artifact id and the capture path, so the agent never
  believes a file is where it asked.
- The turn card shows "File captured" (artifact) and "Saved to your path" (delivery) as two
  lines. The `downloadType` check (4.3) is about the captured artifact only; a failed delivery
  never turns into a passed check, and a delivered file is never inferred from a `Ready`
  artifact.

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
- `chrome_use_record start` → `agentRecordingAuthorize` refuses under the D1a rules (generation
  from the MCP's environment, live and not retiring, `notAfter` not passed) and unless the
  active message has `record`; otherwise allocates `<workspace>/.chrome-use-out/<artifactId>/recording.webm`,
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
  (`sidepanel.js:466`, stored at `508`); or (b) the retried message is itself canonical,
  meaning `project(m) == m` (it carries no value the rules would mask, its masked values are
  already in marker form), and its projection hash equals the stored projection of that id.
  Rule (b) exists for a client that only holds the redacted form. A message that still carries
  a raw sensitive value is matched by (a) only: two different raw tokens with the same
  projection are different payloads and get the existing "reused for different text" error. A
  message with nothing sensitive is its own projection, so (a) and (b) agree and today's
  rejection of changed text is unchanged. For (b) the service keeps a lazily built map
  `messageId → hash(projection)`, filled from the journal's `user` events with
  `redact::legacy` applied, so messages accepted by v1.5.214 (whose `user` events hold the raw
  URL) match too. Neither matches → the existing error.
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
  `requestRevision` (engine-wide counter), `sentProjection`: the exact redacted projection
  journaled in that revision's `request` event, frozen at that moment, and the existing `sent`
  boolean, which still means only "a reply was dispatched". The `request` event carries
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
  one revision. `can_use_tool`: `updatedInput` must equal `sentProjection.input`, then the
  original is substituted. AskUserQuestion: `updatedInput` without `answers` must equal
  `sentProjection.input` without `answers`, and `answers` is validated as above, then built from the
  original. Codex `item/permissions/requestApproval`: `permissions` must equal
  `sentProjection.permissions`, then the original is used. The comparison is against the frozen projection, never
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
shows "unknown" whenever an event lacks the field. The one exception is D1a's binding condition:
reusing a generation after a clean completion requires a backend version on the fixture-backed
allowlist, and any other version retires its generation after every turn. The committed fixtures
(`cli/src/agent_panel/fixtures/`, including the synthetic stub-MCP runs) stay in the repository
as regression inputs for the adapter tests in 14.1.

| capability | Codex app-server | Claude Code stream-json | Pi JSONL RPC | OpenCode ACP |
|---|---|---|---|---|
| Turn start/end per message | host | host | host | host |
| Backend turn id | `turn/started` id | none (null) | none (null) | prompt request id |
| Terminal outcome | `turn/completed.status` | `result.subtype`/`is_error` | `agent_settled` + the last assistant `stopReason` (a provider failure is three `stopReason: "error"` retries, then `agent_settled {aborted: false}`) | `stopReason` |
| Tool call id | `item.id` | `tool_use.id` / `tool_use_id` | `toolCallId` | `toolCallId` of the codemode `execute` wrapper, which may hold several MCP calls |
| Tool error flag | `status`/`error` | `is_error` | `isError` | wrapper `status: failed`; an inner MCP failure shows as `status: completed` with `metadata.error: true` |
| actionId survives in tool result | observed: `item.result.content[0]` marker and `item.result.structuredContent` both kept | observed: marker text **dropped** when `structuredContent` is present (`tool_result.content` becomes its JSON; `tool_use_result.structuredContent` kept); marker kept in `content[0]` without it | observed: `result.content[0]` kept; whole MCP result under `result.structuredContent` | observed: model-dependent; MCP runs inside codemode `execute`, ACP `content` is the script's return value (`structuredContent` JSON when present), `rawOutput` has no result |
| Browser target and effect | host (MCP side channel) | host | host | host |
| Artifacts | host | host | host | host |
| Final vs commentary text | `agentMessage.phase` observed (`commentary`, `final_answer`); else unknown | `result.result` → final | unknown | unknown |
| Context compaction signal | `contextCompaction` item observed (`thread/compacted` not observed) | `system/compact_boundary` observed (`compact_metadata.trigger`) | `compaction_start`/`compaction_end` with `reason` observed (`auto_compaction_end` not observed) | `session_info_update` `_meta["opencode/compaction"]` `started`/`completed` observed |
| Approval requests | yes | yes (`can_use_tool`) | none (Pi never asks) | yes |
| Approval resolution confirm | `serverRequest/resolved` | matching `tool_use_id` result | n/a | tool completion / prompt end |
| Interrupt confirmation | `turn/completed interrupted` | `result` while stopping | `agent_settled aborted` | `stopReason cancelled` |
| Task assessment | `chrome_use_task` (agent compliance) | same | same | same |
| Recording | host via `chrome_use_record` | same | same | same |
| Exit confirmation | host process group / job | host | host | host |

If the actionId does not survive for a backend, its browser evidence still appears in the right
turn, because the action's lease (D1) names the message, as "browser record not matched to a
step", and steps show "no browser record". For OpenCode a step is the `execute` wrapper and may
link to several actions (3.4). No adjacency matching.

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

0. **Prerequisite: Codex full-access browser calls.** The fixtures found that a full-access
   Codex conversation sends `approvalPolicy: "never"` but takes the sandbox from
   `effective_access`, so with `toolAccess` set to `browserOnly` or `readFiles` Codex refuses
   every chrome_use call ("MCP tool call requires approval, but approval policy is never";
   `fixtures/README.md`, "Other findings"). Landed in `ea4dd857`: the `chrome_use` server entry
   gets `default_tools_approval_mode: "approve"` under full access (`codex_browser_server` in
   `backend.rs`), without widening shell or file access, with fixtures
   `codex-browser-only-full-access{,-unfixed}.jsonl`. The `tool_timeout_sec` from D1a rule 3 is
   added to that same server entry.

1. **Protocol fixture capture on the build host** (`leo@192.168.0.190`): run each installed
   backend once with a chrome_use MCP that returns a marker block; save raw protocol lines as
   test fixtures under `cli/src/agent_panel/fixtures/`. Settles the *verify* cells of
   section 11 before anything depends on them. Also capture the v1.5.214 compatibility fixtures
   (section 10), record the kernel PID-allocation source reading and run the PID-cycling
   observation for 5.3 (macOS reading recorded: XNU `xnu-12377.121.6` `bsd/kern/kern_fork.c:964-974`
   `forkproc` skips a candidate PID that is a live process, process group or session; groups
   and sessions leave their hashes only with their last reference, `kern_proc.c:3107-3118`,
   `2849-2873`; `setpgid` is same-session only, `kern_prot.c:595`, `626`; same loop in
   `xnu-11417.140.69` and `xnu-10063.141.1`. `ProofPolicy` is on for Linux and macOS; see
   `agent_panel/recovery.rs`), and for each backend: set the MCP client timeout as in D1a rule 3 and run
   a fixture whose chrome_use call outlives the client's timeout, to see whether the turn can
   complete with the call still running (if so, that backend retires after every turn).
2. **Redaction gate**: `redact.rs`, `Store::event(Redacted)`, type-preserving masks, message
   projection after fingerprint, legacy read redaction, request revisions and reply
   construction, stop-time approval rules. Everything after this writes through it. The
   existing `Store::event` tests (`conversation.rs:811-1057`) move to a test-only `Redacted`
   constructor.
3. **Turn lifecycle**: Pi's outcome follows the last assistant message: `failed` only when the
   last assistant `message_end` before `agent_settled` has `stopReason: "error"`. The fixtures
   show a provider failure as three `stopReason: "error"` retries and then
   `agent_settled {aborted: false}`, which stays `failed`. Today `failure` is set by any error
   `message_end` and is never cleared by a later successful one (`backend.rs:661-664`, `684-687`),
   so a successful auto-retry would end `failed`; this step clears it on a non-error assistant
   `message_end`. `Engine::begin`, `turnStarted`, `turnEnded` from the message snapshot,
   generation retirement and the `start()` change for retiring jobs, `toolCallId`, open tools, `batchInterrupted`, `emptyAnswer`, `continuation`, `control` in
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
  v1.5.214. Four cells, each with a token URL and an attachment: (1) accepted by the v1.5.214
  fixture, reply lost, raw retry after the upgrade → duplicate; (2) accepted by this build, raw
  retry identical → duplicate; (3) retry that is the canonical projection → duplicate; (4) retry
  with a different raw token whose projection equals the stored one → refused as different
  payload. Also: a changed text under the same id is still refused; a message with nothing
  sensitive and changed text is refused by the raw hash;
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
- Leases: begin refused in `Stopping`, `RecoveryRequired`, `Idle`, for a retired or stale
  generation and after `notAfter`; late Begin: A's Begin is held before it reaches the engine,
  the user stops A, A ends `interrupted`, its generation retires and B starts on a new process;
  releasing A's Begin is refused (by retirement, and again by `notAfter` in a variant where the
  generation check is bypassed), the CLI for A's tool never runs, and nothing is journaled
  under B; late report (a separate test): turn A completes cleanly, B runs on the same
  generation, an action leased in A reports during B and is journaled under A with
  `afterTurnEnd`; a turn that completes with a chrome_use tool still open retires its
  generation and the next message gets a new process; `agentTaskAssess` and
  `agentRecordingAuthorize` from a retired generation or past `notAfter` are refused; a report
  with no lease goes to `unboundActionReport` and never appears in a turn, summary or
  assessment; two concurrent reports for one lease: the second gets `pending` and only one
  ingest runs; a capture whose artifact ends unavailable is journaled `failed`, never
  `confirmed`; crash injection after `actionAdmitted`, after `browserActionTarget` and after one
  of two artifact events reloads to the states in 3.3 and quota recomputed from the journal
  equals the sum of live reservations and ready sizes; an `Open` lease at `turnEnded` becomes
  `Unreported` with `executed: "unknown"`; an artifact copy
  that spans A's `turnEnded` and B's `turnStarted` lands under A; a report whose reply was lost
  and is retried returns the first answer, including an artifact's `Ready` or `Unavailable`
  state; a lease left `Reporting` across a simulated crash reloads with `interrupted` artifacts
  and a retry gets those states; `turnEnded` carries the active message id captured before
  `terminal()` cleared it. State machine: every row of the 3.3 table, and every other
  (state, event) pair rejected; an `Unreported` lease's first late report journals one
  `browserActionTarget` and one `actionObserved` with `lateReported`, under the original turn,
  with no artifact event; a second and third late report journal nothing and return the stored
  reply; reload after a late report rebuilds `LateReported` and the same reply.
- Deadlines: a Begin with another `bootId`, a passed `notAfter`, or a `notAfter` more than 5.25 s
  ahead is refused, for Begin, assess and recording authorize; no monotonic value appears in the
  journal or `state.json`.
- Retirement and resume: the old process ignores `Shutdown` past 10 s (fault switch): the
  dequeued message stays `chat.active`, `turnStartFailed {delivered: false}` is journaled, the
  conversation is `RecoveryRequired`, no backend received the message, and after a confirmed
  terminate the panel offers its text as a draft; a non-resumable OpenCode fixture (`initialize`
  without resume or `loadSession`) after an interrupted turn journals `generationRetired
  {resumable: false}`, refuses the next message with the documented text and starts no process;
  a backend version not on the allowlist retires after a clean completed turn; an allowlisted
  version keeps its generation.
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
  ignore markers in non chrome_use items; Claude: the id comes from
  `tool_use_result.structuredContent` when the text marker is absent, and the tool result the
  model sees carries `stdout` and `chromeUseAction.notes`; Pi: the nested
  `result.structuredContent.structuredContent`; OpenCode: one `execute` wrapper with two
  chrome_use calls links both ids, a wrapper whose script returned no ids stays unlinked while
  its leased evidence still shows under the turn, an id with no lease in this conversation is not
  linked, and `status: completed` with `metadata.error: true` never reads as a successful step;
  Pi: three errored retries then `agent_settled {aborted: false}` → `failed`, an errored attempt
  followed by a successful one → `completed`; image data stripped before `short()`.
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
  recording refused without `record`; encoder missing; encoder failed. Delivery: a download with
  a requested path is delivered and the read-back digest matches → `fileDelivered: delivered`
  and the tool result names the path; an unwritable destination → `failed`, the tool result
  says the requested location was not written, and the artifact stays `Ready`; a destination
  changed after the rename → `failed`; no requested path → no `fileDelivered`.
- Recovery: Windows held job: probe counts processes, terminate empties it, after a simulated
  restart the probe is `unknown`. Unix: group gone → `exited`; leader dead, group alive with
  members in the service session → `alive`, terminate confirms; a child forked by a member during
  terminate is caught by a later round and `confirmed` waits for `ESRCH`; a member in another
  session → `unknown`, no signal sent; with the OS proof flag off (no source-backed guarantee),
  every live group is `unknown`; leader alive with mismatched start time → `unknown`; fork refused
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
20. **Late Begin across Stop**: a fixture MCP shim delays turn A's `agentActionBegin` until
    after the user pressed Stop, A ended and B started. B runs on a new generation; A's Begin is
    refused; the fixture site records no request from A's tool; B's card shows nothing from A.
    Run once per installed backend. A second run with Stop & send checks the same through the
    direct-message path. Also measures the restart cost of retirement per backend (target under
    5 s from `turnEnded` to B's `ready`).

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
   pgid or sid; Linux guarantees it in `alloc_pid`; macOS needs the same from the XNU source for
   the shipped versions, and without it every live macOS group is `unknown`. A PID-cycling test
   is observation only.
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
13. **Generation retirement.** Every interrupted, failed or unknown turn, and every completed
    turn with an open chrome_use tool, restarts the backend process, so the next message pays a
    resume (seconds; measured in live check 20). A backend that cannot resume (OpenCode without
    `session/resume` or `session/load`, `acp.rs:173-177`) fails that next message with its
    existing error. A backend whose MCP client timeout cannot be held above the Begin window
    retires after every turn. The safety of keeping a generation after a clean completion rests
    on the backend not finishing a turn while a tool call is still running; step 1 tests this per
    backend.
14. **Delivery is a second effect.** Delivering to the agent's requested path is done by the MCP
    with the agent's rights and verified by read-back digest; like other reports it is
    "reported by chrome-use's browser tools".

## 16. Deliberately excluded (not deferred)

- Automatic `--observe` on every panel action: adds snapshots and settle waits to every step
  for a UI feature (D3).
- A live page preview or periodic screenshots in the panel: would be continuous capture for
  progress display.
- Judging "meets the request" by a second model: the research shows a same-model judge is not
  ground truth; the card reports the agent's checks and the evidence instead.
- Rewriting or back-filling old journals: they are evidence as recorded.
