# Agent backend protocol fixtures

Raw protocol lines captured on 2026-10-15 for implementation step 1 of
`docs/superpowers/plans/2026-10-15-verifiable-panel-turns.md`. They settle the
*verify* cells of the plan's section 11 (backend capability matrix).

Each backend ran once, in the protocol mode the panel uses (`backend.rs`,
`acp.rs`), against a stub stdio MCP server named `chrome_use`
(`capture/stub-mcp.mjs`) with two tools:

- `chrome_use_read`: `content` = `[text "chrome-use action 11111111-2222-4333-8444-555555555555", text "Page title: ..."]`,
  `structuredContent` = `{chromeUseAction: {actionId: "11111111-..."}}`.
- `chrome_use_shot`: `content` = `[text "chrome-use action 99999999-8888-4777-8666-555555555555", image 1x1 PNG]`,
  `structuredContent` = `{chromeUseAction: {actionId: "99999999-..."}}`.

The prompt asks for one sentence first, then both tools, then a line starting
with `done` (to get commentary and final text in one turn). After the turn, the
driver sends one cheap compaction request.

## Files

| file | content |
|---|---|
| `<backend>-tool-marker.jsonl` | backend stdout, one protocol line per line |
| `<backend>-tool-marker.stdin.jsonl` | the lines the driver wrote to the backend |
| `*-no-structured.jsonl` | same run with `STUB_MCP_NO_STRUCTURED=1` (no `structuredContent`) |
| `codex-tool-marker-full-access.jsonl` | Codex with `approvalPolicy: never`, `sandbox: danger-full-access` |
| `capture/drive.mjs` | driver: launches a backend and speaks its protocol |
| `capture/stub-mcp.mjs` | the stub MCP server |
| `capture/redact.mjs` | raw capture to committed fixture |

`redact.mjs` drops user hook lines (`system/hook_*`, `hook/*`), replaces
capture-machine catalogs (Claude commands/skills/plugins/agents/models, the
Codex `config/read` reply, ACP model option lists) with a placeholder, and
redacts the account email, host name, Codex installation id and credit balance,
and home or scratch paths (`/Users/USER`, `/WORK`, `/CAPTURE`). Everything else
is what the backend wrote, re-serialized by `JSON.stringify` (same values; escapes and key
spacing may differ from the raw bytes).

## Versions and hosts

| backend | version | host | model |
|---|---|---|---|
| Codex app-server | codex-cli 0.162.1 | build host 192.168.0.190 | account default |
| OpenCode ACP | 2.0.25 (`/private/tmp/chrome-use-opencode-2.0.25`) | build host | `google/gemini-4-argon` (default) |
| Claude Code stream-json | 2.1.300 | laptop (not installed on the build host) | `claude-opus-5-5` (default) |
| Pi JSONL RPC | 1.1.0 | laptop (not installed on the build host) | `claude/claude-haiku-5-5` |

The build host has `opencode` 1.18.20 on `PATH`, but it exits with "Database
is not empty and has no session table" because the shared data directory was
migrated by 2.0.25. The capture used the 2.0.25 binary with
`XDG_DATA_HOME` set to a private directory that holds only a copy of `auth.json`.

Pi's default model (`ai-leeguoo-com/auto`) returned 503 for the final message in
two attempts, after both tools had run, so the committed Pi fixture uses
`--model claude/claude-haiku-5-5`.

## Commands

From a scratch directory (the driver writes `<backend>[-tag].{stdout,stdin,mcp}.jsonl` and a
`<backend>-workspace/` there):

```sh
node capture/drive.mjs codex .                                   # approvalPolicy on-request, sandbox read-only
CODEX_APPROVAL=never CODEX_SANDBOX=danger-full-access node capture/drive.mjs codex . fullaccess
node capture/drive.mjs claude .
STUB_MCP_NO_STRUCTURED=1 node capture/drive.mjs claude . nostructured
PI_MODEL=claude/claude-haiku-5-5 node capture/drive.mjs pi .
XDG_DATA_HOME=$PWD/xdg BACKEND_BIN=/private/tmp/chrome-use-opencode-2.0.25 node capture/drive.mjs opencode .
STUB_MCP_NO_STRUCTURED=1 XDG_DATA_HOME=$PWD/xdg BACKEND_BIN=... node capture/drive.mjs opencode . nostructured
node capture/redact.mjs <backend>.stdout.jsonl <backend>-tool-marker.jsonl
```

Launch arguments are the panel's:

- Codex: `codex app-server --listen stdio://`; `initialize`, `initialized`, `config/read`,
  `thread/start` with every user MCP server set to `enabled: false` plus `chrome_use`, `turn/start`,
  then `thread/compact/start`. MCP elicitations were answered `{action: "accept"}`.
- Claude: `claude -p --input-format stream-json --output-format stream-json --verbose
  --include-partial-messages --strict-mcp-config --mcp-config mcp.json --append-system-prompt ...
  --permission-mode bypassPermissions --permission-prompts host --permission-prompt-tool stdio
  --tools AskUserQuestion`, with `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_SSE_PORT`
  removed; `control_request initialize`, one `user` line, then a `/compact` user line.
- Pi: `pi --mode rpc --approve --session-dir <ws>/sessions --session-id fixture-session
  --append-system-prompt ... --tools mcp__chrome_use__* [--model ...]` with
  `<ws>/.pi/mcp.json` = `{autoEnableCodemode: false, mcpServers: {chrome_use: {..., exposure: "direct", timeout: 120}}}`;
  `get_state`, `prompt`, then `{type: "compact"}`.
- OpenCode: `opencode acp`; `initialize` (protocolVersion 1, no fs or terminal),
  `session/new` with `chrome_use` in `mcpServers`, `session/prompt`, then a `/compact` prompt.

## Observed answers to the *verify* cells

### actionId in the tool result

| backend | text marker (`content[0]`) | `structuredContent` | image |
|---|---|---|---|
| Codex 0.162.1 | kept: `item/completed` → `item.result.content[0].text` | kept: `item.result.structuredContent` | kept: `item.result.content[1]` (`type: image`) |
| Claude 2.1.300 | **lost when `structuredContent` is present** | `tool_use_result.structuredContent` (top-level field of the `user` line) | kept, plus an added `[Image: source: ...]` text block |
| Pi 1.1.0 | kept: `tool_execution_end.result.content[0].text` | kept, nested: `result.structuredContent` is the whole MCP result, its own `structuredContent` inside | kept: `result.content[1]` |
| OpenCode 2.0.25 | depends on code the model writes | not passed through as a field | kept as an ACP `content` image block |

Claude: when the MCP result has `structuredContent`, the `tool_result` the model
sees is only `JSON.stringify(structuredContent)`. For `chrome_use_read` it was
the string `{"chromeUseAction":{"actionId":"11111111-..."}}`. Both text blocks,
the marker and the page text, were dropped, and the model answered that the
read "returned only an action marker and no page text". For the image tool, the
image is kept and the JSON replaces the marker text block. Declaring an
`outputSchema` did not change this (separate run, not committed). Without
`structuredContent` (`claude-tool-marker-no-structured.jsonl`), `content[0].text`
is the marker and the page text reaches the model. So for Claude, a chrome-use
tool that returns `structuredContent` hides its text content from the model.
chrome-use already returns `structuredContent` today (`cli/src/mcp.rs`, e.g. the
`response.path` of screenshots), so Claude already sees only that JSON. An actionId
that must reach Claude has to be inside `structuredContent`. A text-only marker
would be lost there.

OpenCode 2.0.25 does not expose MCP tools directly. The model calls one
codemode tool, `execute` (`kind: other`), whose `rawInput.code` is JavaScript
that calls `tools.chrome_use.chrome_use_read()`. The ACP `toolCallId` belongs to
that `execute` call. One `execute` can make several MCP calls: in the
no-structured run, one call ran both tools.
`rawOutput.metadata.toolCalls` lists `{tool: "chrome_use.chrome_use_read", status}`
for each inner call, with no ids and no results. The ACP `content` is whatever
the script returns. With `structuredContent`, that was the pretty-printed
`structuredContent` JSON, so the marker text was gone but the actionId remained.
Without it, the script returned an object of strings that contained both
markers. In three of five runs, the model (gemini-4-argon) said the chrome_use
tools were "not registered" and called nothing, even though the server log shows
`mcp connected server=chrome_use tools=2`.

### Final vs commentary text

- Codex: `agentMessage.phase` is present on `item/started` and `item/completed`:
  `"commentary"` for the text before the tools, `"final_answer"` for the last message.
- Claude: no role field on `assistant` text blocks; the final text is `result.result`.
- Pi: no role field. The opening sentence shares an assistant message with the first `toolCall`.
- OpenCode: no role field. `agent_message_chunk` carries a `messageId`, and each message has its own id.

### Compaction signal (manual compaction; automatic compaction not triggered)

- Codex: `thread/compact/start` returns `{}`, then runs as a new turn: `turn/started`, an
  `item/started` + `item/completed` of `{type: "contextCompaction", id}`, then `turn/completed`
  with status `completed`. **`thread/compacted` was not observed.**
- Claude: `system` `status: "compacting"`, then `system` `status: null, compact_result: "success"`, a new
  `system/init`, then `system/compact_boundary` with
  `compact_metadata.trigger: "manual"` and pre/post token counts. The `/compact` line ends with a
  `result` (`subtype: success`, `num_turns: 0`, `result: ""`).
- Pi: `compaction_start {reason: "manual"}` and `compaction_end {reason, aborted, willRetry, errorMessage}`.
  The session was too small, so `compaction_end` carries
  `errorMessage: "Compaction failed: Nothing to compact (session too small)"` and the `compact`
  response has `success: false`. **`auto_compaction_end` was not observed.** The observed events are
  `compaction_*` with a `reason` field.
- OpenCode: `session/update` `session_info_update` with
  `_meta["opencode/compaction"] = {status: "started" | "completed", messageId, reason: "manual"}`.

### Tool call ids

- Codex: `item.id` of `mcpToolCall`, e.g. `exec-<uuid>`. `item.server` and `item.tool` are separate fields.
- Claude: `tool_use.id` (`toolu_...`) in `assistant`; `tool_use_id` in the `user` `tool_result`.
- Pi: `toolCallId` (`call_...`) on `tool_execution_start` and `tool_execution_end`. `toolName` is `mcp__chrome_use__<tool>`.
- OpenCode: `toolCallId` (`call_...`) of the codemode `execute` call, not of an MCP call.

## Other findings

- Codex with `approvalPolicy: "never"` and `sandbox: "read-only"`: both MCP calls completed with
  `status: "failed"` and `error.message: "MCP tool call requires approval, but approval policy is never"`
  (first run, not committed). With `danger-full-access` they ran without a prompt
  (`codex-tool-marker-full-access.jsonl`). With `on-request`, each call sent
  `mcpServer/elicitation/request` (`_meta.codex_approval_kind: "mcp_tool_call"`), and
  `serverRequest/resolved` followed the accept. The panel sends `never` whenever `full_access` is set,
  but takes the sandbox from `effective_access`. A full-access conversation with `toolAccess` set to
  `browserOnly` or `readFiles` would therefore have every chrome_use call refused. The stub declares no
  tool annotations; neither does the real chrome-use MCP.
- Codex still started plugin-provided MCP servers (`agentparty`) that `config/read` does not
  list, even with every listed server disabled.
- Pi printed `extension_ui_request notify` "MCP tools are only reachable from the codemode or
  tool_search tool, but neither is active", yet `exposure: "direct"` tools were called normally.
- Pi provider failure (the two 503 runs above): every retry produced `stopReason: "error"`, then
  `auto_retry_start` ×3, `auto_retry_end {success: false}`, and finally `agent_settled {aborted: false}`.
