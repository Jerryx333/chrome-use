// Live check of #661 on the build host (one browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node stall-live.mjs
// A relay session's page blocks its main thread; commands sent meanwhile
// must say the tab is alive and busy (wait and retry), not that the session
// needs a restart. Then the tab's debugger is detached behind the session's
// back, and the next command must say the connection is gone. Evidence goes
// to $OUT (default <work>/out); with STALL_EXPLORE=1 nothing is asserted.
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup } from './rig.mjs';
import { sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/st-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const explore = process.env.STALL_EXPLORE === '1';
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[stall] ${name}`, JSON.stringify(data).slice(0, 1500)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok && !explore) { save(); throw new Error(message); } if (!ok) step('would fail', { message }); };

const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8960, cdpPort: 9421 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const project = path.join(work, 'proj');
fs.mkdirSync(project, { recursive: true });

function cu(args, extra = {}) {
  const env = { ...process.env, HOME: rig.home, CHROME_USE_NO_UPDATE_CHECK: '1', CLAUDE_PID: '5001', TMUX_PANE: '%81', ...extra };
  for (const k of ['AGENT_BROWSER_SESSION', 'CMUX_SURFACE_ID', 'CMUX_CLAUDE_PID', 'TERM_SESSION_ID', 'ITERM_SESSION_ID', 'GHOSTTY_SURFACE_ID', 'CODEX_THREAD_ID']) delete env[k];
  const started = Date.now();
  const r = spawnSync(rig.hostBin, ['--json', ...args], { cwd: project, env, encoding: 'utf8', timeout: 120000 });
  let json = null;
  try { json = JSON.parse(r.stdout.trim().split('\n').at(-1)); } catch {}
  return { code: r.status, json, ms: Date.now() - started, stderr: r.stderr.slice(-2000) };
}
const summary = r => ({ code: r.code, ms: r.ms, success: r.json?.success, error: r.json?.error, data: r.json?.data, stderr: r.json ? undefined : r.stderr });

try {
  const opened = cu(['open', rig.site + '/a?stall']);
  step('open', summary(opened));
  check(opened.code === 0, 'open failed');

  // 1. The page blocks its main thread for 25 s (from a timer, so the
  // command that starts it returns first).
  const block = cu(['eval', 'setTimeout(() => { const t = Date.now() + 25000; while (Date.now() < t) {} }, 100); "armed"']);
  step('armed a 25 s main-thread block', summary(block));
  const blockedAt = Date.now();
  await sleep(600);
  const busyEval = cu(['eval', '1 + 1']);
  step('eval while blocked', summary(busyEval));
  const busyShot = cu(['screenshot', path.join(out, 'busy.png')]);
  step('screenshot while blocked', summary(busyShot));
  const busyList = cu(['tab', 'list']);
  step('tab list while blocked', summary(busyList));
  const busyText = `${busyEval.json?.error || ''}`;
  check(busyEval.json?.success === false, 'eval during the block did not fail');
  check(/Diagnosis: the tab is alive/.test(busyText) && /main thread/.test(busyText), `busy eval did not say the page is busy: ${busyText}`);
  check(!/restart the session and retry/.test(busyText), `busy eval still says to restart the session: ${busyText}`);
  const busyShotText = `${busyShot.json?.error || ''}`;
  check(!/drifted/.test(busyShotText), `a busy page was reported as a drifted target: ${busyShotText}`);
  check(busyShot.json?.success === true || /Diagnosis: the tab is alive/.test(busyShotText), `busy screenshot neither worked nor said the page is busy: ${busyShotText}`);

  // 2. After the block, the same read works again.
  const wait = blockedAt + 27000 - Date.now();
  if (wait > 0) await sleep(wait);
  const after = cu(['eval', '1 + 1']);
  step('eval after the block', summary(after));
  check(after.json?.success === true && after.json?.data?.result === 2, 'eval after the block failed');
  const afterShot = cu(['screenshot', path.join(out, 'after.png')]);
  step('screenshot after the block', summary(afterShot));

  // 3. The tab's debugger is detached behind the session's back.
  const tabs = await rig.ev('chrome.tabs.query({})');
  const tab = tabs.find(t => (t.url || '').includes('/a?stall'));
  step('detach the tab debugger', { tabId: tab?.id });
  const detached = await rig.ev(`chrome.debugger.detach({ tabId: ${tab?.id} }).then(() => 'detached', e => String(e))`);
  step('detach result', { detached });
  const deadEval = cu(['eval', '1 + 1']);
  step('eval after detach', summary(deadEval));
  const deadShot = cu(['screenshot', path.join(out, 'dead.png')]);
  step('screenshot after detach', summary(deadShot));

  // 4. The tab is closed behind the session's back.
  await rig.ev(`chrome.tabs.remove(${tab?.id}).then(() => 'closed', e => String(e))`);
  const goneEval = cu(['eval', '1 + 1']);
  step('eval after the tab closed', summary(goneEval));
  const goneShot = cu(['screenshot', path.join(out, 'gone.png')]);
  step('screenshot after the tab closed', summary(goneShot));
  check(goneEval.json?.success === false && goneShot.json?.success === false, 'commands on a closed tab did not fail');
  check(!/drifted/.test(`${goneShot.json?.error || ''}`), `a closed tab was reported as a drifted target: ${goneShot.json?.error}`);
  cu(['done']);
  step('END', {});
  evidence.pass = true;
} catch (error) {
  step('FAIL', { error: String(error?.stack || error) });
  evidence.pass = false;
  process.exitCode = 1;
} finally {
  save();
  cleanup(rig);
  process.exit(process.exitCode ?? 0);
}
