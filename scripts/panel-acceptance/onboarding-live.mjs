// Time-to-first-answer for a user who starts from the extension (#781 G4).
//
//   node onboarding-live.mjs <chrome-use-darwin-<arch>.dmg> [--real-codex]
//
// A fresh Chrome for Testing profile with the unpacked extension and NO
// native host, in a private HOME under /tmp/ob/<stamp>. The side panel page
// must show the setup guide; then the disk image's ChromeUse.app runs the
// same setup a Finder double-click runs (`__gui-setup`, which Finder launch
// calls) into that private HOME, and the panel must connect by itself,
// without a reload. With --real-codex the agent step is checked and one real
// Codex turn is timed, using the build host's Codex login (CODEX_HOME) in
// place of the user's own `codex login`.
//
// Never touches the host's own Chrome, its relay or its install:
// AGENT_BROWSER_BIN_DIR keeps the install in the private HOME, and
// CHROME_USE_NO_BROWSER_SCAN keeps `extension install` from registering the
// host in other running browsers' data directories (other agents' rigs).
// Steps a person does by hand (download, Gatekeeper's "Open", the sign-in in
// a browser) are not timed here; the report says which.
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { launch, api } from '../inspect-acceptance/lib.mjs';
import { waitFor, sleep } from './lib.mjs';

const [dmg, ...flags] = process.argv.slice(2);
if (!dmg || !fs.existsSync(dmg)) { console.error('usage: node onboarding-live.mjs <dmg> [--real-codex]'); process.exit(64); }
const realCodex = flags.includes('--real-codex');
const EXT_ID = 'ciiljdlhdpfckdcfkphgmfalanpdejep';
const chrome = process.env.CHROME || path.join(os.homedir(), '.chrome-use/browsers/chrome-155.0.8059.39/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing');
const extSrc = process.env.EXT_DIR || path.resolve(path.dirname(new URL(import.meta.url).pathname), '../../extensions/ab-connect');
const stamp = new Date().toISOString().replace(/\D/g, '').slice(4, 14);
const work = path.join('/tmp/ob', stamp);
const home = path.join(work, 'home');
const udd = path.join(work, 'chrome-profile');
const mnt = path.join(work, 'mnt');
const ext = path.join(work, 'ext');
for (const d of [home, udd, mnt]) fs.mkdirSync(d, { recursive: true });
// English messages only, as the other rigs do (macOS ignores --lang).
fs.cpSync(extSrc, ext, { recursive: true, filter: src => !src.endsWith('.test.js') });
for (const lang of fs.readdirSync(path.join(ext, '_locales'))) if (lang !== 'en') fs.rmSync(path.join(ext, '_locales', lang), { recursive: true });

// A copied auth.json would refresh (rotate) the user's ChatGPT token in the
// copy and log the real one out, so a real-Codex run shares the build host's
// Codex home, as the panel-acceptance rig does (REAL_CODEX_HOME).
const codexHome = process.env.REAL_CODEX_HOME || path.join(os.homedir(), '.codex');
const env = { ...(realCodex ? { CODEX_HOME: codexHome } : {}), HOME: home, CHROME_USE_NO_UPDATE_CHECK: '1', CHROME_USE_NO_BROWSER_SCAN: '1', AGENT_BROWSER_BIN_DIR: path.join(home, '.local/bin'), PATH: `${path.join(os.homedir(), '.local/bin')}:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin` };
const marks = {};
const t0 = Date.now();
const mark = name => { marks[name] = (Date.now() - t0) / 1000; console.log(`${marks[name].toFixed(1)}s ${name}`); };
let browser;
const cleanup = () => {
  try { browser?.close(); } catch {}
  spawnSync('hdiutil', ['detach', mnt, '-force'], { stdio: 'ignore' });
  // The panel service's launchd job of this run only (its program lives here).
  const list = spawnSync('launchctl', ['list'], { encoding: 'utf8' }).stdout || '';
  for (const label of list.split('\n').map(l => l.split('\t')[2]).filter(l => l?.startsWith('com.chrome-use.panel-service.'))) {
    const info = spawnSync('launchctl', ['print', `gui/${process.getuid()}/${label}`], { encoding: 'utf8' }).stdout || '';
    if (info.includes(work) || info.includes(`/private${work}`)) spawnSync('launchctl', ['bootout', `gui/${process.getuid()}/${label}`], { stdio: 'ignore' });
  }
  spawnSync('pkill', ['-f', work], { stdio: 'ignore' });
};

try {
  // Chrome for Testing sometimes takes longer than launch()'s 20 s on a busy
  // build host; a fresh start is cheaper than a longer wait.
  for (let attempt = 1; !browser; attempt++) {
    try { browser = await launch({ chrome, port: 9393, userDataDir: udd, env, args: ['--headless=new', `--load-extension=${ext}`, `--disable-extensions-except=${ext}`] }); }
    catch (error) { spawnSync('pkill', ['-f', udd]); await sleep(1000); if (attempt === 3) throw error; }
  }
  const cdp = api(browser);
  mark('chrome started');
  const panel = await cdp.openPage(`chrome-extension://${EXT_ID}/sidepanel.html`);
  const ev = expr => cdp.evaluate(panel.sid, expr);
  const guide = await waitFor(async () => {
    const r = await ev(`(() => { const b = document.getElementById('onboardHost'); return b && !b.hidden && b.textContent.length > 20 ? { text: b.innerText, connection: document.getElementById('connection').textContent, buttons: [...b.querySelectorAll('button')].map(x => x.textContent) } : null })()`);
    return r;
  }, { timeout: 20000, label: 'setup guide' });
  mark('setup guide shown');
  console.log('  connection line:', guide.connection);
  console.log('  buttons:', guide.buttons.join(' | '));
  if (!guide.buttons.some(b => /Download for Mac/.test(b))) throw new Error('no Mac download button in the guide');

  // "Open the download, double-click ChromeUse": the disk image's app, run
  // through the same setup path Finder launch takes.
  const attach = spawnSync('hdiutil', ['attach', dmg, '-nobrowse', '-readonly', '-mountpoint', mnt], { encoding: 'utf8' });
  if (attach.status !== 0) throw new Error('hdiutil attach: ' + attach.stderr);
  mark('disk image mounted');
  const app = path.join(mnt, 'ChromeUse.app/Contents/MacOS/chrome-use');
  const setup = spawnSync(app, ['__gui-setup', '--no-dialog', '--user-data-dir', udd], { env: { ...process.env, ...env }, encoding: 'utf8' });
  mark('ChromeUse setup finished');
  console.log('  setup:', setup.status, (setup.stdout + setup.stderr).trim().slice(0, 400));
  if (setup.status !== 0) throw new Error('setup failed');
  const link = fs.readlinkSync(path.join(home, '.local/bin/chrome-use'));
  if (!link.startsWith(path.join(home, '.local/share/chrome-use/ChromeUse.app'))) throw new Error('unexpected link ' + link);

  await waitFor(async () => ev(`document.getElementById('onboardHost').hidden && /Connected|connected/.test(document.getElementById('connection').textContent + document.getElementById('connection').className)`), { timeout: 60000, interval: 250, label: 'panel connects without a reload' });
  mark('panel connected (no reload)');
  // Ejecting while the installed copy runs must not matter: it runs from HOME.
  spawnSync('hdiutil', ['detach', mnt, '-force'], { stdio: 'ignore' });

  const agents = await waitFor(async () => ev(`(() => { const b = document.getElementById('onboardAgents'); return b && !b.hidden && b.innerText.length > 10 ? b.innerText : null })()`), { timeout: 30000, label: 'agent step' });
  mark('agent step shown');
  console.log('  ' + agents.replace(/\n+/g, ' | '));

  if (realCodex) {
    // The user's `codex login` (a browser sign-in) is not timed: CODEX_HOME
    // already holds the build host's login.
    await ev(`[...document.querySelectorAll('#onboardAgents button')].find(b => /Check sign-in/.test(b.textContent)).click()`);
    const ready = await waitFor(async () => ev(`(() => { const t = document.getElementById('onboardAgents').innerText; return /Codex\\s*Ready/.test(t) ? t : null })()`), { timeout: 90000, label: 'Codex ready' })
      .catch(async error => {
        console.error('  agent step:', (await ev(`document.getElementById('onboardAgents').innerText`)).replace(/\n+/g, ' | '));
        console.error('  agentInspect:', JSON.stringify(await ev(`chrome.runtime.sendMessage({ type: 'ab-agent-request', operation: 'agentInspect', params: { agentId: 'codex' } })`)).slice(0, 800));
        throw error;
      });
    mark('agent step says Codex ready');
    console.log('  ' + ready.replace(/\n+/g, ' | '));
    await waitFor(async () => ev(`!document.getElementById('message').disabled`), { timeout: 30000, label: 'composer enabled' });
    await ev(`(() => { const a = document.getElementById('agent'); if ([...a.options].some(o => o.value === 'codex')) { a.value = 'codex'; a.dispatchEvent(new Event('change')); } const m = document.getElementById('message'); m.value = 'Reply with exactly the word pong and nothing else. Do not use any tools.'; m.dispatchEvent(new Event('input')); document.getElementById('composer').requestSubmit(); })()`);
    mark('first message sent');
    const answer = await waitFor(async () => ev(`(() => { const t = document.getElementById('transcript').innerText; return /\\bpong\\b/i.test(t.replace(/Reply with exactly the word pong[^\\n]*/i, '')) ? t : null })()`), { timeout: 240000, interval: 500, label: 'first answer' })
      .catch(async error => { console.error('  panel:', JSON.stringify(await ev(`({ notice: document.getElementById('notice').textContent, phase: document.getElementById('phase').textContent, agent: document.getElementById('agent').value, message: document.getElementById('message').value, transcript: document.getElementById('transcript').innerText.slice(-600) })`))); throw error; });
    mark('first answer in the panel');
    console.log('  transcript tail:', answer.trim().split('\n').slice(-4).join(' | '));
  }
  fs.writeFileSync(path.join(work, 'timings.json'), JSON.stringify(marks, null, 2));
  console.log('timings:', JSON.stringify(marks));
} catch (error) {
  console.error('FAILED:', error.message);
  try { console.error(fs.readFileSync(path.join(home, '.chrome-use/gui-setup.log'), 'utf8').slice(-2000)); } catch {}
  process.exitCode = 1;
} finally {
  if (!process.env.KEEP) cleanup();
}
