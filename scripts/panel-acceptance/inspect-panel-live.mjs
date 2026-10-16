// Live end-to-end check of Inspect & Ask → side-panel chat (ab-connect
// 0.5.69) through the real picker, input frame and side panel (build host
// only; one browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension with host_permissions> node inspect-panel-live.mjs
// The extension copy needs host_permissions in place of the activeTab grant
// the keyboard shortcut gives (inspect-panel-live.sh adds it). The agent is
// the Codex fixture backend calling the real `chrome-use mcp`. Steps:
//   1. the picker starts on the fixture page; shift+clicks pick a plain
//      button, one in an open shadow root and one in a same-origin iframe;
//      the input frame shows the side-panel destination by default;
//   2. a question is typed in the frame and "Add to chat" is clicked (a
//      trusted click): the tab's own side panel opens, shows three chips and
//      the question in its composer, nothing is sent, and the page keeps no
//      picker element, probe attribute or global;
//   3. a chip is removed and picked again, so removal is seen to work;
//   4. send: the backend's turn text holds the question and every element's
//      selector, place and locate expression; its eval of those expressions
//      in the lent tab finds all three elements; the screenshots arrive as
//      attachments; the chips leave the composer and the agent's reply shows;
//   5. with the panel open and a conversation running, a new pick adds a
//      chip without sending; navigating the tab marks it stale;
//   6. the terminal destination is a click away and is remembered for the
//      site (shown on the next picker), and switching back restores the panel.
// Evidence and screenshots go to $OUT (default <work>/out).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup, openTab, activate } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/ip-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[inspect-panel] ${name}`, JSON.stringify(data).slice(0, 600)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok) { save(); throw new Error(message); } };

let failState = async () => {};
const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8946, cdpPort: 9406 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const A = rig.cdp, b = rig.browser;
const scenario = (name, spec) => fs.writeFileSync(path.join(rig.accept, 'scenarios', name + '.json'), JSON.stringify(spec, null, 1));
const backendLog = () => fs.existsSync(path.join(rig.accept, 'backends.jsonl')) ? fs.readFileSync(path.join(rig.accept, 'backends.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line)) : [];
const shotOf = async (sid, name) => { try { const s = await b.send('Page.captureScreenshot', { format: 'png' }, sid); fs.writeFileSync(path.join(out, `${name}.png`), Buffer.from(s.data, 'base64')); } catch {} };
const panelTargets = async () => (await b.send('Target.getTargets')).targetInfos.filter(t => /\/sidepanel\.html(\?|$)/.test(t.url));
const SHIFT = 8;
const chipsKey = tabId => `ab_inspect_tab_${tabId}`;
const chipRecord = async tabId => (await rig.ev(`chrome.storage.session.get(${JSON.stringify(chipsKey(tabId))})`))[chipsKey(tabId)] || null;

// Points in the top window for the three fixture elements.
const points = {
  save: `(() => { const r = document.getElementById('save').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`,
  inner: `(() => { const r = document.querySelector('x-card').shadowRoot.querySelector('button.inner').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`,
  pay: `(() => { const f = document.getElementById('pay'), o = f.getBoundingClientRect(), r = f.contentDocument.getElementById('pay-now').getBoundingClientRect(); return { x: o.left + f.clientLeft + r.left + r.width / 2, y: o.top + f.clientTop + r.top + r.height / 2 }; })()`,
};
async function pick(page, name) {
  const p = await A.evaluate(page.sid, points[name]);
  // Two moves: the pointer enters (a frame starts reporting), then settles.
  await b.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: p.x - 3, y: p.y - 2, modifiers: SHIFT }, page.sid);
  await sleep(120);
  await A.click(page.sid, p.x, p.y, SHIFT);
  await sleep(250);
}
async function startPicker(tab) {
  const started = await rig.ev(`globalThis.__chromeUseAcceptance.startInspect(${tab.tabId}).then(r => JSON.stringify(r), e => 'ERR ' + e.message)`);
  check(started === '{"started":true}', 'startInspect: ' + started);
}
async function pageSession(tab) {
  const sid = await A.attach(tab.targetId);
  await b.send('Runtime.enable', {}, sid);
  return { sid, targetId: tab.targetId };
}
const frameText = frame => A.evaluate(frame.sid, 'document.getElementById("app").innerText');
async function openPanel(tab) {
  const target = await waitFor(async () => (await panelTargets()).find(t => t.url.endsWith(`?tab=${tab.tabId}`)), { timeout: 15000, label: `panel of tab ${tab.tabId}` });
  const sid = await A.attach(target.targetId);
  await waitFor(() => A.evaluate(sid, `document.getElementById('connection').classList.contains('ok')`), { timeout: 30000, label: 'panel connected' });
  return { sid, run: expression => A.evaluate(sid, expression) };
}
const chipView = panel => panel.run(`({ hidden: document.getElementById('inspectChips').hidden, chips: [...document.getElementById('inspectChips').children].map(c => ({ text: c.textContent, stale: c.classList.contains('stale'), title: c.title })), message: document.getElementById('message').value, selected: document.getElementById('conversation').value })`);
// What a page could see of the picker after it closed.
const pageTraces = page => A.evaluate(page.sid, `({
  hosts: [...document.documentElement.children].filter(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName)).length,
  probeAttrs: [...document.querySelectorAll('*'), ...document.querySelector('x-card').shadowRoot.querySelectorAll('*'), ...document.getElementById('pay').contentDocument.querySelectorAll('*')].flatMap(e => [...e.attributes].map(a => a.name)).filter(n => /^data-[a-z]{12}$/.test(n)),
  globals: Object.getOwnPropertyNames(window).filter(k => /inspect|chromeuse|chrome_use/i.test(k)),
  frameGlobals: Object.getOwnPropertyNames(document.getElementById('pay').contentWindow).filter(k => /inspect|chromeuse|chrome_use/i.test(k)),
})`);

try {
  const X = await openTab(rig, rig.site + '/inspect');
  await activate(rig, X);
  const page = await pageSession(X);
  await waitFor(() => A.evaluate(page.sid, `!!document.querySelector('x-card')?.shadowRoot && document.getElementById('pay').contentDocument?.readyState === 'complete'`), { label: 'fixture ready' });

  // 1. Pick three elements.
  await startPicker(X);
  await waitFor(() => A.evaluate(page.sid, '[...document.documentElement.children].some(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName))'), { label: 'picker host element' });
  await pick(page, 'save');
  const frame = await A.realFrame();
  failState = async () => { await shotOf(page.sid, 'fail-page'); evidence.failure = { frame: await frameText(frame).catch(e => String(e)) }; };
  await pick(page, 'inner');
  await pick(page, 'pay');
  const cards = await waitFor(async () => { const n = await A.evaluate(frame.sid, `document.querySelectorAll('.card').length`); return n === 3 && n; }, { timeout: 15000, label: 'three cards' });
  const thumbs = await waitFor(() => A.evaluate(frame.sid, `document.querySelectorAll('img.thumb').length === 3 && 3`), { timeout: 15000, label: 'three screenshots' }).catch(() => 0);
  const frameState = { cards, thumbs, text: await frameText(frame), destination: await A.evaluate(frame.sid, `[...document.querySelectorAll('.seg')].map(s => s.getAttribute('aria-pressed') + ':' + s.textContent)`) };
  await shotOf(page.sid, '1-picked');
  step('three elements picked', frameState);
  check(frameState.destination[0].startsWith('true:'), 'the side-panel chat is the default destination');
  check(/Inner action/.test(frameState.text) && /Pay now/.test(frameState.text) && /Save order/.test(frameState.text), 'the shadow-root and iframe elements were picked, not their hosts');

  // 2. Question + "Add to chat": the tab's panel opens with the chips.
  await A.clickInFrame(page.sid, frame.sid, `document.getElementById('question')`);
  await b.send('Input.insertText', { text: 'Why is Save gray?' }, page.sid);
  await A.clickInFrame(page.sid, frame.sid, `document.getElementById('toPanel')`);
  const panel = await openPanel(X);
  const view = await waitFor(async () => { const v = await chipView(panel); return v.chips.length === 3 && v; }, { timeout: 15000, label: 'three chips' });
  const record = await chipRecord(X.tabId);
  await waitFor(async () => (await pageTraces(page)).hosts === 0, { timeout: 10000, label: 'picker closed' });
  const traces = await pageTraces(page);
  await shotOf(panel.sid, '2-chips');
  step('added to the chat', { view, record: record.items.map(c => ({ selector: c.selector, frames: c.frames, shadowHosts: c.shadowHosts, image: !!c.image, documentId: !!c.documentId })), traces, backendTurns: backendLog().filter(e => e.event === 'turn').length });
  check(!view.hidden && view.message === 'Why is Save gray?', 'the question waits in the composer');
  check(view.selected === '', 'nothing was sent: no conversation yet');
  check(backendLog().filter(e => e.event === 'turn').length === 0, 'no agent turn ran');
  check(record.question === '', 'the question was taken out of storage once shown');
  check(record.items.some(c => c.shadowHosts.length === 1) && record.items.some(c => c.frames.length === 1), 'shadow and frame places were kept');
  check(traces.hosts === 0 && traces.probeAttrs.length === 0 && traces.globals.length === 0 && traces.frameGlobals.length === 0, 'the page keeps no picker element, probe attribute or global');

  // 3. Remove the iframe chip, then pick it again on a second picker.
  const payIndex = record.items.findIndex(c => c.frames.length);
  await panel.run(`document.getElementById('inspectChips').children[${payIndex}].querySelector('button').click()`);
  await waitFor(async () => (await chipView(panel)).chips.length === 2, { label: 'chip removed' });
  check((await chipRecord(X.tabId)).items.length === 2, 'removal reached storage');
  await activate(rig, X);
  await startPicker(X);
  await waitFor(() => A.evaluate(page.sid, '[...document.documentElement.children].some(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName))'), { label: 'second picker' });
  await pick(page, 'pay');
  const frame2 = await A.realFrame();
  await waitFor(() => A.evaluate(frame2.sid, `document.querySelectorAll('.card').length === 1`), { label: 'one card' });
  await waitFor(() => A.evaluate(frame2.sid, `document.querySelectorAll('img.thumb').length === 1`), { timeout: 15000, label: 'screenshot' }).catch(() => {});
  await A.clickInFrame(page.sid, frame2.sid, `document.getElementById('toPanel')`);
  const readded = await waitFor(async () => { const v = await chipView(panel); return v.chips.length === 3 && v; }, { timeout: 15000, label: 'chip added to the open panel' });
  step('removed and added again', { chips: readded.chips.map(c => c.text) });

  // 4. Send. The scenario evaluates every chip's locate expression (or
  // selector) in the lent tab, the way an agent would use them.
  const items = (await chipRecord(X.tabId)).items;
  const exprs = items.map(c => {
    let e = 'document';
    for (const f of c.frames) { for (const h of f.hosts) e += `.querySelector(${JSON.stringify(h)}).shadowRoot`; e += `.querySelector(${JSON.stringify(f.selector)}).contentDocument`; }
    for (const h of c.shadowHosts) e += `.querySelector(${JSON.stringify(h)}).shadowRoot`;
    return `${e}.querySelector(${JSON.stringify(c.selector)})`;
  });
  scenario('ip-act', { steps: [{ tool: 'chrome_use_eval', args: { script: `[${exprs.join(', ')}].map(e => e ? e.textContent.trim() : 'MISSING').join('|')` } }, { say: 'Found all three.' }] });
  await panel.run(`(() => { const s = document.getElementById('agent'); s.value = 'codex'; s.dispatchEvent(new Event('change')); document.getElementById('executable').value = ${JSON.stringify(rig.backend('codex'))}; const m = document.getElementById('message'); m.value = m.value + ' run scenario ip-act'; document.getElementById('composer').requestSubmit(); return true; })()`);
  const conversationId = await waitFor(() => panel.run(`document.getElementById('conversation').value`), { timeout: 30000, label: 'conversation created' });
  const ended = await waitFor(async () => { const r = await rig.service.journal(conversationId); return r.events.some(e => e.kind === 'turnEnded') && r.events; }, { timeout: 120000, interval: 500, label: 'turn ended' });
  const turn = backendLog().find(e => e.event === 'turn' && e.scenario === 'ip-act');
  const tool = backendLog().find(e => e.event === 'tool' && e.scenario === 'ip-act');
  const userEvent = ended.find(e => Array.isArray(e.data?.attachments) && e.data.attachments.length);
  const attachedImages = backendLog().filter(e => e.event === 'in' && /localImage|"image"/.test(e.m || '')).length;
  const afterSend = await chipView(panel);
  const folded = await waitFor(() => panel.run(`(() => { const d = document.querySelector('#transcript details.inspect-context'); return d && { summary: d.querySelector('summary').textContent, open: d.open, words: d.parentElement.firstChild.textContent }; })()`), { timeout: 10000, label: 'folded element block' }).catch(() => null);
  const reply = await waitFor(() => panel.run(`document.getElementById('transcript').innerText.includes('Found all three.') && true`), { timeout: 20000, label: 'reply shown' }).catch(() => false);
  await shotOf(panel.sid, '4-sent');
  step('sent', { text: turn?.text?.slice(0, 6000), tool: tool && { isError: tool.isError, result: String(tool.result).slice(0, 400) }, attachments: userEvent?.data.attachments?.length ?? null, attachmentEvent: userEvent?.kind ?? null, kinds: [...new Set(ended.map(e => e.kind))], attachedImages, afterSend, reply, folded });
  check(folded && /3/.test(folded.summary) && !folded.open && folded.words.startsWith('Why is Save gray?'), 'the transcript shows the words and folds the element block');
  check(!/data-[a-z]{12}=/.test(turn?.text || ''), 'no probe attribute in the HTML the agent got');
  check(turn && turn.text.includes('Why is Save gray?'), 'the agent got the question');
  for (const c of items) check(turn.text.includes(c.selector), `the agent got selector ${c.selector}`);
  check(turn.text.includes(`chrome-tab:${X.tabId}`), 'the agent was told which tab');
  check(/shadow root of `x-card#card`|shadow root of `x-card`/.test(turn.text) && /iframe `iframe#pay`/.test(turn.text), 'the agent got the shadow and frame places');
  check(/locate \(JavaScript/.test(turn.text), 'the agent got locate expressions');
  check(tool && !tool.isError && String(tool.result).includes('Save order') && String(tool.result).includes('Inner action') && String(tool.result).includes('Pay now') && !String(tool.result).includes('MISSING'), 'every element resolved in the lent tab');
  check(afterSend.hidden && afterSend.chips.length === 0, 'sent chips leave the composer');
  check(reply, 'the reply shows in the panel');
  check(userEvent?.data.attachments.length === 3, 'the three screenshots went with the message');
  evidence.attachments = userEvent?.data.attachments?.length ?? null;

  // 5. Panel open, conversation there: a new pick adds a chip, sends nothing;
  // a navigation marks it stale.
  const turnsBefore = backendLog().filter(e => e.event === 'turn').length;
  await activate(rig, X);
  await startPicker(X);
  await waitFor(() => A.evaluate(page.sid, '[...document.documentElement.children].some(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName))'), { label: 'third picker' });
  await pick(page, 'save');
  const frame3 = await A.realFrame();
  await waitFor(() => A.evaluate(frame3.sid, `document.querySelectorAll('.card').length === 1`), { label: 'one card' });
  await A.clickInFrame(page.sid, frame3.sid, `document.getElementById('toPanel')`);
  const waiting = await waitFor(async () => { const v = await chipView(panel); return v.chips.length === 1 && v; }, { timeout: 15000, label: 'chip in the running conversation' });
  await sleep(1500);
  check(backendLog().filter(e => e.event === 'turn').length === turnsBefore, 'adding a chip sent nothing');
  check(waiting.selected === conversationId, 'the conversation stays selected');
  await rig.ev(`chrome.tabs.update(${X.tabId}, { url: ${JSON.stringify(rig.site + '/a')} })`);
  const stale = await waitFor(async () => { const v = await chipView(panel); return v.chips[0]?.stale && v; }, { timeout: 15000, label: 'chip marked stale' });
  await shotOf(panel.sid, '5-stale');
  step('stale after navigation', { stale });

  // 6. The terminal destination, remembered per site.
  await rig.ev(`chrome.tabs.update(${X.tabId}, { url: ${JSON.stringify(rig.site + '/inspect')} })`);
  await waitFor(async () => (await rig.ev(`chrome.tabs.get(${X.tabId})`)).status === 'complete', { label: 'back on the fixture' });
  const page2 = await pageSession(X);
  await waitFor(() => A.evaluate(page2.sid, `document.readyState === 'complete' && !!document.getElementById('save')`), { label: 'fixture loaded' });
  await startPicker(X);
  await waitFor(() => A.evaluate(page2.sid, '[...document.documentElement.children].some(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName))'), { label: 'fourth picker' });
  await pick(page2, 'save');
  const frame4 = await A.realFrame();
  await A.clickInFrame(page2.sid, frame4.sid, `document.querySelectorAll('.seg')[1]`);
  const terminal = await waitFor(async () => { const t = await frameText(frame4); return /ocs|Inspect & Ask lets you/.test(t) && t; }, { timeout: 10000, label: 'terminal view' });
  const stored = (await rig.ev(`chrome.storage.local.get('inspectDestinations')`)).inspectDestinations;
  await shotOf(page2.sid, '6-terminal');
  // A new picker replaces this one, as pressing the shortcut again does.
  await startPicker(X);
  const frame5 = await waitFor(async () => { try { return await A.realFrame(); } catch { return null; } }, { timeout: 15000, label: 'terminal frame opens at once (enable step)' });
  const remembered = await A.evaluate(frame5.sid, `[...document.querySelectorAll('.seg')].map(s => s.getAttribute('aria-pressed'))`);
  await A.clickInFrame(page2.sid, frame5.sid, `document.querySelectorAll('.seg')[0]`);
  await waitFor(async () => !(await rig.ev(`chrome.storage.local.get('inspectDestinations')`)).inspectDestinations?.[rig.site], { label: 'panel restored' });
  step('terminal destination', { terminal: terminal.slice(0, 300), stored, remembered });
  check(stored?.[rig.site] === 'terminal', 'the terminal choice is stored for the site');
  check(remembered[1] === 'true', 'the next picker shows the terminal choice');

  evidence.passed = true;
  console.log('[inspect-panel] PASS');
} catch (failure) {
  evidence.error = String(failure.stack || failure.message || failure);
  await failState().catch(() => {});
  console.log('[inspect-panel] FAIL', evidence.error);
} finally {
  save();
  cleanup(rig);
  console.log(`evidence: ${out}`);
  // The rig's proxy and service sockets keep the event loop alive.
  setTimeout(() => process.exit(evidence.passed ? 0 : 1), 1000).unref();
}
