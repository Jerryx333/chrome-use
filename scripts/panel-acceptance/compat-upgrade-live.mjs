// Live check of an in-place CLI upgrade under the Chrome Web Store extension
// (ab-connect 0.5.58): the panel is used with OLD_BIN, a conversation is left
// running, then the native host is switched to NEW_BIN while OLD_BIN's panel
// service keeps running, as after `chrome-use upgrade` (build host only; one
// browser run per host at a time):
//   OLD_BIN=<chrome-use> NEW_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node compat-upgrade-live.mjs
// BUSY=0 leaves no conversation running. Evidence goes to $OUT.
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/c58-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const busy = process.env.BUSY !== '0';
const evidence = { work, oldBin: process.env.OLD_BIN, newBin: process.env.NEW_BIN, busy, steps: [], exceptions: [], workerExceptions: [], console: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[upgrade] ${name}`, JSON.stringify(data).slice(0, 900)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const failures = [];
const check = (ok, message) => { console.log(`[upgrade] ${ok ? 'PASS' : 'FAIL'} ${message}`); if (!ok) failures.push(message); };
const within = (promise, ms) => Promise.race([promise, sleep(ms).then(() => { throw new Error('hung'); })]);

const binDir = path.join(work, 'bin'); fs.mkdirSync(binDir, { recursive: true });
const oldBin = path.join(binDir, 'chrome-use-old'), newBin = path.join(binDir, 'chrome-use-new');
fs.copyFileSync(process.env.OLD_BIN, oldBin); fs.copyFileSync(process.env.NEW_BIN, newBin);
fs.chmodSync(oldBin, 0o755); fs.chmodSync(newBin, 0o755);

const rig = await startRig({ dir, work, hostBin: oldBin, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8951, cdpPort: 9412 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const A = rig.cdp, b = rig.browser;
const watched = new Set();
b.listeners.push(m => {
  if (!watched.has(m.sessionId)) return;
  const where = m.sessionId === rig.sw.sid ? 'worker' : 'panel';
  if (m.method === 'Runtime.exceptionThrown') (where === 'worker' ? evidence.workerExceptions : evidence.exceptions).push(m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
  if (m.method === 'Runtime.consoleAPICalled' && ['error', 'warning'].includes(m.params.type)) evidence.console.push({ where, type: m.params.type, args: m.params.args.map(a => a.value ?? a.description) });
  if (m.method === 'Log.entryAdded') evidence.console.push({ where, log: m.params.entry.level, text: m.params.entry.text });
});
const watch = async sid => { watched.add(sid); await b.send('Runtime.enable', {}, sid); await b.send('Log.enable', {}, sid); };
await watch(rig.sw.sid);

async function openPanel(label) {
  await rig.ev(`chrome.windows.create({ url: chrome.runtime.getURL('popup.html'), focused: true })`);
  const opener = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => t.url.endsWith('/popup.html') && t.type === 'page'), { label: 'opener page' });
  const osid = await A.attach(opener.targetId);
  await waitFor(() => A.evaluate(osid, 'document.readyState === "complete"'), { label: 'opener load' });
  await A.evaluate(osid, `(() => { const b = document.createElement('button'); b.style = 'position:fixed;left:0;top:0;width:200px;height:80px;z-index:2147483647'; b.onclick = () => chrome.windows.getCurrent().then(w => chrome.sidePanel.open({ windowId: w.id })); document.body.append(b); return true; })()`);
  await A.click(osid, 50, 40);
  const target = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => /\/sidepanel\.html(\?|$)/.test(t.url)), { timeout: 15000, label: 'side panel' });
  const sid = await A.attach(target.targetId);
  await watch(sid);
  await b.send('Page.enable', {}, sid);
  await b.send('Page.reload', {}, sid);
  await sleep(1500);
  const run = expression => within(A.evaluate(sid, expression), 8000);
  await waitFor(() => run(`document.readyState === 'complete' && !/Checking/.test(document.getElementById('connection').textContent)`), { timeout: 30000, label: `${label} panel connection state` }).catch(() => {});
  await sleep(5000);
  return { sid, run };
}
const state = p => p.run(`({ connection: document.getElementById('connection').textContent, ok: document.getElementById('connection').classList.contains('ok'), notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').textContent, agentsView: !document.getElementById('agentsView').hidden, chatView: !document.getElementById('chatView').hidden, agentList: document.getElementById('agentList').innerText.slice(0, 300), skillsTab: !document.getElementById('skillsSettingsTab').hidden, schedulesTab: !document.getElementById('schedulesSettingsTab').hidden, conversation: document.getElementById('conversation').value, phase: document.getElementById('phase').textContent, transcript: document.getElementById('transcript').innerText.slice(-300) })`).catch(e => ({ hung: String(e.message) }));
const shot = async (p, name) => { try { const s = await within(b.send('Page.captureScreenshot', { format: 'png' }, p.sid), 10000); fs.writeFileSync(path.join(out, `${name}.png`), Buffer.from(s.data, 'base64')); } catch (e) { step(`screenshot ${name} failed`, { error: String(e.message) }); } };
async function sendFrom(p, scenarioName) {
  await p.run(`(() => { const s = document.getElementById('agent'); s.value = 'codex'; s.dispatchEvent(new Event('change')); document.getElementById('executable').value = ${JSON.stringify(rig.backend('codex'))}; document.getElementById('message').value = 'run scenario ${scenarioName}'; document.getElementById('composer').requestSubmit(); return true; })()`);
}
try {
  fs.writeFileSync(path.join(rig.accept, 'scenarios', 'c58.json'), JSON.stringify({ steps: [{ say: 'Hello from the fixture.' }] }));
  fs.writeFileSync(path.join(rig.accept, 'scenarios', 'c58busy.json'), JSON.stringify({ steps: [{ say: 'Working on it.' }, { sleep: 900000 }] }));

  // 1. Use the panel with the old build.
  const p1 = await openPanel('old');
  step('old build panel', await state(p1));
  await sendFrom(p1, 'c58');
  const idle = await waitFor(() => p1.run(`document.getElementById('conversation').value`), { timeout: 30000, label: 'idle conversation' });
  await waitFor(() => p1.run(`document.getElementById('transcript').innerText.includes('Hello from the fixture.')`), { timeout: 60000, label: 'idle reply' });
  let running = null;
  if (busy) {
    await p1.run(`document.getElementById('newChat').click(), true`);
    await sleep(500);
    await sendFrom(p1, 'c58busy');
    running = await waitFor(async () => { const v = await p1.run(`document.getElementById('conversation').value`); return v && v !== idle && v; }, { timeout: 30000, label: 'busy conversation' });
    await waitFor(() => p1.run(`document.getElementById('transcript').innerText.includes('Working on it.')`), { timeout: 60000, label: 'busy reply' });
  }
  step('old build used', { idle, running, state: await state(p1) });
  await shot(p1, '1-old');

  // 2. The upgrade: the native host now runs the new build; the old
  // service keeps running.
  const oldService = JSON.parse(fs.readFileSync(rig.serviceJson, 'utf8')).pid;
  const nm = path.join(work, 'nm-host.sh');
  fs.writeFileSync(nm, fs.readFileSync(nm, 'utf8').replace(oldBin, newBin));
  spawnSync('pkill', ['-f', `${oldBin} __nm-host`]);
  await waitFor(() => spawnSync('pgrep', ['-f', `${newBin} __nm-host`]).status === 0, { timeout: 30000, label: 'new native host' }).catch(async () => {
    step('host did not reconnect by itself; reloading the extension');
    await rig.ev('chrome.runtime.reload()').catch(() => {});
    await sleep(2000);
    rig.sw = await A.worker(); rig.ev = expr => A.evaluate(rig.sw.sid, expr); await watch(rig.sw.sid);
    await waitFor(() => spawnSync('pgrep', ['-f', `${newBin} __nm-host`]).status === 0, { timeout: 30000, label: 'new native host after reload' });
  });
  await sleep(3000);
  step('switched', { oldServiceAlive: (() => { try { process.kill(oldService, 0); return true; } catch { return false; } })(), status: await within(rig.ev(`new Promise(r => chrome.runtime.sendMessage({ type: 'ab-status' }, r))`), 8000).catch(e => String(e.message)) });
  for (const t of (await b.send('Target.getTargets')).targetInfos.filter(t => /\/sidepanel\.html/.test(t.url))) await b.send('Target.closeTarget', { targetId: t.targetId }).catch(() => {});

  // 3. The panel after the upgrade.
  const p2 = await openPanel('new');
  const started = await state(p2);
  step('new build panel', { ...started, exceptions: evidence.exceptions, workerExceptions: evidence.workerExceptions });
  await shot(p2, '2-new');
  check(!started.hung, 'the panel is responsive');
  check(evidence.exceptions.length === 0, 'no panel exception');
  await p2.run(`document.getElementById('agentsTab').click(), true`).catch(() => {});
  await sleep(5000);
  const agents = await state(p2);
  step('agents & models', agents);
  await shot(p2, '3-agents');
  check(agents.agentsView && /codex/i.test(agents.agentList || ''), 'Agents & models lists the local agents');
  check(agents.skillsTab && agents.schedulesTab, 'the Skills and Schedules tabs show');
  await p2.run(`document.getElementById('chatTab').click(), true`).catch(() => {});
  await sleep(1000);
  const back = await state(p2);
  step('back', back);
  check(back.chatView && !back.agentsView, '"← Back" returns to the chat');
  await shot(p2, '4-back');
} catch (e) {
  failures.push(String(e.stack || e));
  console.log('[upgrade] ERROR', e.stack || e);
} finally {
  evidence.failures = failures;
  save();
  for (const f of ['proxy.jsonl', 'service.log']) if (fs.existsSync(path.join(work, f))) fs.copyFileSync(path.join(work, f), path.join(out, f));
  await cleanup(rig);
  const labels = spawnSync('sh', ['-c', `launchctl list | awk '/com\\.chrome-use\\.panel-service\\./ {print $3}'`], { encoding: 'utf8' }).stdout.split('\n').filter(Boolean);
  for (const label of labels) {
    const info = spawnSync('launchctl', ['print', `gui/${process.getuid()}/${label}`], { encoding: 'utf8' }).stdout;
    if (info.includes(work)) spawnSync('launchctl', ['bootout', `gui/${process.getuid()}/${label}`]);
  }
  console.log(`[upgrade] ${failures.length ? 'FAILED' : 'PASSED'}; evidence in ${out}`);
  process.exit(failures.length ? 1 : 0);
}
