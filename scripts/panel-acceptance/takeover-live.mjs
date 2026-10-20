// Live check of #687 on the build host (one browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node takeover-live.mjs
// Agent sessions of one project, as agent CLIs in separate panes run them.
// While session A has a slow page-changing command in flight:
//   1. session B starts: it must not take A's tabs, and A's command finishes;
//   2. A's daemon dies (as a relay recovery stopped it in the report) and C
//      starts at once: C must not take A's tabs while A still waits, A must
//      exit non-zero with code action_outcome_unknown, and D, starting right
//      after A's failure, must not take them either;
//   3. `session last` reports A's interrupted command as unknown, without
//      running anything;
//   4. the rig's native host dies while A's command runs: A must not exit 0
//      without a confirmed result.
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
// Whether `name`'s record now holds any of `targets` (A's tabs). The
// takeover note is not printed in --json mode, so the records tell.
const holds = (name, targets) => (createdOf(name)?.target_ids || []).some(t => targets.includes(t));
const socketOf = name => fs.existsSync(path.join(socketDir, `${name}.sock`));
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
  const targetsA = createdOf(nameA)?.target_ids || [];
  step('A session', { name: nameA, targetsA });
  check(targetsA.length > 0, 'A has no recorded tab');
  const tookOver = name => holds(name, targetsA);

  // 1. B starts while A's command is in flight.
  const inflight1 = cuAsync(A.agent, A.pane, slow(6000));
  await sleep(1500);
  const openB = cu(B.agent, B.pane, ['open', rig.site + '/b?B']);
  const nameB = sessionOf(B.agent, B.pane)?.name;
  step('B open while A busy', { ...summary(openB), session: nameB, tookOver: tookOver(nameB) });
  const a1 = await inflight1;
  step('A slow command finished', summary(a1));
  check(!tookOver(nameB), 'B took over tabs while A had a command in flight');
  check(a1.code === 0 && a1.json?.data?.result === 'done', 'A\'s command did not finish normally');

  // 2. A's daemon dies while A's command runs (as in the report, where a
  // relay recovery stopped it); B starts right then.
  const inflight2 = cuAsync(A.agent, A.pane, slow(6000));
  await sleep(1500);
  const pidA = fs.readFileSync(path.join(socketDir, `${nameA}.pid`), 'utf8').trim();
  spawnSync('kill', ['-9', pidA]);
  step('killed A\'s daemon mid-command', { pid: pidA });
  await sleep(300);
  const C = { agent: 6003, pane: '%93' };
  const openC = cu(C.agent, C.pane, ['open', rig.site + '/c?C']);
  const nameC = sessionOf(C.agent, C.pane)?.name;
  step('C starts while A waits on its dead daemon', { ...summary(openC), session: nameC, tookOver: tookOver(nameC), createdA: createdOf(nameA)?.target_ids });
  const a2 = await inflight2;
  step('A command whose daemon died', summary(a2));
  check(a2.code !== 0, 'A exited 0 although its command was never confirmed');
  check(a2.json?.code === 'action_outcome_unknown', `A's unconfirmed command was not reported as action_outcome_unknown: ${a2.json?.code}`);
  check(!tookOver(nameC), 'C took over A\'s tabs while A still waited on its command');
  // Right after A's command ended, a new session still must not take them:
  // A ran a command moments ago.
  const D = { agent: 6004, pane: '%94' };
  const openD = cu(D.agent, D.pane, ['open', rig.site + '/d?D']);
  const nameD = sessionOf(D.agent, D.pane)?.name;
  step('D starts right after A\'s failure', { ...summary(openD), session: nameD, tookOver: tookOver(nameD), createdA: createdOf(nameA)?.target_ids });
  check(!tookOver(nameD), 'D took over A\'s tabs moments after A\'s command');
  const pageA = await rig.ev(`chrome.tabs.query({}).then(t => t.filter(x => (x.url || '').includes('/a?A')).length)`);
  check(pageA === 1, `A's page is not open any more (${pageA})`);

  // 3. What happened to A's last command, without running anything.
  const last = cu(A.agent, A.pane, ['session', 'last']);
  step('A session last', summary(last));
  check(last.code === 0 && last.json?.data?.lastCommand?.action === 'evaluate', 'session last did not report A\'s last command');
  check(last.json?.data?.lastCommand?.state === 'unknown', `A's interrupted command was not reported as unknown: ${last.json?.data?.lastCommand?.state}`);

  // 4. The relay drops while A's command runs.
  const reopenA = cu(A.agent, A.pane, ['tab', 'list']);
  step('A again', { ...summary(reopenA), live: { B: socketOf(nameB), C: socketOf(nameC), D: socketOf(nameD) } });
  const listedA = JSON.stringify(reopenA.json?.data?.tabs || []);
  check(!/\/b\?B|\/c\?C|\/d\?D/.test(listedA), `A's tab list shows other running sessions' tabs: ${listedA}`);
  const inflight3 = cuAsync(A.agent, A.pane, slow(6000));
  await sleep(1500);
  const hosts = nmHosts();
  for (const pid of hosts) spawnSync('kill', ['-9', pid]);
  step('stopped this rig\'s native host', { pids: hosts });
  const a3 = await inflight3;
  step('A command in flight when the relay dropped', summary(a3));
  check(a3.code !== 0 || a3.json?.success === true, 'A exited 0 without a confirmed result');
  const relayFile = path.join(socketDir, 'relay-cdp-url');
  const back = await waitFor(() => fs.existsSync(relayFile), { timeout: 60000, label: 'relay back' }).then(() => true, () => false);
  step('relay back', { back });
  check(back, 'the relay did not come back after the native host died');
  const lastAfterDrop = cu(A.agent, A.pane, ['session', 'last']);
  step('A session last after the drop', summary(lastAfterDrop));
  const title = await rig.ev(`chrome.tabs.query({}).then(t => t.filter(x => (x.url || '').includes('/a?A')).map(x => x.title))`);
  step('A page title (did the side effect run?)', { title });
  for (const s of [A, B, C, D]) cu(s.agent, s.pane, ['done']);
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
