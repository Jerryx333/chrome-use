// Live check that messages sent while a turn runs stay in the queue area
// (build host only; one browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node queue-live.mjs
// Through the real side panel and the Codex fixture backend:
//   1. a turn that waits for a signal is running;
//   2. three messages sent meanwhile show as three queue rows and nowhere in
//      the transcript, with no "Checks not recorded" under them;
//   3. removing the third from the queue leaves no trace in the transcript;
//   4. released, the first queued message becomes the active turn and leaves
//      the queue; the second follows; the removed one never runs.
// Evidence and screenshots go to $OUT (default <work>/out).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/ql-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [], exceptions: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[queue] ${name}`, JSON.stringify(data).slice(0, 800)); };
const failures = [];
const check = (ok, message) => { console.log(`[queue] ${ok ? 'PASS' : 'FAIL'} ${message}`); if (!ok) failures.push(message); };
const texts = ['david 不要，删掉', 'karlwong 也不行 gavin 可以', 'robot 给 admin 权限'];

const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8951, cdpPort: 9412 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const A = rig.cdp, b = rig.browser;
const shot = async (sid, name) => { const s = await b.send('Page.captureScreenshot', { format: 'png' }, sid); fs.writeFileSync(path.join(out, `${name}.png`), Buffer.from(s.data, 'base64')); };
try {
  await rig.ev(`chrome.windows.create({ url: chrome.runtime.getURL('popup.html'), focused: true })`);
  const opener = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => t.url.endsWith('/popup.html') && t.type === 'page'), { label: 'opener page' });
  const osid = await A.attach(opener.targetId);
  await waitFor(() => A.evaluate(osid, 'document.readyState === "complete"'), { label: 'opener load' });
  await A.evaluate(osid, `(() => { const b = document.createElement('button'); b.style = 'position:fixed;left:0;top:0;width:200px;height:80px;z-index:2147483647'; b.onclick = () => chrome.windows.getCurrent().then(w => chrome.sidePanel.open({ windowId: w.id })); document.body.append(b); return true; })()`);
  await A.click(osid, 50, 40);
  const target = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => /\/sidepanel\.html(\?|$)/.test(t.url)), { timeout: 15000, label: 'side panel' });
  const sid = await A.attach(target.targetId);
  b.listeners.push(m => { if (m.sessionId === sid && m.method === 'Runtime.exceptionThrown') evidence.exceptions.push(m.params.exceptionDetails?.exception?.description || m.params.exceptionDetails?.text); });
  await b.send('Runtime.enable', {}, sid);
  await b.send('Page.enable', {}, sid);
  await b.send('Emulation.setDeviceMetricsOverride', { width: 420, height: 900, deviceScaleFactor: 1, mobile: false }, sid).catch(() => {});
  const run = expression => A.evaluate(sid, expression);
  await waitFor(() => run(`document.readyState === 'complete' && document.getElementById('connection').classList.contains('ok')`), { timeout: 30000, label: 'panel connected' });
  await sleep(2000);

  const scenario = (name, spec) => fs.writeFileSync(path.join(rig.accept, 'scenarios', name + '.json'), JSON.stringify(spec));
  scenario('qbusy', { steps: [{ say: 'Working on the collaborators.' }, { waitFor: 'release', timeoutMs: 180000 }, { say: 'First task done.' }] });
  scenario('qnext', { steps: [{ say: 'Handling the follow-up.' }, { sleep: 6000 }, { say: 'Follow-up done.' }] });
  // Turn-view state as the user sees it.
  const state = () => run(`(() => {
    const cards = [...document.querySelectorAll('#transcript .turn-card')].filter(c => !c.hidden && c.offsetParent !== null);
    return { phase: document.getElementById('phase').textContent, queue: document.getElementById('queue').textContent,
      queueRows: [...document.querySelectorAll('#queuedMessages .queued-message')].map(n => n.querySelector('summary').textContent),
      cards: cards.map(c => ({ user: c.querySelector('.message-row.user .content')?.innerText || '', text: c.innerText.slice(0, 400) })),
      transcript: document.getElementById('transcript').innerText };
  })()`);
  const submit = async text => {
    await run(`(() => { document.getElementById('message').value = ${JSON.stringify(text)}; document.getElementById('composer').requestSubmit(); return true; })()`);
    await waitFor(() => run(`document.getElementById('message').value === ''`), { timeout: 20000, label: 'send confirmed' });
  };

  await run(`(() => { const s = document.getElementById('agent'); s.value = 'codex'; s.dispatchEvent(new Event('change')); document.getElementById('executable').value = ${JSON.stringify(rig.backend('codex'))}; return true; })()`);
  await submit('Clean up the collaborators. run scenario qbusy');
  const conversation = await waitFor(() => run(`document.getElementById('conversation').value`), { timeout: 30000, label: 'conversation created' });
  await waitFor(() => run(`document.getElementById('transcript').innerText.includes('Working on the collaborators.')`), { timeout: 90000, interval: 500, label: 'busy turn running' });
  step('busy turn running', { conversation, ...(await state()) });

  for (const text of texts) await submit(`${text} run scenario qnext`);
  await waitFor(async () => (await state()).queueRows.length === 3, { timeout: 15000, label: 'three queue rows' });
  await sleep(1500);
  const queued = await state();
  step('three queued', queued);
  await shot(sid, '1-three-queued');
  check(queued.queueRows.length === 3, 'three queue rows');
  check(queued.cards.length === 1, `one turn card in the transcript (saw ${queued.cards.length})`);
  check(texts.every(text => !queued.transcript.includes(text)), 'no queued text in the transcript');
  check(!/Checks not recorded|未记录检查/.test(queued.transcript), 'no "Checks not recorded" badge');

  // Remove the third from the queue.
  await run(`(() => { const row = document.querySelectorAll('#queuedMessages .queued-message')[2]; row.open = true; row.querySelectorAll('.actions button')[1].click(); return true; })()`);
  await waitFor(async () => (await state()).queueRows.length === 2, { timeout: 15000, label: 'queue row removed' });
  await sleep(1500);
  const removed = await state();
  step('third removed', removed);
  await shot(sid, '2-third-removed');
  check(removed.cards.length === 1 && !removed.transcript.includes(texts[2]), 'the removed message does not appear in the transcript');

  // Release the busy turn: the first queued message starts.
  fs.mkdirSync(path.join(rig.accept, 'signals'), { recursive: true });
  fs.writeFileSync(path.join(rig.accept, 'signals', `${conversation}.release`), String(Date.now()));
  await waitFor(() => run(`document.getElementById('transcript').innerText.includes('Handling the follow-up.')`), { timeout: 60000, interval: 300, label: 'first queued turn running' });
  const started = await state();
  step('first queued turn started', started);
  await shot(sid, '3-first-started');
  check(started.queueRows.length === 1, `one queue row left (saw ${started.queueRows.length})`);
  check(started.cards.length === 2 && started.cards[1].user.includes(texts[0]), 'the first queued message is the second turn card');
  check(!started.transcript.includes(texts[1]), 'the still-queued message stays out of the transcript');

  await waitFor(async () => { const s = await state(); return s.cards.length === 3 && s.transcript.split('Follow-up done.').length === 3; }, { timeout: 90000, interval: 500, label: 'second queued turn done' });
  const done = await state();
  step('all done', done);
  await shot(sid, '4-all-done');
  check(done.queueRows.length === 0, 'queue empty');
  check(done.cards[2].user.includes(texts[1]) && !done.transcript.includes(texts[2]), 'the second message ran; the removed one never did');
  check(evidence.exceptions.length === 0, 'no exception in the panel');
} catch (e) {
  failures.push(String(e.stack || e));
  console.log('[queue] ERROR', e.stack || e);
} finally {
  evidence.failures = failures;
  fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
  await cleanup(rig);
  console.log(`[queue] ${failures.length ? 'FAILED' : 'PASSED'}; evidence in ${out}`);
  process.exit(failures.length ? 1 : 0);
}
