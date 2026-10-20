// Live check of #687 on the build host (one browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node takeover-live.mjs
// Two agent sessions of one project, as two agent CLIs in two panes would
// run them. While session A has a slow command in flight:
//   1. session B starts: it must not take A's tabs, and A's command must
//      finish normally;
//   2. the rig's native host dies (the relay drops) while A's command runs,
//      then B starts again: B must not take A's tabs while A's command is
//      still waiting, and A must exit non-zero with an "outcome unknown"
//      error, never 0;
//   3. `session last` tells what happened to A's last command without
//      running anything.
// Only this run's native host is ever stopped. With TAKEOVER_EXPLORE=1
// nothing is asserted: the steps only record what happens.
import fs from 'node:fs';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup } from './rig.mjs';
import { sleep, waitFor } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/to-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const explore = process.env.TAKEOVER_EXPLORE === '1';
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[takeover] ${name}`, JSON.stringify(data).slice(0, 1800)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok && !explore) { save(); throw new Error(message); } if (!ok) step('would fail', { message }); };

const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8970, cdpPort: 9431 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const project = path.join(work, 'proj');
fs.mkdirSync(project, { recursive: true });
const socketDir = path.join(rig.home, '.chrome-use');

function env(agent, pane) {
  const e = { ...process.env, HOME: rig.home, CHROME_USE_NO_UPDATE_CHECK: '1', CLAUDE_PID: String(agent), TMUX_PANE: pane };
  for (const k of ['AGENT_BROWSER_SESSION', 'CMUX_SURFACE_ID', 'CMUX_CLAUDE_PID', 'TERM_SESSION_ID', 'ITERM_SESSION_ID', 'GHOSTTY_SURFACE_ID', 'CODEX_THREAD_ID']) delete e[k];
  return e;
}
function cu(agent, pane, args) {
  const started = Date.now();
  const r = spawnSync(rig.hostBin, ['--json', ...args], { cwd: project, env: env(agent, pane), encoding: 'utf8', timeout: 120000 });
  let json = null;
  try { json = JSON.parse(r.stdout.trim().split('\n').at(-1)); } catch {}
  return { code: r.status, json, ms: Date.now() - started, stderr: r.stderr.slice(-1500) };
}
// A command left running, as an agent's slow side-effecting command is.
function cuAsync(agent, pane, args) {
  const started = Date.now();
  const child = spawn(rig.hostBin, ['--json', ...args], { cwd: project, env: env(agent, pane) });
  let stdout = '', stderr = '';
  child.stdout.on('data', d => { stdout += d; });
  child.stderr.on('data', d => { stderr += d; });
  return new Promise(resolve => child.on('close', code => {
    let json = null;
    try { json = JSON.parse(stdout.trim().split('\n').at(-1)); } catch {}
    resolve({ code, json, ms: Date.now() - started, stderr: stderr.slice(-1500) });
  }));
}
const summary = r => ({ code: r.code, ms: r.ms, success: r.json?.success, errorCode: r.json?.code, error: r.json?.error, data: r.json?.data, stderr: r.stderr });
const sessionOf = (agent, pane) => cu(agent, pane, ['status']).json?.data?.currentSession;
const tabs = () => rig.ev('chrome.tabs.query({})');
const createdOf = name => { try { return JSON.parse(fs.readFileSync(path.join(socketDir, `${name}.created-targets.json`), 'utf8')); } catch { return null; } };
const tookOver = r => /tabs of ended session/.test(r.stderr) || /taken over/.test(r.stderr);
// This rig's native host only: its command line runs the binary copied
// into this run's directory.
const nmHosts = () => spawnSync('pgrep', ['-f', `${work}/.*__nm-host`], { encoding: 'utf8' }).stdout.trim().split('\n').filter(Boolean);
const slow = ms => ['eval', `new Promise(r => setTimeout(() => { document.title = 'side effect done'; r('done'); }, ${ms}))`];

try {
  const A = { agent: 6001, pane: '%91' };
  const B = { agent: 6002, pane: '%92' };
  const openA = cu(A.agent, A.pane, ['open', rig.site + '/a?A']);
  step('A open', summary(openA));
  const nameA = sessionOf(A.agent, A.pane)?.name;
  step('A session', { name: nameA, created: createdOf(nameA) });

  // 1. B starts while A's command is in flight.
  const inflight1 = cuAsync(A.agent, A.pane, slow(12000));
  await sleep(2000);
  const openB = cu(B.agent, B.pane, ['open', rig.site + '/b?B']);
  const nameB = sessionOf(B.agent, B.pane)?.name;
  step('B open while A busy', { ...summary(openB), session: nameB, tookOver: tookOver(openB), createdA: createdOf(nameA), createdB: createdOf(nameB) });
  const a1 = await inflight1;
  step('A slow command finished', summary(a1));
  check(!tookOver(openB), 'B took over tabs while A had a command in flight');
  check(a1.code === 0 && a1.json?.data?.result === 'done', 'A\'s command did not finish normally');

  // 2. The relay drops while A's command runs.
  const inflight2 = cuAsync(A.agent, A.pane, slow(15000));
  await sleep(1500);
  const hosts = nmHosts();
  step('stop this rig\'s native host', { pids: hosts });
  for (const pid of hosts) spawnSync('kill', ['-9', pid]);
  await sleep(1500);
  const relayFile = path.join(socketDir, 'relay-cdp-url');
  step('relay after the host died', { relayUrlFile: fs.existsSync(relayFile), aSocket: fs.existsSync(path.join(socketDir, `${nameA}.sock`)) });
  // B's next command, as another agent would run one meanwhile; let the
  // relay come back first so B does not go through its own recovery.
  const back = await waitFor(() => fs.existsSync(relayFile), { timeout: 60000, label: 'relay back' }).then(() => true, () => false);
  step('relay back', { back });
  // Without the relay, a command would run the CLI's relay recovery, which
  // must never be triggered from a rig (it is not scoped to this HOME in
  // older builds).
  check(back, 'the relay did not come back after the native host died');
  const againB = back ? cu(B.agent, B.pane, ['tab', 'list']) : { code: null, json: null, ms: 0, stderr: 'skipped: relay down' };
  step('B command after the relay drop', { ...summary(againB), tookOver: tookOver(againB), createdA: createdOf(nameA), createdB: createdOf(nameB), aSocket: fs.existsSync(path.join(socketDir, `${nameA}.sock`)) });
  const a2 = await inflight2;
  step('A command that was in flight when the relay dropped', summary(a2));
  check(a2.code !== 0 || a2.json?.success === true, 'A exited 0 without a confirmed result');
  if (a2.json?.success !== true) {
    check(a2.json?.code === 'outcome_unknown', `A's unconfirmed command was not reported as outcome_unknown: ${a2.json?.code}`);
  }
  check(!tookOver(againB) || a2.code !== null, 'B took over A\'s tabs while A waited on its command');

  // 3. What happened to A's last command, without running anything.
  const last = cu(A.agent, A.pane, ['session', 'last']);
  step('A session last', summary(last));
  check(last.code === 0 && last.json?.data?.lastCommand?.action === 'evaluate', 'session last did not report A\'s last command');
  const afterA = cu(A.agent, A.pane, ['tab', 'list']);
  step('A tab list afterwards', summary(afterA));
  const title = await rig.ev(`chrome.tabs.query({}).then(t => t.filter(x => (x.url || '').includes('/a?A')).map(x => x.title))`);
  step('A page title (did the side effect run?)', { title });
  cu(A.agent, A.pane, ['done']);
  cu(B.agent, B.pane, ['done']);
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
