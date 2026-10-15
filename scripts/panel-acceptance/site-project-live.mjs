// Live end-to-end check of a site's project and /resume through the real side
// panel (build host only; one browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node site-project-live.mjs
// Chrome for Testing runs headless with its own profile, the unpacked
// extension and the native host in an isolated HOME (rig.mjs). Steps:
//   1. create a conversation in a project folder from the side panel on a
//      localhost page, answer "Remember" and check the Inspect & Ask store;
//   2. a new conversation on the same site preselects that project;
//   3. the options page lists the binding;
//   4. /resume a real Codex session made with `codex exec`, confirm the
//      "active elsewhere" warning and check the agent remembers it.
// Evidence and side-panel screenshots go to $OUT (default <work>/out).
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup, openTab, activate, EXT_ID } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/sp-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
// Codex with a copy of its login only: no user session is listed or touched.
const codexHome = path.join(work, 'codex');
fs.mkdirSync(codexHome, { recursive: true });
fs.copyFileSync(path.join(process.env.REAL_CODEX_HOME || path.join(os.homedir(), '.codex'), 'auth.json'), path.join(codexHome, 'auth.json'));
const codex = process.env.REAL_CODEX_BIN || spawnSync('sh', ['-c', 'command -v codex'], { env: { ...process.env, PATH: `${os.homedir()}/.local/bin:/opt/homebrew/bin:${process.env.PATH}` }, encoding: 'utf8' }).stdout.trim();
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[site-project] ${name}`, JSON.stringify(data).slice(0, 300)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok) { save(); throw new Error(message); } };

let failState = async () => {};
const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8920, cdpPort: 9381, serviceEnv: { CODEX_HOME: codexHome } });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const A = rig.cdp, b = rig.browser;
const project = fs.realpathSync(fs.mkdtempSync(path.join(work, 'project-')));
try {
  // A localhost page, then the real side panel (a trusted click opens it).
  const site = await openTab(rig, rig.site + '/');
  const win = await rig.ev('chrome.windows.getLastFocused()');
  const opener = await A.openPage(`chrome-extension://${EXT_ID}/sidepanel.html?opener=1`);
  await A.evaluate(opener.sid, `(() => { const b = document.createElement('button'); b.style = 'position:fixed;left:0;top:0;width:200px;height:80px;z-index:2147483647'; b.onclick = () => chrome.sidePanel.open({ windowId: ${win.id} }); document.body.append(b); return true; })()`);
  await A.click(opener.sid, 50, 40);
  const panelTarget = await waitFor(async () => (await b.send('Target.getTargets')).targetInfos.find(t => t.url.endsWith('/sidepanel.html')), { label: 'side panel' });
  const ps = await A.attach(panelTarget.targetId);
  await b.send('Target.closeTarget', { targetId: opener.targetId });
  await activate(rig, site);
  const panel = expression => A.evaluate(ps, expression);
  // Text in the panel's own language (Chrome's UI language on the host).
  const msg = (key, ...subs) => panel(`chrome.i18n.getMessage(${JSON.stringify(key)}, ${JSON.stringify(subs)})`);
  evidence.uiLanguage = await panel('chrome.i18n.getUILanguage()');
  const remembered = (await msg('panel_project_remembered', rig.site)), confirmLabel = await msg('panel_resume_confirm'), unsafe = await msg('panel_resume_unsafe');
  failState = async () => { await shot('fail').catch(() => {}); evidence.failure = await panel(`({ notice: document.getElementById('notice').textContent, projectHost: document.getElementById('projectHost').innerHTML.slice(0, 1500), project: document.getElementById('project').value, selected: document.getElementById('conversation').value, page: document.getElementById('pageTitle').textContent })`).catch(e => String(e)); };
  const shot = async name => { const s = await b.send('Page.captureScreenshot', { format: 'png' }, ps); fs.writeFileSync(path.join(out, `${name}.png`), Buffer.from(s.data, 'base64')); };
  await waitFor(() => panel(`document.getElementById('pageTitle')?.textContent.length > 0 && !document.getElementById('projectChip').hidden`), { timeout: 20000, label: 'panel with page and project chip' });
  step('panel open', { page: await panel(`document.getElementById('pageTitle').textContent`) });

  // The folder becomes a recent one through an earlier panel conversation
  // (the native "Other…" dialog cannot be driven headless).
  await rig.service.call('conversationCreate', { agentId: 'codex', project, executable: rig.backend('codex') });
  await panel(`document.getElementById('newChat').click()`);
  await waitFor(() => panel(`[...document.getElementById('project').options].some(o => o.value === ${JSON.stringify(project)})`), { timeout: 20000, label: 'project offered' });
  const before = await panel(`document.getElementById('project').value`);
  step('project offered, nothing remembered yet', { selected: before });
  check(before === '', 'no project may be preselected before one is remembered');

  // 1. Create in the project, then Remember.
  await panel(`(() => { const s = document.getElementById('agent'); s.value = 'codex'; s.dispatchEvent(new Event('change')); document.getElementById('executable').value = ${JSON.stringify(rig.backend('codex'))};
    const p = document.getElementById('project'); p.value = ${JSON.stringify(project)}; p.dispatchEvent(new Event('change'));
    document.getElementById('message').value = 'Hello from the site project check'; document.getElementById('composer').requestSubmit(); return true; })()`);
  const question = await msg('panel_project_remember', rig.site);
  await waitFor(() => panel(`!document.querySelector('.project-remember')?.hidden && document.querySelector('.project-remember').textContent.includes(${JSON.stringify(question)})`), { timeout: 30000, label: 'remember question' });
  const header = await panel(`document.getElementById('projectPath').textContent`);
  await shot('1-remember-bar');
  step('created in the project', { header, question: await panel(`document.querySelector('.project-remember').textContent`) });
  await panel(`[...document.querySelectorAll('.project-remember button')].find(b => b.textContent === ${JSON.stringify(await msg('panel_project_remember_yes'))}).click()`);
  await waitFor(() => panel(`document.querySelector('.project-remember').textContent.includes(${JSON.stringify(remembered)})`), { timeout: 20000, label: 'remembered' });
  const store = JSON.parse(fs.readFileSync(path.join(rig.home, '.chrome-use', 'inspect', 'bindings.json'), 'utf8'));
  const binding = store.bindings.find(x => x.origin === rig.site);
  step('remembered in the Inspect & Ask store', { binding });
  check(binding?.root === project, 'the binding names the project');

  // 2. A new conversation on the same site starts in it.
  await panel(`document.getElementById('newChat').click()`);
  await waitFor(() => panel(`document.getElementById('project').value === ${JSON.stringify(project)}`), { timeout: 20000, label: 'project preselected' });
  await shot('2-preselected');
  step('new conversation preselects the project', { title: await panel(`document.getElementById('project').title`) });

  // 3. The options page lists it.
  const options = await A.openPage(`chrome-extension://${EXT_ID}/options.html`);
  const listed = await waitFor(async () => { const text = await A.evaluate(options.sid, `document.getElementById('inspectBindings').innerText`); return text.includes(project) && text; }, { timeout: 20000, label: 'binding on the options page' });
  const o = await b.send('Page.captureScreenshot', { format: 'png' }, options.sid); fs.writeFileSync(path.join(out, '3-options.png'), Buffer.from(o.data, 'base64'));
  step('options page lists the binding', { text: listed.slice(0, 400) });
  await b.send('Target.closeTarget', { targetId: options.targetId });
  await activate(rig, site);

  // 4. /resume a real Codex session.
  const word = `HERON-${Math.floor(Math.random() * 90 + 10)}`;
  const codexProject = fs.realpathSync(fs.mkdtempSync(path.join(work, 'codex-project-')));
  const made = spawnSync(codex, ['exec', '--skip-git-repo-check', '-c', 'model_reasoning_effort=low', `Remember the codeword ${word}. Reply with just OK.`], { cwd: codexProject, env: { ...process.env, CODEX_HOME: codexHome }, encoding: 'utf8', timeout: 180000, stdio: ['ignore', 'pipe', 'pipe'] });
  step('terminal codex session', { status: made.status, word });
  check(made.status === 0, 'codex exec failed');
  await panel(`(() => { document.getElementById('executable').value = ''; document.getElementById('message').value = '/resume'; document.getElementById('composer').requestSubmit(); return true; })()`);
  await waitFor(() => panel(`[...document.querySelectorAll('.resume-item')].some(r => r.title === ${JSON.stringify(codexProject)})`), { timeout: 40000, label: 'codex session listed' });
  await panel(`[...document.querySelectorAll('.resume-item')].find(r => r.title === ${JSON.stringify(codexProject)}).click()`);
  // Written seconds ago: it must ask first.
  await waitFor(() => panel(`document.querySelector('.resume-picker').innerText.includes(${JSON.stringify(unsafe)})`), { timeout: 30000, label: 'active-elsewhere warning' });
  await shot('4-resume-warning');
  await panel(`[...document.querySelectorAll('.resume-picker button')].find(b => b.textContent === ${JSON.stringify(confirmLabel)}).click()`);
  await waitFor(() => panel(`!!document.querySelector('.imported-history')`), { timeout: 40000, label: 'imported history' });
  const resumed = await panel(`({ header: document.getElementById('projectPath').textContent, imported: document.querySelector('.imported-history').innerText })`);
  step('resumed', resumed);
  check(resumed.imported.includes(word), 'the earlier turn was imported');
  const list = await rig.service.call('conversationList', {});
  const conv = list.conversations.find(c => c.resumed?.cwd === codexProject);
  check(conv, 'the resumed conversation exists');
  await panel(`(() => { document.getElementById('message').value = 'What codeword did I ask you to remember? Reply with just the codeword.'; document.getElementById('composer').requestSubmit(); return true; })()`);
  const answer = await waitFor(async () => {
    const r = await rig.service.journal(conv.id);
    const ended = r.events.find(e => e.kind === 'turnEnded');
    return ended && { outcome: ended.data.outcome, text: r.events.filter(e => e.kind === 'textFinal').map(e => e.data.text).join('') };
  }, { timeout: 240000, interval: 1000, label: 'codex answer' });
  await sleep(1500); await shot('5-resume-answer');
  step('codex answered', answer);
  check(answer.outcome === 'completed' && answer.text.includes(word), 'the agent remembered the earlier turn');
  evidence.passed = true;
  console.log('[site-project] PASS');
} catch (failure) {
  evidence.error = String(failure.message || failure);
  await failState();
  throw failure;
} finally {
  save();
  cleanup(rig);
  console.log(`evidence: ${out}`);
}
