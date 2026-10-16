// Live end-to-end check of per-tab side panels (ab-connect 0.5.63, CLI
// 1.5.223) through the real side panel (build host only; one browser run per
// host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node per-tab-live.mjs
// Chrome for Testing runs headless with its own profile, the unpacked
// extension and the native host in an isolated HOME (rig.mjs); the agent is
// the Codex fixture backend calling the real `chrome-use mcp`. A tab's panel
// is opened the way the toolbar button opens it (setOptions for that tab, then
// open) from a trusted click in another window. Steps:
//   1. tab X's panel starts a new conversation; its first message lends X by
//      itself (AI badge, grant bar), and the agent's call without a tab handle
//      runs in X, with no tab of its own opened;
//   1b. the conversation's daemon exits (as after its idle timeout); the next
//      message still runs in X and opens no tab or window;
//   2. tab Y has no panel until one is opened on it; Y's panel starts its own
//      conversation; X's panel still shows X's;
//   3. slash completion in the middle of a message, and no raw message key on
//      screen;
//   4. "Take back the tab" in X's panel: badge gone, the conversation now says
//      it works in the agent's own window;
//   5. closing Y keeps its conversation in the history and ends its lend.
// Evidence and screenshots go to $OUT (default <work>/out).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup, openTab, activate, EXT_ID } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/pt-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[per-tab] ${name}`, JSON.stringify(data).slice(0, 400)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok) { save(); throw new Error(message); } };

// A skill the agent has, for slash completion.
fs.mkdirSync(path.join(work, 'home', '.agents', 'skills', 'profile-use'), { recursive: true });
fs.writeFileSync(path.join(work, 'home', '.agents', 'skills', 'profile-use', 'SKILL.md'), '---\nname: profile-use\ndescription: Fill forms from the local profile.\n---\nUse the profile.\n');

let failState = async () => {};
const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8940, cdpPort: 9401 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const A = rig.cdp, b = rig.browser;
const scenario = (name, spec) => fs.writeFileSync(path.join(rig.accept, 'scenarios', name + '.json'), JSON.stringify(spec, null, 1));
const backendLog = () => fs.existsSync(path.join(rig.accept, 'backends.jsonl')) ? fs.readFileSync(path.join(rig.accept, 'backends.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line)) : [];
const badge = tabId => rig.ev(`chrome.action.getBadgeText({ tabId: ${tabId} })`);
const shotOf = async (sid, name) => { const s = await b.send('Page.captureScreenshot', { format: 'png' }, sid); fs.writeFileSync(path.join(out, `${name}.png`), Buffer.from(s.data, 'base64')); };
const panelTargets = async () => (await b.send('Target.getTargets')).targetInfos.filter(t => /\/sidepanel\.html(\?|$)/.test(t.url));

// Open tab `tab`'s panel the way the toolbar button does, from a trusted
// click in a window of its own (so `tab` stays the active tab of its window).
async function openPanelFor(tab) {
  const win = await rig.ev(`chrome.windows.create({ url: chrome.runtime.getURL('popup.html'), focused: true })`);
  const opener = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => t.url.endsWith('/popup.html') && t.type === 'page'), { label: 'opener page' });
  const sid = await A.attach(opener.targetId);
  await waitFor(() => A.evaluate(sid, 'document.readyState === "complete"'), { label: 'opener load' });
  await A.evaluate(sid, `(() => { const b = document.createElement('button'); b.style = 'position:fixed;left:0;top:0;width:200px;height:80px;z-index:2147483647'; b.onclick = () => { chrome.sidePanel.setOptions({ tabId: ${tab.tabId}, path: 'sidepanel.html?tab=${tab.tabId}', enabled: true }); chrome.sidePanel.open({ tabId: ${tab.tabId} }); }; document.body.append(b); return true; })()`);
  await A.click(sid, 50, 40);
  const target = await waitFor(async () => (await panelTargets()).find(t => t.url.endsWith(`?tab=${tab.tabId}`)), { timeout: 15000, label: `panel of tab ${tab.tabId}` });
  await rig.ev(`chrome.windows.remove(${win.id})`);
  const psid = await A.attach(target.targetId);
  await waitFor(() => A.evaluate(psid, `document.getElementById('connection').classList.contains('ok')`), { timeout: 30000, label: 'panel connected' });
  return { sid: psid, targetId: target.targetId, run: expression => A.evaluate(psid, expression) };
}
const viewState = panel => panel.run(`({ notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').textContent, selected: document.getElementById('conversation').value, grantBar: !document.getElementById('grantBar').hidden, grantText: document.getElementById('grantText').textContent, workIn: document.getElementById('workIn').value, workRow: !document.getElementById('workRow').hidden, workNote: document.getElementById('workNote').hidden ? '' : document.getElementById('workNote').textContent, page: document.getElementById('pageTitle').textContent })`);
async function sendFrom(panel, text) {
  await panel.run(`(() => { const s = document.getElementById('agent'); s.value = 'codex'; s.dispatchEvent(new Event('change')); document.getElementById('executable').value = ${JSON.stringify(rig.backend('codex'))}; document.getElementById('message').value = ${JSON.stringify(text)}; document.getElementById('composer').requestSubmit(); return true; })()`);
}
async function turnOf(conversationId, count) {
  return waitFor(async () => {
    const r = await rig.service.journal(conversationId);
    const ended = r.events.filter(e => e.kind === 'turnEnded');
    return ended.length >= count && { events: r.events, outcome: ended.at(-1).data.outcome };
  }, { timeout: 120000, interval: 500, label: `turn ${count} ended` });
}
try {
  const X = await openTab(rig, rig.site + '/a');
  const Y = await openTab(rig, rig.site + '/c');
  await activate(rig, X);
  const tabsBefore = (await rig.ev('chrome.tabs.query({})')).length;

  // 1. X's panel, a new conversation, lent X by itself.
  const px = await openPanelFor(X);
  failState = async () => { await shotOf(px.sid, 'fail-x').catch(() => {}); evidence.failure = { x: await viewState(px).catch(e => String(e)) }; };
  const fresh = await viewState(px);
  step('X panel opened', fresh);
  check(fresh.selected === '' && fresh.page === 'Page A', 'a new tab panel starts a new conversation about its own tab');
  check(fresh.workRow && fresh.workIn === 'tab', 'the agent works in this tab by default');
  scenario('pt-here', { steps: [{ tool: 'chrome_use_eval', args: { script: 'location.href' } }, { say: 'Worked here.' }] });
  await sendFrom(px, 'run scenario pt-here');
  const c1 = await waitFor(() => px.run(`document.getElementById('conversation').value`), { timeout: 30000, label: 'conversation created' });
  await turnOf(c1, 1);
  const here = backendLog().filter(e => e.event === 'tool' && e.scenario === 'pt-here');
  const lent = { view: await viewState(px), badge: await badge(X.tabId), binding: await rig.ev(`chrome.storage.session.get(null)`), tabs: (await rig.ev('chrome.tabs.query({})')).map(t => t.url), result: here.map(e => ({ isError: e.isError, result: e.result.slice(0, 300) })) };
  await shotOf(px.sid, '1-x-lent');
  step('first message ran in tab X', lent);
  check(lent.badge === 'AI' && lent.view.grantBar, 'X is lent with its indicator');
  check(here.length === 1 && !here[0].isError && here[0].result.includes(`${rig.site}/a`), 'the call without a tab handle ran in X');
  check(lent.tabs.length === tabsBefore, 'the agent opened no tab of its own');
  check(lent.binding[`ab_panel_tab_${X.tabId}`] === c1, 'X is bound to its conversation');
  const sendParams = (await rig.service.journal(c1)).events.find(e => e.kind === 'tabGrant');
  check(sendParams?.data.granted === true && sendParams.data.tabId === X.tabId, 'the service holds the grant');

  // 1b. The session's daemon exits between turns, as after its idle timeout.
  const pidFile = path.join(rig.home, '.chrome-use', `panel-${c1}.pid`);
  const firstPid = Number(fs.readFileSync(pidFile, 'utf8').trim());
  process.kill(firstPid, 'SIGTERM');
  await waitFor(() => { try { process.kill(firstPid, 0); return false; } catch { return true; } }, { timeout: 20000, label: 'daemon exit' });
  const windowsBefore = (await rig.ev('chrome.windows.getAll()')).length;
  scenario('pt-again', { steps: [{ tool: 'chrome_use_eval', args: { script: 'location.href' } }, { say: 'Still here.' }] });
  await px.run(`(() => { document.getElementById('message').value = 'run scenario pt-again'; document.getElementById('composer').requestSubmit(); return true; })()`);
  await turnOf(c1, 2);
  const again = backendLog().filter(e => e.event === 'tool' && e.scenario === 'pt-again');
  const restarted = { pid: Number(fs.readFileSync(pidFile, 'utf8').trim()), firstPid, tabs: (await rig.ev('chrome.tabs.query({})')).map(t => t.url), windows: (await rig.ev('chrome.windows.getAll()')).length, windowsBefore, result: again.map(e => ({ isError: e.isError, result: e.result.slice(0, 300) })) };
  step('after a daemon restart', restarted);
  check(restarted.pid !== firstPid, 'a new daemon served the turn');
  check(again.length === 1 && !again[0].isError && again[0].result.includes(`${rig.site}/a`), 'the call without a tab handle still ran in X');
  check(restarted.tabs.length === tabsBefore && restarted.windows === windowsBefore, 'no tab or window was opened');

  // 2. Y has no panel until one is opened there; then its own conversation.
  await activate(rig, Y);
  const yOptions = await rig.ev(`chrome.sidePanel.getOptions({ tabId: ${Y.tabId} })`);
  const xOptions = await rig.ev(`chrome.sidePanel.getOptions({ tabId: ${X.tabId} })`);
  step('panel options per tab', { x: xOptions, y: yOptions });
  check(xOptions.enabled === true && xOptions.path.endsWith(`?tab=${X.tabId}`) && yOptions.enabled === false, 'only X has a panel');
  const py = await openPanelFor(Y);
  const yFresh = await viewState(py);
  check(yFresh.selected === '' && yFresh.page === 'Page C', 'Y\'s panel starts its own conversation');
  scenario('pt-y', { steps: [{ tool: 'chrome_use_eval', args: { script: 'location.href' } }, { say: 'Worked in Y.' }] });
  await sendFrom(py, 'run scenario pt-y');
  const c2 = await waitFor(() => py.run(`document.getElementById('conversation').value`), { timeout: 30000, label: 'Y conversation' });
  await turnOf(c2, 1);
  const yUsed = backendLog().filter(e => e.event === 'tool' && e.scenario === 'pt-y');
  await activate(rig, X);
  await sleep(500);
  const xAgain = await viewState(px).catch(() => null);
  step('each tab keeps its conversation', { c1, c2, xAgain, y: yUsed.map(e => e.result.slice(0, 200)), badgeY: await badge(Y.tabId) });
  check(c2 !== c1 && yUsed[0]?.result.includes(`${rig.site}/c`), 'Y\'s agent worked in Y');
  check(xAgain?.selected === c1, 'X\'s panel shows X\'s conversation again');

  // 3. Slash completion mid-text; no raw message key anywhere.
  await px.run(`(() => { const m = document.getElementById('message'); m.value = '所有信息在 /profile'; m.setSelectionRange(m.value.length, m.value.length); m.dispatchEvent(new Event('input')); return true; })()`);
  const choices = await waitFor(() => px.run(`!document.getElementById('skillPicker').hidden && [...document.getElementById('skillPicker').children].map(c => c.textContent)`), { timeout: 15000, label: 'skill choices' });
  await px.run(`document.getElementById('message').dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }))`);
  const completed = await px.run(`document.getElementById('message').value`);
  const rawKeys = await px.run(`[...document.querySelectorAll('body *')].filter(e => !e.children.length && /^(panel|sidepanel)_[a-z0-9_]+$/.test(e.textContent.trim())).map(e => e.textContent.trim())`);
  await shotOf(px.sid, '3-slash');
  step('slash completion', { choices, completed, rawKeys });
  check(choices.some(c => c.startsWith('profile-use')) && completed === '所有信息在 /profile-use ', 'the skill completes mid-text');
  check(rawKeys.length === 0, 'no raw message key is shown');
  await px.run(`document.getElementById('message').value = ''`);

  // 4. Take X back.
  await px.run(`document.getElementById('grantRevoke').click()`);
  await waitFor(() => px.run(`document.getElementById('grantBar').hidden && !document.getElementById('workNote').hidden`), { timeout: 20000, label: 'taken back' });
  const back = { view: await viewState(px), badge: await badge(X.tabId), workIn: (await rig.ev(`chrome.storage.local.get('ab_panel_work_in')`)).ab_panel_work_in };
  await shotOf(px.sid, '4-taken-back');
  step('X taken back', back);
  check(back.badge !== 'AI' && back.view.workIn === 'window' && back.workIn[c1] === 'window', 'the conversation now works in the agent\'s window');
  check(back.view.workNote === await px.run(`chrome.i18n.getMessage('panel_work_in_window_taken_back')`), 'and says so');

  // 5. Close Y: its conversation stays, its lend and binding end.
  await rig.ev(`chrome.tabs.remove(${Y.tabId})`);
  await waitFor(async () => !(`ab_panel_tab_${Y.tabId}` in await rig.ev(`chrome.storage.session.get(null)`)), { timeout: 15000, label: 'Y binding gone' });
  const listed = (await rig.service.call('conversationList', {})).conversations.map(c => c.id);
  const grantsLeft = (await rig.ev(`chrome.storage.session.get('ab_tab_grants')`)).ab_tab_grants;
  step('Y closed', { listed, grantsLeft });
  check(listed.includes(c2), 'Y\'s conversation stays in the history');
  check(!grantsLeft.grants.some(g => g.tabId === Y.tabId), 'Y\'s lend ended');
  evidence.passed = true;
  console.log('[per-tab] PASS');
} catch (failure) {
  evidence.error = String(failure.message || failure);
  await failState();
  throw failure;
} finally {
  save();
  cleanup(rig);
  console.log(`evidence: ${out}`);
  // The rig's proxy and service sockets keep the event loop alive.
  setTimeout(() => process.exit(evidence.passed ? 0 : 1), 1000).unref();
}
