// Panel-service client, a transparent fault-injecting service proxy and
// journal helpers for the verifiable-panel-turns live checks (Node 22+).
import net from 'node:net';
import fs from 'node:fs';
import path from 'node:path';
import { randomUUID, createHash } from 'node:crypto';

export const sleep = ms => new Promise(r => setTimeout(r, ms));
export const sha256 = bytes => 'sha256:' + createHash('sha256').update(bytes).digest('hex');

export async function waitFor(fn, { timeout = 10000, interval = 200, label = 'condition' } = {}) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try { last = await fn(); if (last) return last; } catch (e) { last = e; }
    await sleep(interval);
  }
  throw new Error(`timed out waiting for ${label}${last instanceof Error ? ': ' + last.message : ''}`);
}

// One request per connection, 4-byte little-endian length + JSON, as the
// service's own `exchange` does (service.rs).
function frame(value) {
  const body = Buffer.from(JSON.stringify(value));
  const head = Buffer.alloc(4); head.writeUInt32LE(body.length);
  return Buffer.concat([head, body]);
}
function readFrame(socket, timeout) {
  return new Promise((resolve, reject) => {
    let buf = Buffer.alloc(0);
    const timer = setTimeout(() => { socket.destroy(); reject(new Error('service reply timed out')); }, timeout);
    socket.on('data', d => {
      buf = Buffer.concat([buf, d]);
      if (buf.length >= 4) {
        const len = buf.readUInt32LE(0);
        if (buf.length >= 4 + len) { clearTimeout(timer); resolve(JSON.parse(buf.subarray(4, 4 + len))); socket.end(); }
      }
    });
    socket.on('error', e => { clearTimeout(timer); reject(e); });
    socket.on('close', () => { clearTimeout(timer); reject(new Error('service closed the connection without a reply')); });
  });
}
function connect(address) {
  const i = address.lastIndexOf(':');
  return net.connect({ host: address.slice(0, i), port: Number(address.slice(i + 1)) });
}

export class Service {
  constructor(serviceJson) { this.serviceJson = serviceJson; }
  endpoint() { return JSON.parse(fs.readFileSync(this.serviceJson, 'utf8')); }
  async raw(operation, params = {}, timeout = 70000) {
    const e = this.endpoint();
    const socket = connect(e.address);
    await new Promise((r, j) => { socket.once('connect', r); socket.once('error', j); });
    const requestId = randomUUID();
    socket.write(frame({ capability: e.capability, requestId, operation, params }));
    return readFrame(socket, timeout);
  }
  async call(operation, params = {}, timeout) {
    const reply = await this.raw(operation, params, timeout);
    if (reply.error) { const err = new Error(reply.error); err.serviceError = reply.error; throw err; }
    return reply.result;
  }
  async tryCall(operation, params = {}, timeout) {
    try { return { ok: true, result: await this.call(operation, params, timeout) }; } catch (e) { return { ok: false, error: e.serviceError || String(e.message) }; }
  }
  async journal(conversationId, after = 0) {
    const events = [];
    let cursor = after;
    for (;;) {
      const r = await this.call('conversationRead', { conversationId, after: cursor });
      events.push(...r.journal.events);
      cursor = r.journal.cursor;
      if (!r.journal.more) return { events, conversation: r.conversation, queued: r.queuedMessages, raw: r };
    }
  }
}

// A transparent proxy in front of the service. The service.json that every
// client reads (nm-host, the conversations' chrome_use MCP servers, this
// driver) is rewritten to point here. Faults are armed per check:
//   holdBegin(conversationId)  hold that conversation's agentActionBegin frames
//   dropReply(messageId)       deliver conversationSend upstream, drop its reply
export class Proxy {
  constructor(serviceJson, logFile) {
    this.serviceJson = serviceJson;
    this.logFile = logFile;
    this.holds = new Map();
    this.held = [];
    this.drops = new Set();
    this.timings = [];
  }
  log(entry) { fs.appendFileSync(this.logFile, JSON.stringify({ at: Date.now(), ...entry }) + '\n'); }
  async start() {
    this.upstream = JSON.parse(fs.readFileSync(this.serviceJson, 'utf8'));
    this.server = net.createServer(client => this.handle(client));
    await new Promise(r => this.server.listen(0, '127.0.0.1', r));
    const rewritten = { ...this.upstream, address: `127.0.0.1:${this.server.address().port}` };
    const tmp = this.serviceJson + '.tmp-' + process.pid;
    fs.writeFileSync(tmp, JSON.stringify(rewritten), { mode: 0o600 });
    fs.renameSync(tmp, this.serviceJson);
    this.log({ event: 'proxyStart', upstream: this.upstream.address, proxy: rewritten.address });
  }
  stop() { try { this.server.close(); } catch {} }
  async forward(request) {
    const socket = connect(this.upstream.address);
    await new Promise((r, j) => { socket.once('connect', r); socket.once('error', j); });
    socket.write(frame(request));
    return readFrame(socket, 120000);
  }
  handle(client) {
    let buf = Buffer.alloc(0);
    client.on('error', () => {});
    client.on('data', async d => {
      buf = Buffer.concat([buf, d]);
      if (buf.length < 4) return;
      const len = buf.readUInt32LE(0);
      if (buf.length < 4 + len) return;
      const request = JSON.parse(buf.subarray(4, 4 + len));
      buf = Buffer.alloc(0);
      const op = request.operation;
      const conv = request.params?.callerConversation || request.params?.conversationId;
      const started = Date.now();
      if (op === 'agentActionBegin' && this.holds.has(conv) && this.holds.get(conv).remaining > 0) {
        const gate = this.holds.get(conv);
        gate.remaining--;
        this.held.push({ at: started, request });
        this.log({ event: 'held', op, conv, actionId: request.params.actionId, tool: request.params.tool, generation: request.params.generation });
        await gate.promise;
        const reply = await this.forward(request).catch(e => ({ error: String(e) }));
        this.log({ event: 'released', op, conv, actionId: request.params.actionId, reply });
        gate.replies.push({ request: request.params, reply });
        try { client.write(frame(reply)); } catch {}
        return;
      }
      let reply;
      try { reply = await this.forward(request); } catch (e) { this.log({ event: 'forwardError', op, error: String(e) }); client.destroy(); return; }
      const ms = Date.now() - started;
      if (op === 'agentActionBegin' || op === 'agentActionReport') this.timings.push({ op, ms });
      if (op.startsWith('agent') || op.startsWith('conversationSend')) this.log({ event: 'op', op, conv, ms, error: reply.error || null });
      if ((op === 'conversationSend' || op === 'conversationSendDirect') && this.drops.has(request.params.messageId)) {
        this.drops.delete(request.params.messageId);
        this.log({ event: 'droppedReply', op, messageId: request.params.messageId, reply });
        client.destroy();
        return;
      }
      try { client.write(frame(reply)); } catch {}
    });
  }
  // Hold the next `count` Begins of a conversation; later ones pass.
  holdBegin(conversationId, count = 1) {
    let release;
    const promise = new Promise(r => { release = r; });
    const gate = { promise, release, replies: [], remaining: count, held: () => this.held.filter(x => x.request.params.callerConversation === conversationId) };
    this.holds.set(conversationId, gate);
    return gate;
  }
  releaseBegin(conversationId) { const g = this.holds.get(conversationId); g?.release(); return g; }
  dropReply(messageId) { this.drops.add(messageId); }
}

// Journal helpers.
export const kinds = (events, kind) => events.filter(e => e.kind === kind);
export const one = (events, kind, pred = () => true) => events.find(e => e.kind === kind && pred(e.data, e));
export const ofMessage = (events, messageId) => events.filter(e => e.data?.messageId === messageId);
export function grep(text, needle) { return text.includes(needle); }
export function readDirTexts(dir, filter = () => true) {
  const out = [];
  const walk = d => { for (const e of fs.readdirSync(d, { withFileTypes: true })) { const p = path.join(d, e.name); if (e.isDirectory()) walk(p); else if (filter(p)) out.push([p, fs.readFileSync(p)]); } };
  if (fs.existsSync(dir)) walk(dir);
  return out;
}
// Hash of an extension tree (sorted relative paths + bytes), as rig.mjs does.
export function treeHash(root) {
  const h = createHash('sha256');
  const walk = d => fs.readdirSync(d, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name)).forEach(e => {
    const p = path.join(d, e.name);
    if (e.isDirectory()) walk(p); else { h.update(path.relative(root, p)); h.update(fs.readFileSync(p)); }
  });
  walk(root);
  return h.digest('hex');
}

// A plain (non-panel) chrome_use MCP client, for comparison runs against the
// same relay (check 16: the v1.5.214 binary's CDP traffic for the same steps).
export async function plainMcp(bin, env) {
  const { spawn } = await import('node:child_process');
  const proc = spawn(bin, ['mcp', '--tools', 'all'], { env: { ...process.env, ...env }, stdio: ['pipe', 'pipe', 'ignore'] });
  const pending = new Map(); let next = 0; let buf = '';
  proc.stdout.on('data', d => { buf += d; let i; while ((i = buf.indexOf('\n')) >= 0) { const line = buf.slice(0, i); buf = buf.slice(i + 1); try { const m = JSON.parse(line); if (pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } } catch {} } });
  const rpc = (method, params) => new Promise(resolve => { const id = ++next; pending.set(id, resolve); proc.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n'); });
  await rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'panel-accept', version: '0' } });
  proc.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
  return { call: async (name, args) => (await rpc('tools/call', { name, arguments: args })).result, close: () => proc.kill() };
}
