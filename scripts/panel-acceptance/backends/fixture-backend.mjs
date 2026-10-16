// Fixture agent backends for the verifiable-panel-turns live checks.
//
// One script, four protocol personalities (Codex app-server JSON-RPC, Claude
// stream-json, Pi JSONL RPC, OpenCode ACP), launched by the panel service
// through the conversation `executable` override. No model runs. Each turn
// follows a scripted scenario named in the user message ("run scenario
// <name>"), read from $PANEL_ACCEPT_DIR/scenarios/<name>.json. Browser tools
// are called on the REAL `chrome-use mcp` server started from the config the
// host wrote (Codex thread/start config, Claude --mcp-config, Pi
// .pi/mcp.json, ACP session/new mcpServers), so evidence flows through the
// real MCP, daemon, extension relay and Chrome.
//
// Event shapes follow the committed protocol fixtures in
// cli/src/agent_panel/fixtures/*-tool-marker.jsonl.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';
import { randomUUID } from 'node:crypto';

const personality = process.argv[2];
const argv = process.argv.slice(3);
const ACCEPT = process.env.PANEL_ACCEPT_DIR;
const sleep = ms => new Promise(r => setTimeout(r, ms));
const arg = name => { const i = argv.indexOf(name); return i >= 0 ? argv[i + 1] : null; };

function log(entry) {
  if (!ACCEPT) return;
  try { fs.appendFileSync(path.join(ACCEPT, 'backends.jsonl'), JSON.stringify({ at: Date.now(), pid: process.pid, personality, ...entry }) + '\n'); } catch {}
}
function out(value) { process.stdout.write(JSON.stringify(value) + '\n'); }
log({ event: 'start', argv, cwd: process.cwd() });
process.on('uncaughtException', e => { log({ event: 'crash', error: String(e.stack || e) }); process.exit(70); });

// ---------------------------------------------------------------- MCP client
class Mcp {
  constructor(server) {
    const env = { ...process.env };
    const entries = Array.isArray(server.env) ? server.env.map(r => [r.name, r.value]) : Object.entries(server.env || {});
    for (const [k, v] of entries) env[k] = String(v);
    this.env = env;
    this.server = server;
    this.conversation = (env.AGENT_BROWSER_SESSION || '').replace(/^panel-/, '');
    this.next = 0;
    this.pending = new Map();
    this.started = null;
  }
  start(overrides = {}) {
    if (this.started) return this.started;
    const env = { ...this.env, ...overrides };
    this.proc = spawn(this.server.command, this.server.args || [], { env, stdio: ['pipe', 'pipe', 'pipe'] });
    this.proc.stderr.on('data', d => log({ event: 'mcpStderr', text: String(d).slice(0, 2000) }));
    readline.createInterface({ input: this.proc.stdout }).on('line', line => {
      let m; try { m = JSON.parse(line); } catch { return; }
      if (m.id != null && this.pending.has(m.id)) { this.pending.get(m.id)(m); this.pending.delete(m.id); }
    });
    this.proc.on('exit', code => { log({ event: 'mcpExit', code }); for (const fn of this.pending.values()) fn({ error: { message: 'mcp exited' } }); this.pending.clear(); });
    this.started = (async () => {
      await this.rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'panel-fixture', version: '0' } });
      this.proc.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
    })();
    return this.started;
  }
  rpc(method, params) {
    const id = ++this.next;
    return new Promise(resolve => {
      this.pending.set(id, resolve);
      this.proc.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
    });
  }
  async call(name, args) {
    await this.start();
    const reply = await this.rpc('tools/call', { name, arguments: args });
    if (reply.error) return { content: [{ type: 'text', text: 'MCP error: ' + reply.error.message }], isError: true };
    return reply.result;
  }
}

// ------------------------------------------------------------- scenarios
function scenarioName(text) {
  const m = /run scenario ([A-Za-z0-9_.-]+)/.exec(text || '');
  return m ? m[1] : null;
}
function loadScenario(text) {
  const name = scenarioName(text);
  if (!name || !ACCEPT) return { name: null, steps: [{ say: 'No scenario was named.' }] };
  const file = path.join(ACCEPT, 'scenarios', name + '.json');
  const s = JSON.parse(fs.readFileSync(file, 'utf8'));
  return { name, ...s };
}
const actionId = result => result?.structuredContent?.chromeUseAction?.actionId
  || /chrome-use action ([0-9a-f-]{36})/.exec(JSON.stringify(result?.content || ''))?.[1] || null;
function findKey(value, key, depth = 0) {
  if (!value || typeof value !== 'object' || depth > 10) return undefined;
  if (key in value && value[key] != null) return value[key];
  for (const child of Object.values(value)) { const f = findKey(child, key, depth + 1); if (f !== undefined) return f; }
  return undefined;
}
// "${name.actionId}", "${name.artifactId}", "${name:json.path}" resolve
// against results saved earlier in this process.
function resolve(value, saved) {
  if (typeof value === 'string') {
    value = value.replaceAll('${cwd}', process.cwd());
    return value.replace(/\$\{([A-Za-z0-9_]+)(?:\.(actionId|artifactId)|:([A-Za-z0-9_.[\]]+))\}/g, (_, n, short, p) => {
      const r = saved[n];
      if (r === undefined) return `missing-${n}`;
      if (short === 'actionId') return actionId(r) || `noaction-${n}`;
      if (short === 'artifactId') return findKey(r?.structuredContent, 'artifactId') ?? `noartifact-${n}`;
      return String(p.split(/[.[\]]/).filter(Boolean).reduce((o, k) => o?.[k], r));
    });
  }
  if (Array.isArray(value)) return value.map(v => resolve(v, saved));
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, resolve(v, saved)]));
  return value;
}
const signalPath = (conv, name) => path.join(ACCEPT, 'signals', `${conv}.${name}`);

// Runs one turn's steps through a personality's emitter.
async function runTurn(text, emit, mcp, control) {
  const scenario = loadScenario(text);
  log({ event: 'turn', scenario: scenario.name, conversation: mcp?.conversation, text: String(text || '').slice(0, 40000) });
  control.ignoreInterrupt = !!scenario.ignoreInterrupt;
  // exitOnInterrupt: the leader exits at once on Stop while children it
  // spawned still run in its process group, which the service must end.
  if (scenario.exitOnInterrupt) setInterval(() => { if (control.interrupted) { log({ event: 'exitOnInterrupt' }); process.exit(0); } }, 50);
  const saved = control.saved;
  const inflight = new Set();
  let callNo = 0;
  const runTool = async (step, approved) => {
    const callId = `${personality}-call-${process.pid}-${++callNo}`;
    const args = resolve(approved || step.args || {}, saved);
    emit.toolStart(callId, step.tool, args);
    const started = Date.now();
    const p = (async () => {
      if (step.mcpEnv) mcp.start(step.mcpEnv);
      const result = await mcp.call(step.tool, args);
      log({ event: 'tool', scenario: scenario.name, tool: step.tool, args, ms: Date.now() - started, isError: !!result?.isError, actionId: actionId(result), result: JSON.stringify(result).slice(0, 3000) });
      if (step.save) saved[step.save] = result;
      emit.toolEnd(callId, step.tool, args, result);
      return result;
    })();
    inflight.add(p);
    p.finally(() => inflight.delete(p));
    return p;
  };
  for (const raw of scenario.steps || []) {
    // A step marked afterInterrupt still runs once the user pressed Stop
    // (a request that arrives during stop, check 6).
    if (control.interrupted && !control.ignoreInterrupt && !raw.afterInterrupt) continue;
    const step = raw;
    if (step.say != null) {
      emit.text(resolve(step.say, saved), step.deltas ? resolve(step.deltas, saved) : null, step.role || null);
    } else if (step.tool) {
      const p = runTool(step);
      if (!step.async) await p;
    } else if (step.parallel) {
      await Promise.all(step.parallel.map(s => runTool(s)));
    } else if (step.wrapper) {
      // OpenCode codemode: several MCP calls inside one `execute` wrapper.
      if (!emit.wrapper) { for (const s of step.wrapper) await runTool(s); continue; }
      const results = [];
      for (const s of step.wrapper) {
        const args = resolve(s.args || {}, saved);
        const r = await mcp.call(s.tool, args);
        log({ event: 'tool', scenario: scenario.name, tool: s.tool, args, wrapped: true, actionId: actionId(r), isError: !!r?.isError });
        if (s.save) saved[s.save] = r;
        results.push({ tool: s.tool, result: r });
      }
      emit.wrapper(`${personality}-wrap-${process.pid}-${++callNo}`, results, step.returnIds !== false);
    } else if (step.assess) {
      const args = resolve({ action: 'assess', ...step.assess }, saved);
      const p = runTool({ tool: 'chrome_use_task', args, save: step.save || 'assessment' });
      await p;
    } else if (step.assessFile) {
      // Written by the driver while this turn waits (check 18).
      const spec = JSON.parse(fs.readFileSync(path.join(ACCEPT, 'scenarios', step.assessFile + '.json'), 'utf8'));
      await runTool({ tool: 'chrome_use_task', args: resolve({ action: 'assess', ...spec }, saved), save: 'assessment' });
    } else if (step.approve) {
      // Ask the host before running a tool; the reply decides.
      const s = step.approve;
      const args = resolve(s.args || {}, saved);
      const reply = await emit.request(s.tool, args);
      log({ event: 'approvalReply', scenario: scenario.name, reply });
      if (reply?.allowed) await runTool(s, reply.input || args);
      else emit.text(`The ${s.tool} call was not allowed.`, null, null);
    } else if (step.request) {
      // A host request with no tool behind it (approval timing checks).
      const reply = await emit.request(step.request.tool || 'shell', resolve(step.request.args || {}, saved), step.request.kind);
      log({ event: 'requestReply', scenario: scenario.name, reply });
    } else if (step.signal) {
      fs.mkdirSync(path.join(ACCEPT, 'signals'), { recursive: true });
      fs.writeFileSync(signalPath(mcp.conversation, step.signal), String(Date.now()));
    } else if (step.waitFor) {
      const end = Date.now() + (step.timeoutMs || 60000);
      while (!fs.existsSync(signalPath(mcp.conversation, step.waitFor)) && Date.now() < end) {
        if (control.interrupted && !control.ignoreInterrupt) break;
        await sleep(100);
      }
    } else if (step.sleep) {
      const end = Date.now() + step.sleep;
      while (Date.now() < end) { if (control.interrupted && !control.ignoreInterrupt) break; await sleep(100); }
    } else if (step.spawnChild) {
      // Same process group as this leader; outlives the leader if only the
      // leader dies.
      const child = spawn('/bin/sleep', ['600'], { stdio: 'ignore' });
      child.unref();
      fs.mkdirSync(path.join(ACCEPT, 'signals'), { recursive: true });
      fs.writeFileSync(signalPath(mcp.conversation, 'child-pid'), String(child.pid));
      log({ event: 'spawnChild', childPid: child.pid });
    } else if (step.mcpStart) {
      await mcp.start(step.mcpStart);
    }
  }
  // A real backend finishes running tool calls before it confirms an
  // interrupt, so a slow action admitted before Stop reports afterwards.
  await Promise.allSettled([...inflight]);
  if (control.ignoreInterrupt && control.interrupted) await sleep(3600_000);
  const status = control.interrupted ? 'interrupted' : (scenario.end || 'completed');
  log({ event: 'turnEnd', scenario: scenario.name, status });
  emit.end(status);
}

// ------------------------------------------------------- personalities
const control = { interrupted: false, ignoreInterrupt: false, saved: {} };
let mcp = null;
const lines = readline.createInterface({ input: process.stdin });
const pendingReplies = new Map();

if (personality === 'codex') codex();
else if (personality === 'claude') claude();
else if (personality === 'pi') pi();
else if (personality === 'opencode') acp();
else { log({ event: 'unknownPersonality' }); process.exit(64); }

function codex() {
  let thread = null; let turn = null; let reqNo = 1000; let busy = false;
  const note = (method, params) => out({ method, params: { threadId: thread, ...params } });
  lines.on('line', async line => {
    const m = JSON.parse(line);
    log({ event: 'in', m: JSON.stringify(m).slice(0, 2000) });
    if (m.id != null && !m.method) { const fn = pendingReplies.get(String(m.id)); if (fn) { pendingReplies.delete(String(m.id)); fn(m.result); } return; }
    switch (m.method) {
      case 'initialize': return out({ id: m.id, result: { userAgent: 'codex_fixture/0.0.0-fixture (fixture)' } });
      case 'initialized': return;
      case 'config/read': return out({ id: m.id, result: { config: { mcp_servers: {}, model_reasoning_effort: 'medium' } } });
      case 'thread/start': case 'thread/resume': {
        thread = m.method === 'thread/resume' ? m.params.threadId : 'fixture-thread-' + randomUUID();
        mcp = new Mcp(m.params.config.mcp_servers.chrome_use);
        log({ event: 'thread', method: m.method, thread, approvalPolicy: m.params.approvalPolicy, mcpServer: { ...m.params.config.mcp_servers.chrome_use, env: undefined } });
        return out({ id: m.id, result: { thread: { id: thread }, model: 'fixture-model' } });
      }
      case 'turn/start': {
        turn = 'fixture-turn-' + randomUUID(); busy = true; control.interrupted = false;
        out({ id: m.id, result: { turn: { id: turn } } });
        note('turn/started', { turn: { id: turn } });
        const text = m.params.input.map(i => i.text || '').join('\n');
        let item = 0;
        const emit = {
          text(t, deltas, role) {
            const id = `msg-${turn}-${++item}`;
            note('item/started', { turnId: turn, item: { type: 'agentMessage', id, text: '', phase: role === 'commentary' ? 'commentary' : 'final_answer' } });
            for (const d of deltas || [t]) note('item/agentMessage/delta', { turnId: turn, itemId: id, delta: d });
            note('item/completed', { turnId: turn, item: { type: 'agentMessage', id, text: t, phase: role === 'commentary' ? 'commentary' : 'final_answer' } });
          },
          toolStart(id, name, args) { note('item/started', { turnId: turn, item: { type: 'mcpToolCall', id, server: 'chrome_use', tool: name, arguments: args, status: 'inProgress' } }); },
          toolEnd(id, name, args, result) { note('item/completed', { turnId: turn, item: { type: 'mcpToolCall', id, server: 'chrome_use', tool: name, arguments: args, status: result?.isError ? 'failed' : 'completed', result: { content: result?.content || [], structuredContent: result?.structuredContent ?? null }, error: result?.isError ? { message: 'tool error' } : null } }); },
          request(tool, args, kind) {
            const id = ++reqNo;
            const method = kind === 'elicitation' ? 'mcpServer/elicitation/request' : 'item/commandExecution/requestApproval';
            const params = kind === 'elicitation'
              ? { threadId: thread, turnId: turn, serverName: 'chrome_use', message: `Allow ${tool}?`, _meta: { codex_approval_kind: 'mcp_tool_call' }, mode: 'form', requestedSchema: { type: 'object', properties: {} } }
              : { threadId: thread, turnId: turn, itemId: `cmd-${id}`, command: `chrome-use ${tool} ${JSON.stringify(args)}`, cwd: process.cwd(), reason: 'fixture approval' };
            out({ id, method, params });
            return new Promise(res => pendingReplies.set(String(id), result => {
              note('serverRequest/resolved', { requestId: id });
              const d = result?.decision ?? result?.action;
              res({ allowed: d === 'accept' || d === 'acceptForSession' || d === 'approved', raw: result });
            }));
          },
          end(status) { busy = false; note('turn/completed', { turn: { id: turn, status, error: status === 'failed' ? { message: 'fixture failure' } : null } }); },
        };
        runTurn(text, emit, mcp, control);
        return;
      }
      case 'turn/interrupt': {
        out({ id: m.id, result: {} });
        if (busy) control.interrupted = true;
        return;
      }
      default:
        if (m.id != null) out({ id: m.id, result: {} });
    }
  });
}

function claude() {
  const resume = arg('--resume');
  const session = resume || randomUUID();
  const mcpConfig = JSON.parse(fs.readFileSync(arg('--mcp-config'), 'utf8'));
  mcp = new Mcp(mcpConfig.mcpServers.chrome_use);
  log({ event: 'claudeStart', resume, permissionMode: arg('--permission-mode'), tools: arg('--tools') });
  let msgNo = 0; let reqNo = 0; let busy = false;
  lines.on('line', line => {
    const m = JSON.parse(line);
    log({ event: 'in', m: JSON.stringify(m).slice(0, 2000) });
    if (m.type === 'control_request' && m.request?.subtype === 'initialize') return out({ type: 'control_response', response: { subtype: 'success', request_id: m.request_id, response: {} } });
    if (m.type === 'control_request' && m.request?.subtype === 'interrupt') {
      out({ type: 'control_response', response: { subtype: 'success', request_id: m.request_id, response: {} } });
      if (busy) control.interrupted = true;
      return;
    }
    if (m.type === 'control_response') { const fn = pendingReplies.get(m.response.request_id); if (fn) { pendingReplies.delete(m.response.request_id); fn(m.response); } return; }
    if (m.type !== 'user') return;
    busy = true; control.interrupted = false;
    out({ type: 'system', subtype: 'init', session_id: session, model: 'fixture-model', claude_code_version: '0.0.0-fixture', tools: [] });
    const content = m.message.content;
    const text = typeof content === 'string' ? content : content.map(b => b.text || '').join('\n');
    let finalText = '';
    const emit = {
      text(t, deltas) {
        const id = `msg_fixture_${++msgNo}`;
        out({ type: 'stream_event', event: { type: 'message_start', message: { id } }, session_id: session });
        for (const d of deltas || [t]) out({ type: 'stream_event', event: { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: d } }, session_id: session });
        out({ type: 'assistant', message: { id, role: 'assistant', content: [{ type: 'text', text: t }] }, session_id: session });
        finalText = t;
      },
      toolStart(id, name, args) { out({ type: 'assistant', message: { id: `msg_fixture_${++msgNo}`, role: 'assistant', content: [{ type: 'tool_use', id, name: 'mcp__chrome_use__' + name, input: args }] }, session_id: session }); },
      toolEnd(id, name, args, result) {
        // Claude 2.1.300 keeps only JSON.stringify(structuredContent) as the
        // model-visible tool_result text when structuredContent is present.
        const text = result?.structuredContent ? JSON.stringify(result.structuredContent) : (result?.content || []).map(b => b.text || '').join('\n');
        out({ type: 'user', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content: text, is_error: !!result?.isError }] }, tool_use_result: result, session_id: session });
      },
      request(tool, args) {
        const id = `fixture-perm-${++reqNo}`;
        out({ type: 'control_request', request_id: id, request: { subtype: 'can_use_tool', tool_name: 'mcp__chrome_use__' + tool, input: args, tool_use_id: `toolu_perm_${reqNo}` } });
        return new Promise(res => pendingReplies.set(id, response => {
          const r = response.response || {};
          log({ event: 'canUseToolReply', subtype: response.subtype, behavior: r.behavior, updatedInputEqualsOriginal: JSON.stringify(r.updatedInput) === JSON.stringify(args), updatedInput: r.updatedInput });
          res({ allowed: r.behavior === 'allow', input: r.updatedInput, raw: response });
        }));
      },
      end(status) {
        busy = false;
        out({ type: 'result', subtype: status === 'completed' ? 'success' : 'error_during_execution', is_error: status !== 'completed', result: status === 'completed' ? finalText : '', session_id: session, num_turns: 1 });
      },
    };
    runTurn(text, emit, mcp, control);
  });
}

function pi() {
  const session = arg('--session-id') || randomUUID();
  const config = JSON.parse(fs.readFileSync(path.join(process.cwd(), '.pi', 'mcp.json'), 'utf8'));
  mcp = new Mcp(config.mcpServers.chrome_use);
  let msgIndex = 0; let busy = false;
  lines.on('line', line => {
    const m = JSON.parse(line);
    log({ event: 'in', m: JSON.stringify(m).slice(0, 2000) });
    if (m.type === 'get_state') return out({ id: m.id, type: 'response', command: 'get_state', success: true, data: { sessionId: session, model: { provider: 'fixture', id: 'pi-model' } } });
    if (m.type === 'abort') { out({ id: m.id, type: 'response', command: 'abort', success: true }); if (busy) control.interrupted = true; return; }
    if (m.type !== 'prompt') { if (m.id) out({ id: m.id, type: 'response', command: m.type, success: true }); return; }
    out({ id: m.id, type: 'response', command: 'prompt', success: true, data: { disposition: 'started' } });
    out({ type: 'agent_start' });
    busy = true; control.interrupted = false;
    const emit = {
      text(t, deltas) {
        msgIndex++;
        out({ type: 'message_start', message: { role: 'assistant' } });
        for (const d of deltas || [t]) out({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: d } });
        out({ type: 'message_update', assistantMessageEvent: { type: 'text_end', contentIndex: 0, content: t } });
        out({ type: 'message_end', message: { role: 'assistant', stopReason: 'stop' } });
      },
      toolStart(id, name, args) { out({ type: 'tool_execution_start', toolCallId: id, toolName: 'mcp__chrome_use__' + name, args }); },
      toolEnd(id, name, args, result) { out({ type: 'tool_execution_end', toolCallId: id, toolName: 'mcp__chrome_use__' + name, args, result: { content: result?.content || [], structuredContent: result }, isError: !!result?.isError }); },
      request: async () => ({ allowed: true }),
      end(status) {
        busy = false;
        if (status === 'failed') out({ type: 'message_end', message: { role: 'assistant', stopReason: 'error', errorMessage: 'fixture failure' } });
        out({ type: 'agent_end', messages: [] });
        out({ type: 'agent_settled', aborted: status === 'interrupted' });
      },
    };
    runTurn(m.message, emit, mcp, control);
  });
}

function acp() {
  let session = null; let promptId = null; let reqNo = 5000; let busy = false;
  const update = u => out({ jsonrpc: '2.0', method: 'session/update', params: { sessionId: session, update: u } });
  lines.on('line', line => {
    const m = JSON.parse(line);
    log({ event: 'in', m: JSON.stringify(m).slice(0, 2000) });
    if (m.id != null && !m.method) { const fn = pendingReplies.get(String(m.id)); if (fn) { pendingReplies.delete(String(m.id)); fn(m.result); } return; }
    const p = m.params || {};
    switch (m.method) {
      case 'initialize':
        return out({ jsonrpc: '2.0', id: m.id, result: { protocolVersion: 1, agentInfo: { name: 'opencode-fixture', version: '0.0.0-fixture' }, agentCapabilities: { loadSession: true, promptCapabilities: { image: false }, sessionCapabilities: { resume: {} } } } });
      case 'session/new': case 'session/resume': case 'session/load':
        session = m.method === 'session/new' ? 'ses_fixture_' + randomUUID() : p.sessionId;
        mcp = new Mcp(p.mcpServers[0]);
        return out({ jsonrpc: '2.0', id: m.id, result: { sessionId: session, configOptions: [{ id: 'model', type: 'select', currentValue: 'fixture/model', options: [{ value: 'fixture/model', name: 'Fixture' }] }] } });
      case 'session/cancel':
        if (busy) control.interrupted = true;
        return;
      case 'session/prompt': {
        promptId = m.id; busy = true; control.interrupted = false;
        const text = p.prompt.map(b => b.text || '').join('\n');
        const wrapperUpdate = (id, calls, returnIds, status) => {
          const content = returnIds
            ? calls.map(c => ({ type: 'content', content: { type: 'text', text: JSON.stringify(c.result?.structuredContent ?? c.result) } }))
            : [{ type: 'content', content: { type: 'text', text: 'done' } }];
          update({ sessionUpdate: 'tool_call_update', toolCallId: id, status, rawOutput: { metadata: { toolCalls: calls.map(c => ({ tool: 'chrome_use.' + c.tool, status: c.result?.isError ? 'error' : 'completed' })), error: calls.some(c => c.result?.isError) } }, content });
        };
        const emit = {
          text(t, deltas) { for (const d of deltas || [t]) update({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: d } }); },
          toolStart(id, name, args) { update({ sessionUpdate: 'tool_call', toolCallId: id, title: 'execute', kind: 'other', status: 'in_progress', rawInput: { code: `await tools.chrome_use.${name}(${JSON.stringify(args)})` } }); },
          toolEnd(id, name, args, result) { wrapperUpdate(id, [{ tool: name, result }], true, 'completed'); },
          wrapper(id, results, returnIds) {
            update({ sessionUpdate: 'tool_call', toolCallId: id, title: 'execute', kind: 'other', status: 'in_progress', rawInput: { code: results.map(r => `await tools.chrome_use.${r.tool}()`).join(';') } });
            wrapperUpdate(id, results, returnIds, 'completed');
          },
          request(tool, args) {
            const id = ++reqNo;
            out({ jsonrpc: '2.0', id, method: 'session/request_permission', params: { sessionId: session, toolCall: { toolCallId: `perm-${id}`, title: tool, rawInput: args }, options: [{ optionId: 'once', name: 'Allow once', kind: 'allow_once' }, { optionId: 'reject', name: 'Reject', kind: 'reject_once' }] } });
            return new Promise(res => pendingReplies.set(String(id), result => res({ allowed: result?.outcome?.outcome === 'selected' && result?.outcome?.optionId === 'once', raw: result })));
          },
          end(status) {
            busy = false;
            out({ jsonrpc: '2.0', id: promptId, result: { stopReason: { completed: 'end_turn', interrupted: 'cancelled', failed: 'refusal' }[status] } });
          },
        };
        runTurn(text, emit, mcp, control);
        return;
      }
      default:
        if (m.id != null) out({ jsonrpc: '2.0', id: m.id, result: {} });
    }
  });
}
lines.on('close', () => { log({ event: 'stdinClosed' }); setTimeout(() => process.exit(0), 200); });
