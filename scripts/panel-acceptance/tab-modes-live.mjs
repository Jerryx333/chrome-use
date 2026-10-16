// Live end-to-end check of the panel's tab modes (ab-connect 0.5.62, CLI
// 1.5.221) through the real side panel and a panel tab (build host only; one
// browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node tab-modes-live.mjs
// Chrome for Testing runs headless with its own profile, the unpacked
// extension and the native host in an isolated HOME (rig.mjs); the agent is
// the Codex fixture backend calling the real `chrome-use mcp`. Steps:
//   1. "Open in a tab": the panel as a full-page tab on one conversation, in a
//      centred column, with the window's last web tab as "this page"; the side
//      panel shows the same conversation and its saved selection is untouched;
//   2. a message sent from the tab reaches the service once and shows in both;
//   3. "Let the agent use this tab" from the panel tab: both views show the
//      grant, the tab carries the AI badge, and the agent's eval without a tab
//      handle and with chrome-tab:<id> both run in the user's tab;
//   4. "Take back the tab" from the side panel: both views drop it, the badge
//      goes, the debugger lets go, and the agent's next call is refused;
//   5. a grant follows the tab to another origin, says when the page cannot be
//      used, and ends when the tab closes.
// Evidence and screenshots go to $OUT (default <work>/out).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup, openTab, activate, EXT_ID } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/tm-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[tab-modes] ${name}`, JSON.stringify(data).slice(0, 400)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok) { save(); throw new Error(message); } };

let failState = async () => {};
const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8930, cdpPort: 9391 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const A = rig.cdp, b = rig.browser;
const scenario = (name, spec) => fs.writeFileSync(path.join(rig.accept, 'scenarios', name + '.json'), JSON.stringify(spec, null, 1));
const backendLog = () => fs.existsSync(path.join(rig.accept, 'backends.jsonl')) ? fs.readFileSync(path.join(rig.accept, 'backends.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line)) : [];
const badge = tabId => rig.ev(`chrome.action.getBadgeText({ tabId: ${tabId} })`);
async function turn(conversationId, count) {
  return waitFor(async () => {
    const r = await rig.service.journal(conversationId);
    const ended = r.events.filter(e => e.kind === 'turnEnded');
    return ended.length >= count && { events: r.events, outcome: ended.at(-1).data.outcome };
  }, { timeout: 120000, interval: 500, label: `turn ${count} ended` });
}
try {
  // Two web tabs; the second is the one the user was on last.
  const older = await openTab(rig, rig.site + '/c');
  const site = await openTab(rig, rig.site + '/a');
  await activate(rig, site);
  const created = await rig.service.call('conversationCreate', { agentId: 'codex', executable: rig.backend('codex') });
  const id = created.conversation.id;
  await rig.ev(`chrome.storage.local.set({ ab_panel_conversation: ${JSON.stringify(id)} })`);
  const win = await rig.ev('chrome.windows.getLastFocused()');

  // 1. The panel tab.
  const tabView = await A.openPage(`chrome-extension://${EXT_ID}/sidepanel.html?mode=tab&conversation=${id}`);
  const inTab = expression => A.evaluate(tabView.sid, expression);
  const msg = (key, ...subs) => inTab(`chrome.i18n.getMessage(${JSON.stringify(key)}, ${JSON.stringify(subs)})`);
  evidence.uiLanguage = await inTab('chrome.i18n.getUILanguage()');
  const shotOf = async (sid, name) => { const s = await b.send('Page.captureScreenshot', { format: 'png' }, sid); fs.writeFileSync(path.join(out, `${name}.png`), Buffer.from(s.data, 'base64')); };
  const viewState = view => view(`({ notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').textContent, connection: document.getElementById('connection').textContent, selected: document.getElementById('conversation').value, grantBar: !document.getElementById('grantBar').hidden, grantText: document.getElementById('grantText').textContent, grantNote: document.getElementById('grantNote').hidden ? '' : document.getElementById('grantNote').textContent, grantTab: !document.getElementById('grantTab').hidden, picker: document.getElementById('pagePicker').value })`);
  failState = async () => { await shotOf(tabView.sid, 'fail-tab').catch(() => {}); evidence.failure = { tab: await viewState(inTab).catch(e => String(e)) }; };
  await waitFor(() => inTab(`document.getElementById('conversation').value === ${JSON.stringify(id)} && document.getElementById('connection').classList.contains('ok') && document.getElementById('pagePicker').value !== ''`), { timeout: 30000, label: 'panel tab on its conversation' });
  const layout = await inTab(`({ mode: document.documentElement.dataset.mode, width: document.getElementById('chatView').getBoundingClientRect().width, viewport: innerWidth, picker: document.getElementById('pagePicker').value, options: [...document.getElementById('pagePicker').options].map(o => o.textContent), openInTab: document.getElementById('openInTab').hidden, title: document.getElementById('pagePicker').title })`);
  await shotOf(tabView.sid, '1-panel-tab');
  step('panel tab open', layout);
  check(layout.mode === 'tab' && layout.width <= 880 && layout.viewport > 880, 'the panel tab is one centred column');
  check(layout.picker === String(site.tabId), 'this page is the last web tab of the window');
  check(layout.options.length === 2 && layout.openInTab === true, 'the picker lists the web tabs only');
  check(layout.title === await msg('panel_page_tab_last_active', site.url), 'the panel tab says where this page comes from');

  // The real side panel, opened by a trusted click, on the same conversation.
  const opener = await A.openPage(`chrome-extension://${EXT_ID}/popup.html`);
  await A.evaluate(opener.sid, `(() => { const b = document.createElement('button'); b.style = 'position:fixed;left:0;top:0;width:200px;height:80px;z-index:2147483647'; b.onclick = () => chrome.sidePanel.open({ windowId: ${win.id} }); document.body.append(b); return true; })()`);
  await A.click(opener.sid, 50, 40);
  const panelTarget = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => t.url.endsWith('/sidepanel.html')), { label: 'side panel' });
  const ps = await A.attach(panelTarget.targetId);
  await b.send('Target.closeTarget', { targetId: opener.targetId });
  const inPanel = expression => A.evaluate(ps, expression);
  failState = async () => { await shotOf(tabView.sid, 'fail-tab').catch(() => {}); await shotOf(ps, 'fail-panel').catch(() => {}); evidence.failure = { tab: await viewState(inTab).catch(e => String(e)), panel: await viewState(inPanel).catch(e => String(e)) }; };
  await waitFor(() => inPanel(`document.getElementById('conversation').value === ${JSON.stringify(id)}`), { timeout: 30000, label: 'side panel on the same conversation' });
  const stored = await rig.ev(`chrome.storage.local.get('ab_panel_conversation')`);
  step('side panel open on the same conversation', { stored });
  check(stored.ab_panel_conversation === id, 'the panel tab left the side panel selection alone');

  // 2. A message from the panel tab: once at the service, shown in both.
  scenario('tm-hello', { steps: [{ say: 'Hello from the fixture.' }] });
  await inTab(`(() => { document.getElementById('message').value = 'run scenario tm-hello'; document.getElementById('composer').requestSubmit(); return true; })()`);
  const first = await turn(id, 1);
  const users = first.events.filter(e => e.kind === 'user');
  await waitFor(() => inPanel(`document.getElementById('transcript').textContent.includes('Hello from the fixture.')`), { timeout: 20000, label: 'side panel shows the answer' });
  await waitFor(() => inTab(`document.getElementById('transcript').textContent.includes('Hello from the fixture.')`), { timeout: 20000, label: 'panel tab shows the answer' });
  step('message from the panel tab', { users: users.length, outcome: first.outcome });
  check(users.length === 1 && first.outcome === 'completed', 'one message, one turn');

  // 3. Lend the user's tab from the panel tab.
  await waitFor(() => inTab(`!document.getElementById('grantTab').hidden && !document.getElementById('grantTab').disabled`), { timeout: 20000, label: 'grant button' });
  await inTab(`document.getElementById('grantTab').click()`);
  await waitFor(() => inTab(`!document.getElementById('grantBar').hidden`), { timeout: 30000, label: 'grant bar in the panel tab' });
  await waitFor(() => inPanel(`!document.getElementById('grantBar').hidden`), { timeout: 10000, label: 'grant bar in the side panel' });
  const granted = { tab: await viewState(inTab), panel: await viewState(inPanel), badge: await badge(site.tabId), otherBadge: await badge(older.tabId), title: await rig.ev(`chrome.action.getTitle({ tabId: ${site.tabId} })`), state: (await rig.ev(`chrome.storage.session.get('ab_tab_grants')`)).ab_tab_grants };
  await shotOf(tabView.sid, '3-granted-tab'); await shotOf(ps, '3-granted-panel');
  step('tab lent', granted);
  check(granted.tab.notice === '', `the grant went through without a warning: ${granted.tab.notice}`);
  check(granted.badge === 'AI' && granted.otherBadge !== 'AI', 'only the lent tab carries the badge');
  check(granted.tab.grantText === granted.panel.grantText && granted.tab.grantText.includes('127.0.0.1:8930'), 'both views name the lent tab and its origin');
  const grantEvents = (await rig.service.journal(id)).events.filter(e => e.kind === 'tabGrant');
  check(grantEvents.at(-1)?.data.granted === true && grantEvents.at(-1).data.tabId === site.tabId, 'the service recorded the grant');

  scenario('tm-grant', { steps: [
    { tool: 'chrome_use_eval', args: { script: 'location.href' }, save: 'plain' },
    { tool: 'chrome_use_eval', args: { script: 'document.title', tabId: `chrome-tab:${site.tabId}`, force: true }, save: 'forced' },
    { say: 'Used your tab.' },
  ] });
  await inTab(`(() => { document.getElementById('message').value = 'run scenario tm-grant'; document.getElementById('composer').requestSubmit(); return true; })()`);
  await turn(id, 2);
  const used = backendLog().filter(e => e.event === 'tool' && e.scenario === 'tm-grant');
  step('agent used the lent tab', { used: used.map(e => ({ args: e.args, isError: e.isError, result: e.result.slice(0, 300) })) });
  check(used.length === 2 && used.every(e => !e.isError), 'both calls ran');
  check(used[0].result.includes(`${rig.site}/a`), 'a call without a tab handle ran in the lent tab');
  check(used[1].result.includes('Page A'), 'a call naming chrome-tab:<id> ran in the lent tab');

  // 4. Take it back from the side panel.
  await inPanel(`document.getElementById('grantRevoke').click()`);
  await waitFor(() => inPanel(`document.getElementById('grantBar').hidden`), { timeout: 20000, label: 'side panel drops the grant' });
  await waitFor(() => inTab(`document.getElementById('grantBar').hidden`), { timeout: 10000, label: 'panel tab drops the grant' });
  const attached = (await rig.ev('chrome.debugger.getTargets()')).find(t => t.tabId === site.tabId)?.attached;
  const back = { badge: await badge(site.tabId), attached, state: (await rig.ev(`chrome.storage.session.get('ab_tab_grants')`)).ab_tab_grants };
  await shotOf(ps, '4-taken-back');
  step('tab taken back', back);
  check(back.badge !== 'AI' && back.attached === false, 'badge cleared and debugger released');
  check(back.state.refused.includes(site.tabId), 'the tab is refused');
  scenario('tm-after', { steps: [
    { tool: 'chrome_use_eval', args: { script: 'location.href', tabId: `chrome-tab:${site.tabId}`, force: true }, save: 'refused' },
    { say: 'Tried your tab.' },
  ] });
  await inPanel(`(() => { document.getElementById('message').value = 'run scenario tm-after'; document.getElementById('composer').requestSubmit(); return true; })()`);
  await turn(id, 3);
  const refused = backendLog().filter(e => e.event === 'tool' && e.scenario === 'tm-after');
  step('agent refused after take-back', { refused: refused.map(e => ({ isError: e.isError, result: e.result.slice(0, 400) })) });
  check(refused.length === 1 && refused[0].isError && refused[0].result.includes('took this tab back'), 'the next call is refused with the reason');

  // 5. Lend it again, follow it to another origin, a Chrome page, then close.
  await inTab(`document.getElementById('grantTab').click()`);
  await waitFor(() => inTab(`!document.getElementById('grantBar').hidden`), { timeout: 30000, label: 'granted again' });
  const otherOrigin = `http://localhost:8930/b`;
  await rig.ev(`chrome.tabs.update(${site.tabId}, { url: ${JSON.stringify(otherOrigin)} })`);
  await waitFor(() => inTab(`document.getElementById('grantText').textContent.includes('localhost:8930')`), { timeout: 20000, label: 'grant shows the new origin' });
  await sleep(500);
  const moved = { tab: await viewState(inTab), badge: await badge(site.tabId) };
  step('grant followed the tab to another origin', moved);
  check(moved.badge === 'AI', 'the badge stays across a navigation');
  await rig.ev(`chrome.tabs.update(${site.tabId}, { url: 'chrome://version/' })`);
  await waitFor(() => inTab(`!document.getElementById('grantNote').hidden`), { timeout: 20000, label: 'unusable page named' });
  step('Chrome page named', { note: (await viewState(inTab)).grantNote });
  check((await viewState(inTab)).grantNote === await msg('panel_grant_page_unusable'), 'the note says the page cannot be used');
  await shotOf(tabView.sid, '5-chrome-page');
  await rig.ev(`chrome.tabs.remove(${site.tabId})`);
  await waitFor(() => inTab(`document.getElementById('grantBar').hidden`), { timeout: 20000, label: 'grant ends with the tab' });
  const after = (await rig.ev(`chrome.storage.session.get('ab_tab_grants')`)).ab_tab_grants;
  step('grant ended with the tab', after);
  check(after.grants.length === 0 && !after.refused.includes(site.tabId), 'nothing is kept for a closed tab');
  const finalGrant = (await rig.service.journal(id)).events.filter(e => e.kind === 'tabGrant').at(-1);
  check(finalGrant?.data.granted === false, 'the service heard the tab close');
  evidence.passed = true;
  console.log('[tab-modes] PASS');
} catch (failure) {
  evidence.error = String(failure.message || failure);
  await failState();
  throw failure;
} finally {
  save();
  cleanup(rig);
  console.log(`evidence: ${out}`);
}
