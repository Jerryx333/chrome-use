// Live check that the Chrome Web Store build of the extension (ab-connect
// 0.5.58, tag v1.5.219) still works with this CLI (build host only; one
// browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<0.5.58 extension> node compat-058-live.mjs
// The side panel is opened from a trusted click and reloaded with Runtime and
// Log enabled, so a throw during its startup is caught. Steps:
//   1. the panel starts without an exception and the local agents, Skills and
//      Schedules show in Agents & models;
//   2. "← Back" returns to the chat;
//   3. a new conversation with the Codex fixture backend gets its reply.
// Evidence and screenshots go to $OUT (default <work>/out).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/c58-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [], exceptions: [], console: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[compat-058] ${name}`, JSON.stringify(data).slice(0, 600)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const failures = [];
const check = (ok, message) => { console.log(`[compat-058] ${ok ? 'PASS' : 'FAIL'} ${message}`); if (!ok) failures.push(message); };

const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8950, cdpPort: 9411 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const A = rig.cdp, b = rig.browser;
const shot = async (sid, name) => { const s = await b.send('Page.captureScreenshot', { format: 'png' }, sid); fs.writeFileSync(path.join(out, `${name}.png`), Buffer.from(s.data, 'base64')); };
try {
  // Open the panel from a trusted click on an extension page.
  await rig.ev(`chrome.windows.create({ url: chrome.runtime.getURL('popup.html'), focused: true })`);
  const opener = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => t.url.endsWith('/popup.html') && t.type === 'page'), { label: 'opener page' });
  const osid = await A.attach(opener.targetId);
  await waitFor(() => A.evaluate(osid, 'document.readyState === "complete"'), { label: 'opener load' });
  await A.evaluate(osid, `(() => { const b = document.createElement('button'); b.style = 'position:fixed;left:0;top:0;width:200px;height:80px;z-index:2147483647'; b.onclick = () => chrome.windows.getCurrent().then(w => chrome.sidePanel.open({ windowId: w.id })); document.body.append(b); return true; })()`);
  await A.click(osid, 50, 40);
  const target = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => /\/sidepanel\.html(\?|$)/.test(t.url)), { timeout: 15000, label: 'side panel' });
  const sid = await A.attach(target.targetId);
  b.listeners.push(m => {
    if (m.sessionId !== sid) return;
    if (m.method === 'Runtime.exceptionThrown') evidence.exceptions.push(m.params.exceptionDetails);
    if (m.method === 'Runtime.consoleAPICalled') evidence.console.push({ type: m.params.type, args: m.params.args.map(a => a.value ?? a.description) });
    if (m.method === 'Log.entryAdded') evidence.console.push({ log: m.params.entry.level, text: m.params.entry.text, url: m.params.entry.url });
  });
  await b.send('Runtime.enable', {}, sid);
  await b.send('Log.enable', {}, sid);
  await b.send('Page.enable', {}, sid);
  await b.send('Page.reload', {}, sid);
  await sleep(1500);
  const run = expression => A.evaluate(sid, expression);
  await waitFor(() => run(`document.readyState === 'complete' && !/Checking/.test(document.getElementById('connection').textContent)`), { timeout: 30000, label: 'panel connection state' });
  await sleep(4000);
  const state = () => run(`({ connection: document.getElementById('connection').textContent, ok: document.getElementById('connection').classList.contains('ok'), notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').textContent, agentsView: !document.getElementById('agentsView').hidden, chatView: !document.getElementById('chatView').hidden, agentList: document.getElementById('agentList').innerText.slice(0, 600), skillsTab: !document.getElementById('skillsSettingsTab').hidden, schedulesTab: !document.getElementById('schedulesSettingsTab').hidden, agents: [...document.getElementById('agent').options].map(o => o.value), conversation: document.getElementById('conversation').value, transcript: document.getElementById('transcript').innerText.slice(-600) })`);
  const started = await state();
  step('panel started', { ...started, exceptions: evidence.exceptions.map(e => e.exception?.description || e.text) });
  await shot(sid, '1-started');
  check(evidence.exceptions.length === 0, 'no exception while the panel starts');
  check(started.ok, 'the panel reports a full connection');

  await run(`document.getElementById('agentsTab').click(), true`);
  await sleep(4000);
  const agents = await state();
  step('agents & models', { ...agents, exceptions: evidence.exceptions.length });
  await shot(sid, '2-agents');
  check(agents.agentsView && /codex/i.test(agents.agentList), 'Agents & models lists the local agents');
  check(agents.skillsTab && agents.schedulesTab, 'the Skills and Schedules tabs show');

  await run(`document.getElementById('chatTab').click(), true`);
  await sleep(1000);
  const back = await state();
  step('back', back);
  check(back.chatView && !back.agentsView, '"← Back" returns to the chat');

  fs.writeFileSync(path.join(rig.accept, 'scenarios', 'c58.json'), JSON.stringify({ steps: [{ tool: 'chrome_use_open', args: { url: rig.site + '/a' } }, { tool: 'chrome_use_screenshot', args: {} }, { tool: 'chrome_use_read', args: {} }, { say: 'Hello from the fixture.\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |' }] }));
  await run(`(() => { const s = document.getElementById('agent'); s.value = 'codex'; s.dispatchEvent(new Event('change')); document.getElementById('executable').value = ${JSON.stringify(rig.backend('codex'))}; document.getElementById('message').value = 'run scenario c58'; document.getElementById('composer').requestSubmit(); return true; })()`);
  const c1 = await waitFor(() => run(`document.getElementById('conversation').value`), { timeout: 30000, label: 'conversation created' }).catch(() => '');
  const reply = c1 && await waitFor(() => run(`document.getElementById('transcript').innerText.includes('Hello from the fixture.')`), { timeout: 90000, interval: 500, label: 'reply shown' }).catch(() => false);
  const sent = await state();
  step('sent', { c1, reply, ...sent, exceptions: evidence.exceptions.length });
  await shot(sid, '3-sent');
  check(Boolean(c1) && reply === true, 'a new conversation gets its reply in the panel');
  check(evidence.exceptions.length === 0, 'no exception during the run');
} catch (e) {
  failures.push(String(e.stack || e));
  console.log('[compat-058] ERROR', e.stack || e);
} finally {
  evidence.failures = failures;
  save();
  if (fs.existsSync(path.join(work, 'proxy.jsonl'))) fs.copyFileSync(path.join(work, 'proxy.jsonl'), path.join(out, 'proxy.jsonl'));
  await cleanup(rig);
  console.log(`[compat-058] ${failures.length ? 'FAILED' : 'PASSED'}; evidence in ${out}`);
  process.exit(failures.length ? 1 : 0);
}
