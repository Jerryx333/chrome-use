// Drives one agent backend through one tool turn in the protocol mode the
// panel uses (see ../../backend.rs and ../../acp.rs), then one cheap
// compaction probe. Usage: node drive.mjs <codex|claude|pi|opencode> <outdir> [tag]
// Writes <outdir>/<backend>[-tag].stdout.jsonl (raw backend lines),
// .stdin.jsonl (lines sent) and .mcp.jsonl (stub traffic).
// MCP_TIMEOUT_S=<s> switches to the client-timeout run: the stub gets a
// chrome_use_wait tool that answers after STUB_MCP_SLEEP_MS (default 20000),
// the backend's chrome_use client timeout is set the way the panel sets it,
// and instead of compaction the driver waits for the late reply and then sends
// a second, tool-free turn. The driver notes each terminal event in the stub
// log, so that file's order shows whether a turn ended with a call unanswered.
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, appendFileSync, rmSync } from "node:fs";
import { createInterface } from "node:readline";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const [backend, outArg, tag = ""] = process.argv.slice(2);
const out = resolve(outArg);
const stub = join(dirname(fileURLToPath(import.meta.url)), "stub-mcp.mjs");
const name = tag ? `${backend}-${tag}` : backend;
const workspace = join(out, `${name}-workspace`);
rmSync(workspace, { recursive: true, force: true });
mkdirSync(workspace, { recursive: true });
const files = Object.fromEntries(
  ["stdout", "stdin", "mcp"].map((k) => [k, join(out, `${name}.${k}.jsonl`)]),
);
for (const f of Object.values(files)) writeFileSync(f, "");
const timeoutS = Number(process.env.MCP_TIMEOUT_S || 0);
const sleepMs = timeoutS ? Number(process.env.STUB_MCP_SLEEP_MS || 20_000) : 0;
const env = {
  STUB_MCP_LOG: files.mcp,
  STUB_MCP_SLEEP_MS: String(sleepMs),
  STUB_MCP_OUTPUT_SCHEMA: process.env.STUB_MCP_OUTPUT_SCHEMA ?? "0",
  STUB_MCP_NO_STRUCTURED: process.env.STUB_MCP_NO_STRUCTURED ?? "0",
};
const node = process.execPath;
const instructions = "You are the browser assistant in chrome-use. Use only the chrome_use MCP server.";
const PROMPT = timeoutS
  ? "First call the chrome_use_read tool, then call the chrome_use_wait tool once. If a tool call fails or " +
    "times out, do not call it again. Finally answer with one short line that starts with the word done."
  : "First write one short sentence saying what you are about to do. Then call the chrome_use_read tool, " +
  "then call the chrome_use_shot tool. Finally answer with one short line that starts with the word done.";

const childEnv = { ...process.env };
for (const k of ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"]) delete childEnv[k];
// Client timeouts, set through the same channel the panel uses (backend.rs);
// OpenCode's ACP server entry has none, so its config file key is used.
if (timeoutS && backend === "claude") childEnv.MCP_TOOL_TIMEOUT = String(timeoutS * 1000);
if (timeoutS && backend === "opencode")
  childEnv.OPENCODE_CONFIG_CONTENT ??= JSON.stringify({ experimental: { mcp_timeout: timeoutS * 1000 } });
const note = (event) => appendFileSync(files.mcp, JSON.stringify({ at: Date.now(), direction: "driver", value: event }) + "\n");
const launch = {
  codex: ["codex", ["app-server", "--listen", "stdio://"]],
  opencode: ["opencode", ["acp"]],
  pi: [
    "pi",
    ["--mode", "rpc", "--approve", "--session-dir", join(workspace, "sessions"), "--session-id",
     "fixture-session", "--append-system-prompt", instructions, "--tools", "mcp__chrome_use__*",
     ...(process.env.PI_MODEL ? ["--model", process.env.PI_MODEL] : [])],
  ],
  claude: [
    "claude",
    ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
     "--include-partial-messages", "--strict-mcp-config", "--mcp-config", join(workspace, "mcp.json"),
     "--append-system-prompt", instructions, "--permission-mode", "bypassPermissions",
     "--permission-prompts", "host", "--permission-prompt-tool", "stdio", "--tools", "AskUserQuestion"],
  ],
}[backend];
if (backend === "claude")
  writeFileSync(join(workspace, "mcp.json"), JSON.stringify({ mcpServers: { chrome_use: { type: "stdio", command: node, args: [stub], env } } }));
if (backend === "pi") {
  mkdirSync(join(workspace, ".pi"), { recursive: true });
  writeFileSync(join(workspace, ".pi", "mcp.json"), JSON.stringify({ autoEnableCodemode: false, mcpServers: { chrome_use: { command: node, args: [stub], env, exposure: "direct", timeout: timeoutS || 120 } } }));
}

// BACKEND_BIN overrides the executable (e.g. a specific OpenCode build).
const child = spawn(process.env.BACKEND_BIN || launch[0], launch[1], { cwd: workspace, env: childEnv, stdio: ["pipe", "pipe", "inherit"] });
const write = (value) => {
  appendFileSync(files.stdin, JSON.stringify(value) + "\n");
  child.stdin.write(JSON.stringify(value) + "\n");
};
let finished = false;
const finish = (why) => {
  if (finished) return;
  finished = true;
  console.error(`[drive ${backend}] ${why}`);
  if (timeoutS) note(`finish: ${why}`);
  setTimeout(() => { child.kill("SIGTERM"); process.exit(0); }, 1500);
};
setTimeout(() => finish("timeout"), 240_000 + 2 * sleepMs);
child.on("exit", (code) => finish(`backend exited ${code}`));

let phase = "init";
let nextId = 1;
const pending = new Map();
const rpc = (method, params, jsonrpc = false) => {
  const id = ++nextId;
  pending.set(id, method);
  write({ ...(jsonrpc ? { jsonrpc: "2.0" } : {}), id, method, params });
  return id;
};
let session = null;
let promptId = null;
let compactTimer = null;
const afterCompact = (ms) => { clearTimeout(compactTimer); compactTimer = setTimeout(() => finish("compaction probe done"), ms); };
const SECOND = "Reply with the single word ok. Do not call any tool.";
// Timeout run: called on each terminal event instead of starting compaction.
// Returns true when it handled the event.
const timeoutTurnEnded = (sendSecond) => {
  if (!timeoutS) return false;
  note(`turn ended (${phase})`);
  if (phase === "turn") {
    phase = "second";
    setTimeout(() => { note("second turn sent"); sendSecond(); }, sleepMs + 5000);
  } else if (phase === "second") {
    phase = "done";
    setTimeout(() => finish("timeout run done"), 3000);
  }
  return true;
};

const handlers = {
  codex(v) {
    if (v.id !== undefined && v.method) {
      // Server request (approval or MCP elicitation): accept, as a user would.
      const result = v.method === "mcpServer/elicitation/request" ? { action: "accept", content: null } : { decision: "accept" };
      write({ id: v.id, result });
      return;
    }
    if (v.id !== undefined && pending.has(v.id)) {
      const method = pending.get(v.id);
      pending.delete(v.id);
      if (method === "initialize") {
        write({ method: "initialized" });
        rpc("config/read", { includeLayers: false });
      } else if (method === "config/read") {
        // As the panel does: disable every user MCP server, add chrome_use.
        const servers = Object.fromEntries(Object.keys(v.result?.config?.mcp_servers ?? {}).map((n) => [n, { enabled: false }]));
        servers.chrome_use = { command: node, args: [stub], env, enabled: true };
        if (timeoutS) servers.chrome_use.tool_timeout_sec = timeoutS;
        // CODEX_MCP_APPROVAL sets chrome_use's default_tools_approval_mode.
        if (process.env.CODEX_MCP_APPROVAL) servers.chrome_use.default_tools_approval_mode = process.env.CODEX_MCP_APPROVAL;
        rpc("thread/start", {
          cwd: workspace, approvalPolicy: process.env.CODEX_APPROVAL ?? "on-request",
          sandbox: process.env.CODEX_SANDBOX ?? "read-only", developerInstructions: instructions,
          config: { web_search: "disabled", features: { apps: false, goals: false, multi_agent: false, image_generation: false }, mcp_servers: servers },
        });
      } else if (method === "thread/start") {
        session = v.result?.thread?.id;
        rpc("turn/start", { threadId: session, input: [{ type: "text", text: PROMPT, text_elements: [] }] });
      } else if (method === "thread/compact/start") {
        afterCompact(v.error ? 1000 : 90_000);
      }
      return;
    }
    if (v.method === "turn/completed" && timeoutTurnEnded(() => rpc("turn/start", { threadId: session, input: [{ type: "text", text: SECOND, text_elements: [] }] }))) {
      return;
    } else if (v.method === "turn/completed" && phase === "turn") {
      phase = "compact";
      rpc("thread/compact/start", { threadId: session });
    } else if (phase === "compact" && (v.method === "thread/compacted" || v.method === "turn/completed")) {
      afterCompact(3000);
    }
  },
  claude(v) {
    if (v.type === "control_request") {
      write({ type: "control_response", response: { subtype: "success", request_id: v.request_id, response: { behavior: "allow", updatedInput: v.request?.input ?? {} } } });
      return;
    }
    if (v.type === "control_response" && v.response?.request_id === "panel-initialize") {
      write({ type: "user", session_id: "", message: { role: "user", content: PROMPT }, parent_tool_use_id: null });
      return;
    }
    if (v.type === "result" && timeoutTurnEnded(() => write({ type: "user", session_id: v.session_id, message: { role: "user", content: SECOND }, parent_tool_use_id: null }))) {
      return;
    } else if (v.type === "result" && phase === "turn") {
      phase = "compact";
      write({ type: "user", session_id: v.session_id, message: { role: "user", content: "/compact" }, parent_tool_use_id: null });
    } else if (v.type === "result" && phase === "compact") {
      afterCompact(3000);
    }
  },
  pi(v) {
    if (v.type === "response" && v.id === "panel-state") {
      write({ id: "panel-turn-fixture", type: "prompt", message: PROMPT, images: [] });
    } else if (v.type === "extension_ui_request") {
      write({ type: "extension_ui_response", id: v.id, cancelled: true });
    } else if (v.type === "agent_settled" && timeoutTurnEnded(() => write({ id: "panel-turn-second", type: "prompt", message: SECOND, images: [] }))) {
      return;
    } else if (v.type === "agent_settled" && phase === "turn") {
      phase = "compact";
      write({ id: "panel-compact", type: "compact" });
    } else if (phase === "compact" && v.type === "response" && v.id === "panel-compact") {
      afterCompact(3000);
    }
  },
  opencode(v) {
    if (v.method === "session/request_permission") {
      const allow = v.params.options.find((o) => o.kind === "allow_once") ?? v.params.options[0];
      write({ jsonrpc: "2.0", id: v.id, result: { outcome: { outcome: "selected", optionId: allow.optionId } } });
      return;
    }
    if (v.id !== undefined && v.method) {
      write({ jsonrpc: "2.0", id: v.id, error: { code: -32601, message: "unsupported" } });
      return;
    }
    if (v.id !== undefined && pending.has(v.id)) {
      const method = pending.get(v.id);
      pending.delete(v.id);
      if (method === "initialize") {
        rpc("session/new", { cwd: workspace, mcpServers: [{ name: "chrome_use", command: node, args: [stub], env: Object.entries(env).map(([name, value]) => ({ name, value })) }] }, true);
      } else if (method === "session/new") {
        session = v.result.sessionId;
        promptId = rpc("session/prompt", { sessionId: session, prompt: [{ type: "text", text: `${instructions}\n\n${PROMPT}` }] }, true);
      } else if (method === "session/prompt" && timeoutTurnEnded(() => rpc("session/prompt", { sessionId: session, prompt: [{ type: "text", text: SECOND }] }, true))) {
        return;
      } else if (method === "session/prompt" && phase === "turn") {
        phase = "compact";
        rpc("session/prompt", { sessionId: session, prompt: [{ type: "text", text: "/compact" }] }, true);
      } else if (method === "session/prompt" && phase === "compact") {
        afterCompact(3000);
      }
    }
  },
};

createInterface({ input: child.stdout }).on("line", (line) => {
  appendFileSync(files.stdout, line + "\n");
  let value;
  try { value = JSON.parse(line); } catch { return; }
  handlers[backend](value);
});

phase = "turn";
if (backend === "codex") {
  pending.set(1, "initialize");
  write({ id: 1, method: "initialize", params: { clientInfo: { name: "chrome_use_panel", title: "chrome-use", version: "fixture" } } });
} else if (backend === "claude") {
  write({ type: "control_request", request_id: "panel-initialize", request: { subtype: "initialize", hooks: null } });
} else if (backend === "pi") {
  write({ id: "panel-state", type: "get_state" });
} else if (backend === "opencode") {
  pending.set(1, "initialize");
  write({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: 1, clientInfo: { name: "chrome_use_panel", version: "fixture" }, clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false } } });
}
