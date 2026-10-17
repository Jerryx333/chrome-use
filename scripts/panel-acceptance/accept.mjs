// Verifiable panel turns: live checks 1-20 of plan 14.2 on the build host.
// Every expectation is an assert; results.json keeps evidence per check.
// Usage (from run.sh): node accept.mjs   env: HOST_BIN CHROME EXT_DIR WORK OUT ONLY=1,4
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { startRig, cleanup, openTab, activate, cdpLog, EXT_ID as EXT } from './rig.mjs';
import { sleep, waitFor, kinds, one, ofMessage, sha256, plainMcp, treeHash } from './lib.mjs';
import { checks } from './checks.mjs';

const dir = path.dirname(new URL(import.meta.url).pathname);
const WORK = process.env.WORK;
const OUT = process.env.OUT;
const only = process.env.ONLY ? process.env.ONLY.split(',') : null;
fs.mkdirSync(OUT, { recursive: true });
const results = { startedAt: new Date().toISOString(), hostBin: process.env.HOST_BIN, checks: [] };
const save = () => fs.writeFileSync(path.join(OUT, process.env.RESULTS_NAME || 'results.json'), JSON.stringify(results, null, 2));

const rig = await startRig({ dir, work: WORK, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR });
results.chrome = rig.chromeVersion;
results.hostSha256 = sha256(fs.readFileSync(process.env.HOST_BIN));
results.buildCommit = process.env.BUILD_COMMIT || null;
results.extensionTree = treeHash(process.env.EXT_DIR);
if (process.env.OLD_BIN && fs.existsSync(process.env.OLD_BIN)) results.v15214Sha256 = sha256(fs.readFileSync(process.env.OLD_BIN));
if (process.env.OLD_EXT_DIR && fs.existsSync(process.env.OLD_EXT_DIR)) results.v15214ExtensionTree = treeHash(process.env.OLD_EXT_DIR);
results.profile = rig.profile;
results.extensionVersion = rig.extensionVersion;
process.on('exit', () => cleanup(rig));
process.on('SIGTERM', () => process.exit(143));

// ---------------------------------------------------------- shared helpers
const h = {
  rig, sleep, waitFor, kinds, one, ofMessage, sha256, openTab: url => openTab(rig, url), activate: tab => activate(rig, tab), cdpLog: () => cdpLog(rig), assert,
  site: p => rig.site + p,
  plainMcp,
  out: OUT,
  scenario(name, spec) { fs.writeFileSync(path.join(rig.accept, 'scenarios', name + '.json'), JSON.stringify(spec, null, 1)); return `run scenario ${name}`; },
  signal(conv, name) { fs.writeFileSync(path.join(rig.accept, 'signals', `${conv}.${name}`), String(Date.now())); },
  hasSignal(conv, name) { return fs.existsSync(path.join(rig.accept, 'signals', `${conv}.${name}`)); },
  async create(agentId, { real = false, executable, fullAccess = true, options, model } = {}) {
    const params = { agentId, fullAccess };
    if (model) params.model = model;
    if (!real) params.executable = rig.backend(agentId);
    if (executable) params.executable = executable;
    if (options) params.options = options;
    return (await rig.service.call('conversationCreate', params)).conversation;
  },
  async send(conversationId, text, { page, record, direct, messageId = randomUUID(), attachments } = {}) {
    const params = { conversationId, messageId, text };
    if (page) params.page = { tabId: page.tabId, url: page.url, title: page.title || '' };
    if (record) params.record = true;
    if (attachments) params.attachments = attachments;
    const result = await rig.service.call(direct ? 'conversationSendDirect' : 'conversationSend', params);
    return { messageId, result };
  },
  journal: (id, after = 0) => rig.service.journal(id, after),
  async events(id) { return (await rig.service.journal(id)).events; },
  async turnEnd(id, messageId, timeout = 90000) {
    return waitFor(async () => {
      const ev = await h.events(id);
      const end = one(ev, 'turnEnded', d => d.messageId === messageId);
      // The final assessment and summary follow turnEnded in the same lock.
      return end && one(ev, 'turnSummary', d => d.messageId === messageId) ? ev : null;
    }, { timeout, interval: 300, label: `turnEnded ${messageId}` });
  },
  async idle(id, timeout = 60000) {
    return waitFor(async () => { const c = (await rig.service.journal(id)).conversation; return ['idle', 'interrupted', 'failed'].includes(c.phase) && !c.activeMessageId ? c : null; }, { timeout, label: 'conversation idle' });
  },
  async waitEvent(id, kind, pred = () => true, timeout = 60000) {
    return waitFor(async () => one(await h.events(id), kind, pred), { timeout, interval: 250, label: `${kind} in ${id}` });
  },
  // Whole artifact through the panel's chunked read; returns bytes + replies.
  async artifactRead(conversationId, artifactId, service = rig.service) {
    const chunks = []; let offset = 0; let last;
    for (;;) {
      last = await service.call('conversationArtifactRead', { conversationId, artifactId, offset, length: 384 * 1024 });
      const data = Buffer.from(last.data, 'base64');
      chunks.push(data); offset += data.length;
      if (offset >= last.total || data.length === 0) break;
    }
    const bytes = Buffer.concat(chunks);
    return { bytes, digest: last.digest, total: last.total, computed: sha256(bytes) };
  },
  async respond(conversationId, request, decision) {
    const d = request.data;
    return rig.service.tryCall('conversationRespond', { conversationId, requestId: d.requestId, generation: d.generation, requestRevision: d.requestRevision, decision });
  },
  state: {},
  // Side-panel expectations are recorded without stopping the check, so the
  // service-level evidence above them is always complete.
  panelCheck(ev, ok, label) { (ev.panelChecks ||= []).push({ label, ok: !!ok }); },
  folder: id => path.join(rig.profileDir, 'conversations', id),
  backendLog: () => fs.readFileSync(path.join(rig.accept, 'backends.jsonl'), 'utf8').trim().split('\n').filter(Boolean).map(l => JSON.parse(l)),
  siteLog: () => fs.existsSync(rig.siteLog) ? fs.readFileSync(rig.siteLog, 'utf8').trim().split('\n').filter(Boolean).map(l => JSON.parse(l)) : [],
  finalAssessment: (ev, messageId) => one(ev, 'taskAssessment', d => d.messageId === messageId && d.phase === 'final')?.data,
  actions(ev, messageId) {
    const map = new Map();
    for (const e of ev) {
      const d = e.data || {};
      if (!d.actionId || (messageId && d.messageId !== messageId)) continue;
      const a = map.get(d.actionId) || { actionId: d.actionId };
      if (e.kind === 'actionAdmitted') a.admitted = { ...d, sequence: e.sequence };
      if (e.kind === 'browserActionTarget') a.target = { ...d, sequence: e.sequence };
      if (e.kind === 'actionObserved') a.observed = { ...d, sequence: e.sequence };
      map.set(d.actionId, a);
    }
    return [...map.values()];
  },
  // The real side panel (a side-panel target, not a tab: agent requests from
  // a tab are refused). sidePanel.open needs a user gesture, so a trusted CDP
  // click on a button in an extension page opens it.
  async openPanel(windowId) {
    const win = windowId ?? (await rig.ev('chrome.windows.getLastFocused()')).id;
    // Since 0.5.63 each tab has its own panel (setOptions for the tab, then
    // open it), the way the toolbar button does; older builds open one panel
    // per window. The opener lives in its own window so the page tab stays
    // the active one.
    const tab = (await rig.ev(`chrome.tabs.query({ active: true, windowId: ${win} })`))?.[0]?.id;
    const perTab = await rig.ev(`chrome.runtime.getManifest().version.split('.').map(Number).reduce((a, n) => a * 1000 + n, 0) >= 5063`);
    const before = new Set((await rig.browser.send('Target.getTargets')).targetInfos.filter(t => t.url.includes('/sidepanel.html')).map(t => t.targetId));
    const { targetId: opener } = await rig.browser.send('Target.createTarget', { url: `chrome-extension://${EXT}/sidepanel.html?opener=1`, ...(perTab ? { newWindow: true } : {}) });
    const os = await rig.cdp.attach(opener);
    await waitFor(() => rig.cdp.evaluate(os, 'document.readyState === "complete" && !!document.body'), { label: 'opener load' });
    await sleep(500);
    const open = perTab && Number.isInteger(tab)
      ? `chrome.sidePanel.setOptions({ tabId: ${tab}, path: 'sidepanel.html?tab=${tab}', enabled: true }); chrome.sidePanel.open({ tabId: ${tab} })`
      : `chrome.sidePanel.open({ windowId: ${win} })`;
    await rig.cdp.evaluate(os, `(() => { const b = document.createElement('button'); b.id = '__open'; b.style = 'position:fixed;left:0;top:0;width:200px;height:80px;z-index:2147483647'; b.onclick = () => ${open}.then(() => { b.dataset.r = 'ok'; }, e => { b.dataset.r = e.message; }); (document.body || document.documentElement).append(b); return true; })()`);
    await rig.cdp.click(os, 50, 40);
    const target = await waitFor(async () => (await rig.browser.send('Target.getTargets')).targetInfos.find(t => t.url.includes('/sidepanel.html') && !t.url.includes('opener=1') && !before.has(t.targetId)), { label: 'side panel target' }).catch(async error => {
      const result = await rig.cdp.evaluate(os, `document.getElementById('__open')?.dataset.r || 'no result'`).catch(e => e.message);
      const targets = (await rig.browser.send('Target.getTargets')).targetInfos.map(t => `${t.type}:${t.url.slice(0, 80)}`);
      throw new Error(`${error.message} (perTab=${perTab} tab=${tab} win=${win} open: ${result}; targets: ${targets.join(' | ')})`);
    });
    await rig.browser.send('Target.closeTarget', { targetId: opener }).catch(() => {});
    const sid = await rig.cdp.attach(target.targetId);
    await waitFor(() => rig.cdp.evaluate(sid, 'document.readyState === "complete"'), { label: 'panel load' });
    return { sid, targetId: target.targetId };
  },
  async screenshot(name, conversationId, panel) {
    panel ||= (h.panel ||= await h.openPanel());
    await rig.ev(`chrome.storage.local.set({ ab_panel_conversation: ${JSON.stringify(conversationId)}, ab_panel_conversation_scope: "all" })`);
    await rig.browser.send('Page.reload', {}, panel.sid);
    await sleep(1000);
    await waitFor(() => rig.cdp.evaluate(panel.sid, 'document.readyState === "complete"'), { label: 'panel reload' });
    await sleep(3000);
    const shot = await rig.browser.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true }, panel.sid);
    const file = path.join(OUT, `panel-${name}.png`);
    fs.writeFileSync(file, Buffer.from(shot.data, 'base64'));
    const text = await rig.cdp.evaluate(panel.sid, 'document.body.innerText');
    const transcript = await rig.cdp.evaluate(panel.sid, `(document.getElementById('transcript') || document.body).innerText`);
    const cards = await rig.cdp.evaluate(panel.sid, `document.querySelectorAll('#transcript article.turn-card').length`);
    return { file: path.basename(file), text: text.slice(0, 6000), transcript: transcript.slice(0, 6000), turnCards: cards, tooManyRequestsBanner: /Too many native agent panel requests/.test(text) };
  },
  panelEval: (expr, panel) => rig.cdp.evaluate((panel || h.panel).sid, expr),
};

for (const check of checks) {
  if (only && !only.includes(String(check.id))) continue;
  const started = Date.now();
  const record = { id: check.id, name: check.name, evidence: {} };
  console.log(`== check ${check.id}: ${check.name}`);
  try {
    const outcome = await Promise.race([check.run(h, record.evidence), new Promise((_, j) => setTimeout(() => j(new Error(`check timed out after ${check.timeout || 300} s`)), (check.timeout || 300) * 1000))]);
    record.status = outcome?.status || 'pass';
    if (outcome?.note) record.note = outcome.note;
    const panelFailed = (record.evidence.panelChecks || []).filter(c => !c.ok).map(c => c.label);
    if (panelFailed.length) { record.status = 'fail'; record.error = 'side panel: ' + panelFailed.join('; '); }
  } catch (e) {
    record.status = 'fail';
    record.error = String(e.stack || e).slice(0, 4000);
  }
  record.seconds = Math.round((Date.now() - started) / 100) / 10;
  results.checks.push(record);
  console.log(`   ${record.status}${record.error ? ': ' + record.error.split('\n')[0] : ''}`);
  save();
}
results.finishedAt = new Date().toISOString();
results.proxyTimings = summarize(rig.proxy.timings);
save();
function summarize(t) {
  const by = {};
  for (const { op, ms } of t) (by[op] ||= []).push(ms);
  return Object.fromEntries(Object.entries(by).map(([op, list]) => { list.sort((a, b) => a - b); return [op, { n: list.length, p50: list[Math.floor(list.length / 2)], p95: list[Math.floor(list.length * 0.95)], max: list.at(-1) }]; }));
}
process.exit(results.checks.some(c => c.status === 'fail') ? 1 : 0);
