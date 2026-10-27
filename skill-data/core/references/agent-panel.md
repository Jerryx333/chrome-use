# Agent side panel, extension settings and mobile

<!-- Moved verbatim from skill-data/core/SKILL.md when the entry point was slimmed to an everyday loop (issue #781, G7). -->

If the side panel reports "Specified native messaging host not found." in a
Chrome started with `--user-data-dir`, rerun `chrome-use extension install` on
macOS/Linux while that Chrome is running, then reopen the panel. Explicit data
directories and verification limits: [commands](commands.md) and
[troubleshooting](troubleshooting.md).

Enabling extension stealth mode or disabling its cursor also removes existing
cursor artifacts. If settings warns that cleanup is unconfirmed, reload affected
pages before relying on stealth; see [connection](connection.md).

Native agent-panel configuration planning, apply and restore are separate from
browser actions. Saved configuration proves file readback, not a model change
in an existing session. Inspect current values after an unconfirmed write;
never replay it automatically. Isolation and journal details: [connection](connection.md).

If Chrome replaces a saved Initial page tab, the conversation follows its replacement instead of opening a duplicate. The saved initial and final addresses remain separate.

Each conversation row offers **Initial page**, **Final page**, and **Delete conversation**. Initial page opens the first recorded HTTP(S) URL; Final page opens the most recently recorded URL, including while a task is running. A matching recorded tab is focused, otherwise the saved URL opens in a new tab. Opening the initial page preserves the final address. Older conversations without an initial address report it as unavailable. The title switches chats; opening a page does not send messages, rerun tasks or restore unsaved input. Addresses stay in local extension storage. Deletion requires confirmation and reports cleanup failures; stop a running reply before deleting.

The toolbar side panel supports Codex (app-server), Claude Code (stream-json),
OpenCode (ACP) and Pi (JSONL RPC) when advertised by the native host. It uses
local CLI logins and adds no page content script. After a failed or interrupted
turn, a new message starts in the same chat/model; older queued messages stay
paused until explicitly resumed. Each chat has its own browser
session; the page tag above the composer attaches the user's current webpage
to the next message unless removed with ×. Settings opens connection options.
Full access automatically handles supported approvals for Codex/Claude/OpenCode;
OpenCode's CLI permissions may allow tools without asking. Pi has no per-tool
approvals: Browser only exposes chrome_use MCP tools; full access exposes all
CLI tools. Agent questions still require user input. Model menus come from CLI
catalogs; the ＋ menu accepts a custom model ID. Catalog metadata does not prove
execution access. Panel history stays local, but the selected CLI may send tasks
and page content to its configured model service. Deletion removes the panel
history, the agent session history and the conversation's browser session;
Codex's SQLite cache is untouched. Details: [connection](connection.md).
Panel replies render fenced code literally. Use a longer outer fence for examples
containing shorter fences; a closing fence must use the same character, be at
least as long as the opener, and have only whitespace after it.
Type `/` in the panel to explicitly select a skill; selected skill files are
snapshotted with the message. Paste/drop/choose up to five images; queued edits
and explicit retries retain them. Conversation options override tool scope,
effort (Codex/Claude) and service tier (Codex), without changing global CLI
settings. Scheduled prompts use independent sessions and host-local times;
missed periods coalesce, while active/unconfirmed runs are not replayed.
Clicking a task notification activates the conversation tab and focuses the window containing that tab, even when another Chrome window was last active or the current window is not yet known. A bound conversation tab takes precedence over the last-window fallback.

Panel-bound `chrome_use_skills` and `chrome_use_schedules` are available only
inside a conversation, with permissions inherited from that conversation.
Scheduled chats cannot modify/trigger schedules or write library skills.
Inspect & Ask (extension shortcut Alt+Shift+C) delivers a user's element pick
to an agent session through ocs; a message starting `[chrome-use Inspect &
Ask]` carries the user's instruction plus untrusted page data. Read
`core/inspect-and-ask` before acting on one.
If Chrome rejects a panel turn notification, a later check in the same extension-worker session retries it. Disabled notifications and turns already being viewed remain suppressed. Viewing a turn while its notification is being created also consumes that turn if creation later fails, including across worker restarts.

Reconnect reads persisted events without resending a turn. Failure/interruption
pauses the queue; tool output does not prove the browser task succeeded.
Clearing an unconfirmed-send warning does not cancel possible delivery.

The `/resume` picker clears old choices when the agent or search changes.
Enter selects only a currently listed session; loading, unavailable or empty
results cannot open or resume an old selection. Changing agent/search or closing
the picker dismisses old confirmations and ignores earlier resume responses.
An already-issued host request may still finish; fresh results remain selectable
while it is pending.

Queued messages can be edited or removed only while still queued and matching
the original text. Edits use a new ID; superseded IDs stay deduplicated. Long
previews need a full-message read before changes. Queue operations preserve
the running turn. Stop & send or Ctrl/Cmd+Enter is a separate direct-send
operation: it waits for terminal/owned-process-exit confirmation and leaves
ordinary queued messages paused. Unconfirmed or recovered turns cannot release
a pending direct message automatically.
Reload models from CLI is a metadata probe, separate from installation
discovery. It returns CLI-reported model suggestions and account state without
identity or credentials, sends no prompt and does not verify model access.
Missing or partial catalogs are explicit; typed model IDs remain available.
A connected legacy relay does not imply panel support: the native host must
advertise schema capabilities. Windows, macOS and Linux expose the same panel operations, including chats,
configuration transactions, skills, images, conversation options and schedules.
Upgrade legacy Windows hosts that advertise readOnly. Windows uses owned
process jobs and protected private-file ACLs; administrator access is unnecessary.

`mobile serve --bind IP:PORT` waits with exponential backoff (1–60s) if the selected IP is unavailable. Waiting is not readiness. It never broadens the bind address; after a network change, update the bind and phone endpoint explicitly. Other bind errors fail immediately.
