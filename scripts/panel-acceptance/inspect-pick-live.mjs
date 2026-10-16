// Live check and screenshots of the side panel's Pick element button
// (ab-connect 0.5.68), build host only, one browser run per host at a time:
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension with host_permissions> THEME=light|dark node inspect-pick-live.mjs
// The panel is laid out 380 px wide; THEME sets prefers-color-scheme on the
// panel, the page and the input frame. Steps (each with screenshots in $OUT):
//   1. the tab's panel shows the Pick button in the composer toolbar;
//   2. a trusted click turns it active, with the hint line, and the page
//      shows the picker's hint bar;
//   3. hovering an element on the page shows the box and its name and size;
//   4. a click flashes "Added to chat" on the page, ends picking, and the chip
//      lands in the composer; the time from the click to the chip is
//      measured (must be under 300 ms);
//   5. the screenshot and source fill in (placeholder dot goes), the details
//      card shows on focus;
//   6. Shift picks several in one go (shadow root, iframe, a long label that
//      must truncate); Esc in the panel ends picking; no horizontal overflow;
//   7. Delete removes a chip; send: the agent gets every selector, the
//      screenshots go with the message, the context is folded;
//   8. on a chrome:// page the button is disabled;
//   9. the shortcut's input frame (other entry point) in the same theme.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup, openTab, activate } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/ip-');
const out = process.env.OUT || path.join(work, 'out');
const theme = process.env.THEME === 'dark' ? 'dark' : 'light';
fs.mkdirSync(out, { recursive: true });
const evidence = { work, theme, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[inspect-pick] ${name}`, JSON.stringify(data).slice(0, 500)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok) { save(); throw new Error(message); } };

const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8947, cdpPort: 9407 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const A = rig.cdp, b = rig.browser;
const scenario = (name, spec) => fs.writeFileSync(path.join(rig.accept, 'scenarios', name + '.json'), JSON.stringify(spec, null, 1));
const backendLog = () => fs.existsSync(path.join(rig.accept, 'backends.jsonl')) ? fs.readFileSync(path.join(rig.accept, 'backends.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line)) : [];
const shot = async (sid, name) => { try { const s = await b.send('Page.captureScreenshot', { format: 'png' }, sid); fs.writeFileSync(path.join(out, `${name}.png`), Buffer.from(s.data, 'base64')); } catch (e) { step('screenshot failed', { name, error: String(e.message) }); } };
const styled = async sid => { await b.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] }, sid).catch(() => {}); };
const SHIFT = 8;

async function openPanelFor(tab) {
  const win = await rig.ev(`chrome.windows.create({ url: chrome.runtime.getURL('popup.html'), focused: true })`);
  const opener = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => t.url.endsWith('/popup.html') && t.type === 'page'), { label: 'opener page' });
  const sid = await A.attach(opener.targetId);
  await waitFor(() => A.evaluate(sid, 'document.readyState === "complete"'), { label: 'opener load' });
  await A.evaluate(sid, `(() => { const b = document.createElement('button'); b.style = 'position:fixed;left:0;top:0;width:200px;height:80px;z-index:2147483647'; b.onclick = () => { chrome.sidePanel.setOptions({ tabId: ${tab.tabId}, path: 'sidepanel.html?tab=${tab.tabId}', enabled: true }); chrome.sidePanel.open({ tabId: ${tab.tabId} }); }; document.body.append(b); return true; })()`);
  await A.click(sid, 50, 40);
  const target = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => t.url.endsWith(`?tab=${tab.tabId}`)), { timeout: 15000, label: 'panel' });
  await rig.ev(`chrome.windows.remove(${win.id})`);
  const psid = await A.attach(target.targetId);
  await b.send('Emulation.setDeviceMetricsOverride', { width: 380, height: 760, deviceScaleFactor: 2, mobile: false }, psid);
  await styled(psid);
  await waitFor(() => A.evaluate(psid, `document.getElementById('connection').classList.contains('ok')`), { timeout: 30000, label: 'panel connected' });
  return { sid: psid, run: expression => A.evaluate(psid, expression) };
}
async function clickIn(sid, expression) {
  const p = await A.evaluate(sid, `(() => { const el = ${expression}; const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
  await A.click(sid, p.x, p.y);
}
const points = {
  save: `(() => { const r = document.getElementById('save').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`,
  long: `(() => { const r = document.getElementById('long').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`,
  inner: `(() => { const r = document.querySelector('x-card').shadowRoot.querySelector('button.inner').getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`,
  pay: `(() => { const f = document.getElementById('pay'), o = f.getBoundingClientRect(), r = f.contentDocument.getElementById('pay-now').getBoundingClientRect(); return { x: o.left + f.clientLeft + r.left + r.width / 2, y: o.top + f.clientTop + r.top + r.height / 2 }; })()`,
};
const panelView = panel => panel.run(`({
  pressed: document.getElementById('pickElement').getAttribute('aria-pressed'), disabled: document.getElementById('pickElement').disabled, title: document.getElementById('pickElement').title,
  hint: !document.getElementById('pickHint').hidden, chips: [...document.getElementById('inspectChips').children].map(c => ({ text: c.textContent, cls: c.className, label: c.getAttribute('aria-label') })),
  live: document.getElementById('chipLive').textContent, card: !document.getElementById('chipCard').hidden && document.getElementById('chipCard').innerText,
  overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth, message: document.getElementById('message').value, selected: document.getElementById('conversation').value,
})`);
// Records, in the panel's clock, when each chip first appears.
const watchChips = panel => panel.run(`(() => { globalThis.__landed = {}; new MutationObserver(() => { for (const c of document.getElementById('inspectChips').children) { const k = c.getAttribute('aria-label'); if (!(k in globalThis.__landed)) globalThis.__landed[k] = { at: Date.now(), cls: c.className }; } }).observe(document.getElementById('inspectChips'), { childList: true }); return true; })()`);

try {
  const X = await openTab(rig, rig.site + '/inspect');
  await activate(rig, X);
  const page = { sid: await A.attach(X.targetId) };
  await b.send('Runtime.enable', {}, page.sid);
  await styled(page.sid);
  await waitFor(() => A.evaluate(page.sid, `!!document.querySelector('x-card')?.shadowRoot && document.getElementById('pay').contentDocument?.readyState === 'complete'`), { label: 'fixture ready' });

  // 1. The Pick button in the toolbar.
  const panel = await openPanelFor(X);
  await watchChips(panel);
  await sleep(800);
  const idle = await panelView(panel);
  await shot(panel.sid, '01-panel-idle');
  step('panel idle', idle);
  check(idle.pressed === 'false' && !idle.disabled && idle.overflow <= 0, 'the Pick button is ready, nothing overflows');

  // 2. Active.
  await clickIn(panel.sid, `document.getElementById('pickElement')`);
  await waitFor(async () => (await panelView(panel)).pressed === 'true', { label: 'button active' });
  await waitFor(() => A.evaluate(page.sid, '[...document.documentElement.children].some(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName))'), { label: 'picker on the page' });
  await waitFor(async () => (await rig.ev(`chrome.storage.session.get('ab_inspect_pick_${X.tabId}')`))[`ab_inspect_pick_${X.tabId}`], { label: 'picker confirmed' });
  const active = await panelView(panel);
  await shot(panel.sid, '02-panel-picking');
  await shot(page.sid, '02-page-picking');
  step('picking', active);
  check(active.hint && active.pressed === 'true', 'the panel shows picking is on');

  // 3. Hover.
  const ps = await A.evaluate(page.sid, points.save);
  await b.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: ps.x - 4, y: ps.y - 2 }, page.sid);
  await b.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: ps.x, y: ps.y }, page.sid);
  await sleep(200);
  await shot(page.sid, '03-page-hover');

  // 4. Click: flash on the page, chip in the panel, picking ends.
  await b.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: ps.x, y: ps.y, button: 'left', clickCount: 1 }, page.sid);
  const t0 = Date.now();
  await b.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: ps.x, y: ps.y, button: 'left', clickCount: 1 }, page.sid);
  await sleep(120);
  await shot(page.sid, '04-page-flash');
  const landed = await waitFor(async () => { const l = await panel.run('globalThis.__landed'); const k = Object.keys(l)[0]; return k && { label: k, at: l[k].at, cls: l[k].cls }; }, { interval: 20, label: 'chip landed' });
  await shot(panel.sid, '04-panel-chip-landed');
  const ended = await waitFor(async () => { const v = await panelView(panel); return v.pressed === 'false' && !v.hint && v; }, { timeout: 10000, label: 'picking ended' });
  const latency = landed.at - t0;
  step('picked one', { latency, ended });
  evidence.latencyMs = latency;
  check(latency < 300, `the chip appeared ${latency} ms after the click`);
  check(/landed/.test(landed.cls), 'the new chip is highlighted');
  check(ended.live.length > 0, 'the chip was announced');

  // 5. Screenshot and source fill in; details card.
  await waitFor(async () => !(await panel.run(`!!document.querySelector('.inspect-chip-pending')`)), { timeout: 15000, label: 'placeholder gone' });
  await waitFor(async () => !(await A.evaluate(page.sid, '[...document.documentElement.children].some(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName))')), { timeout: 8000, label: 'picker left the page' });
  await panel.run(`document.querySelector('.inspect-chip').focus()`);
  await sleep(150);
  const card = await panelView(panel);
  await shot(panel.sid, '05-chip-card');
  step('details card', { card: card.card });
  check(card.card && /button#save/.test(card.card), 'the card shows the selector');
  check(await panel.run(`!!document.querySelector('#chipCard img.chip-card-shot')`), 'the card shows the screenshot');

  // 6. Several with Shift; Esc in the panel ends it.
  await panel.run(`document.getElementById('message').focus()`);
  await clickIn(panel.sid, `document.getElementById('pickElement')`);
  await waitFor(async () => (await rig.ev(`chrome.storage.session.get('ab_inspect_pick_${X.tabId}')`))[`ab_inspect_pick_${X.tabId}`], { label: 'picker again' });
  for (const name of ['inner', 'pay', 'long']) {
    const p = await A.evaluate(page.sid, points[name]);
    await b.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: p.x - 3, y: p.y - 2, modifiers: SHIFT }, page.sid);
    await sleep(100);
    await A.click(page.sid, p.x, p.y, SHIFT);
    await sleep(300);
  }
  const several = await waitFor(async () => { const v = await panelView(panel); return v.chips.length === 4 && v; }, { timeout: 10000, label: 'four chips' });
  check(several.pressed === 'true', 'Shift keeps picking');
  await shot(page.sid, '06-page-several');
  await A.key(panel.sid, 'Escape', 'Escape', 27);
  const stopped = await waitFor(async () => { const v = await panelView(panel); return v.pressed === 'false' && v; }, { label: 'Esc stopped picking' });
  await waitFor(async () => !(await panel.run(`!!document.querySelector('.inspect-chip-pending')`)), { timeout: 15000, label: 'placeholders gone' }).catch(() => {});
  const longChip = await panel.run(`(() => { const c = [...document.querySelectorAll('.inspect-chip')].find(c => /warehouse|Save this/.test(c.textContent)); const l = c.querySelector('.inspect-chip-label'); return { truncated: l.scrollWidth > l.clientWidth, chipRight: c.getBoundingClientRect().right, width: document.documentElement.clientWidth }; })()`);
  const afterEsc = await panelView(panel);
  await shot(panel.sid, '06-panel-four-chips');
  step('several', { chips: afterEsc.chips.map(c => c.text), longChip, overflow: afterEsc.overflow });
  check(longChip.truncated && longChip.chipRight <= longChip.width && afterEsc.overflow <= 0, 'a long name truncates; no horizontal overflow at 380 px');
  check(await A.evaluate(page.sid, '![...document.documentElement.children].some(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName))'), 'Esc removed the picker from the page');
  void stopped;

  // 7. Delete a chip, send the rest.
  await panel.run(`document.querySelectorAll('.inspect-chip')[3].focus()`);
  await A.key(panel.sid, 'Delete', 'Delete', 46);
  await waitFor(async () => (await panelView(panel)).chips.length === 3, { label: 'Delete removed a chip' });
  await shot(panel.sid, '07-panel-ready-to-send');
  const items = (await rig.ev(`chrome.storage.session.get('ab_inspect_tab_${X.tabId}')`))[`ab_inspect_tab_${X.tabId}`].items;
  scenario('ip-pick', { steps: [{ say: 'Got them.' }] });
  await panel.run(`(() => { const s = document.getElementById('agent'); s.value = 'codex'; s.dispatchEvent(new Event('change')); document.getElementById('executable').value = ${JSON.stringify(rig.backend('codex'))}; document.getElementById('message').value = 'Why is Save gray? run scenario ip-pick'; document.getElementById('composer').requestSubmit(); return true; })()`);
  const conversationId = await waitFor(() => panel.run(`document.getElementById('conversation').value`), { timeout: 30000, label: 'conversation' });
  const events = await waitFor(async () => { const r = await rig.service.journal(conversationId); return r.events.some(e => e.kind === 'turnEnded') && r.events; }, { timeout: 120000, interval: 500, label: 'turn ended' });
  const turn = backendLog().find(e => e.event === 'turn' && e.scenario === 'ip-pick');
  const user = events.find(e => Array.isArray(e.data?.attachments) && e.data.attachments.length);
  await waitFor(() => panel.run(`document.getElementById('transcript').innerText.includes('Got them.')`), { timeout: 20000, label: 'reply' }).catch(() => {});
  await shot(panel.sid, '08-panel-sent');
  step('sent', { selectors: items.map(c => c.selector), attachments: user?.data.attachments.length ?? 0 });
  for (const c of items) check(turn?.text.includes(c.selector), `the agent got ${c.selector}`);
  check((user?.data.attachments.length ?? 0) === items.filter(c => c.image).length && items.some(c => c.image), 'the screenshots went with the message');

  // 8. A chrome:// page.
  await rig.ev(`chrome.tabs.update(${X.tabId}, { url: 'chrome://version' })`);
  const off = await waitFor(async () => { await panel.run(`document.getElementById('refresh').click()`).catch(() => {}); const v = await panelView(panel); return v.disabled && v; }, { timeout: 15000, label: 'button disabled' }).catch(() => null);
  await shot(panel.sid, '09-panel-chrome-page');
  step('chrome page', { off });
  check(off?.disabled, 'the Pick button is disabled on a chrome:// page');

  // 9. The shortcut's input frame, same theme.
  await rig.ev(`chrome.tabs.update(${X.tabId}, { url: ${JSON.stringify(rig.site + '/inspect')} })`);
  await waitFor(async () => (await rig.ev(`chrome.tabs.get(${X.tabId})`)).status === 'complete', { label: 'back' });
  await sleep(500);
  const page2 = { sid: await A.attach(X.targetId) };
  await styled(page2.sid);
  await rig.ev(`globalThis.__chromeUseAcceptance.startInspect(${X.tabId})`);
  await sleep(500);
  const p = await A.evaluate(page2.sid, points.save);
  await A.click(page2.sid, p.x, p.y);
  const frame = await A.realFrame();
  await styled(frame.sid);
  await sleep(1500);
  await shot(page2.sid, '10-page-frame');
  evidence.passed = true;
  console.log('[inspect-pick] PASS');
} catch (failure) {
  evidence.error = String(failure.stack || failure.message || failure);
  console.log('[inspect-pick] FAIL', evidence.error);
} finally {
  save();
  cleanup(rig);
  console.log(`evidence: ${out}`);
  setTimeout(() => process.exit(evidence.passed ? 0 : 1), 1000).unref();
}
