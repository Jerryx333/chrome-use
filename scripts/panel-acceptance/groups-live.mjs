// Live check of the tab group lifecycle (CLI 1.5.242, ab-connect 0.5.77) on
// the build host (one browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node groups-live.mjs
// Chrome for Testing runs headless with its own profile, the unpacked
// extension and the native host in an isolated HOME (rig.mjs). Agents are
// plain `chrome-use` processes with the environment an agent CLI gives them:
// an agent id (CLAUDE_PID) that changes per task, a terminal id (TMUX_PANE)
// and a working directory that do not. Steps:
//   1. task A opens two tabs; its daemon idle-exits (3 s) with the tabs open;
//   2. task B, same pane and directory, new agent id: its session takes over
//      A's group: its tab list has A's tabs, a new tab joins the same group,
//      and there is still one ✦ group;
//   3. B ends with `done --keep <tab>`: only the kept tab is left, outside any
//      group and no longer the extension's; no ✦ group remains;
//   3b. task E idles out, then `done --keep <label>` reconnects and keeps
//      that tab; `done` again and `done --keep` with nothing open succeed;
//   4. task C (another pane) opens two tabs; the user switches to one and
//      back; C's daemon idle-exits;
//   5. task D starts with AGENT_BROWSER_GROUP_CLEANUP_MS=1000: its background
//      sweep closes C's untouched tab and leaves the touched one, ungrouped;
//   6. `tabs cleanup` with nothing left to do reports nothing and closes
//      nothing of the user's.
// Evidence goes to $OUT (default <work>/out).
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup, openTab } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/gl-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[groups] ${name}`, JSON.stringify(data).slice(0, 600)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok) { save(); throw new Error(message); } };

const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8950, cdpPort: 9411 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const project = path.join(work, 'proj');
fs.mkdirSync(project, { recursive: true });
const socketDir = path.join(rig.home, '.chrome-use');

// One agent command, as an agent CLI in `pane` would run it.
function cu(agent, pane, args, extra = {}) {
  const env = { ...process.env, HOME: rig.home, CHROME_USE_NO_UPDATE_CHECK: '1', CLAUDE_PID: String(agent), TMUX_PANE: pane, AGENT_BROWSER_IDLE_TIMEOUT_MS: '3000', ...extra };
  for (const k of ['AGENT_BROWSER_SESSION', 'CMUX_SURFACE_ID', 'CMUX_CLAUDE_PID', 'TERM_SESSION_ID', 'ITERM_SESSION_ID', 'GHOSTTY_SURFACE_ID', 'CODEX_THREAD_ID']) delete env[k];
  const r = spawnSync(rig.hostBin, ['--json', ...args], { cwd: project, env, encoding: 'utf8', timeout: 90000 });
  let json = null;
  try { json = JSON.parse(r.stdout.trim().split('\n').at(-1)); } catch {}
  return { code: r.status, json, stdout: r.stdout, stderr: r.stderr };
}
const ok = (r, what) => { check(r.code === 0 && r.json?.success !== false, `${what} failed: ${r.stdout} ${r.stderr}`); return r.json; };
const tabs = () => rig.ev('chrome.tabs.query({})');
const groups = () => rig.ev('chrome.tabGroups.query({})');
const agentGroups = async () => (await groups()).filter(g => /^✦/.test(g.title || ''));
const state = () => rig.ev(`chrome.storage.local.get('ab_owned_tabs').then(g => g.ab_owned_tabs || [])`);
const sessionOf = (agent, pane) => ok(cu(agent, pane, ['status']), 'status').data.currentSession;
const daemonGone = name => waitFor(() => !fs.existsSync(path.join(socketDir, `${name}.sock`)), { timeout: 30000, label: `daemon ${name} idle exit` });
// The row of a `tab list` reply whose url ends with `suffix`, wherever the
// reply nests its rows.
function rowFor(value, suffix) {
  if (Array.isArray(value)) { for (const v of value) { const r = rowFor(v, suffix); if (r) return r; } return null; }
  if (value && typeof value === 'object') {
    if (typeof value.url === 'string' && value.url.endsWith(suffix) && value.tabId) return value;
    for (const v of Object.values(value)) { const r = rowFor(v, suffix); if (r) return r; }
  }
  return null;
}
const byUrl = (list, suffix) => list.find(t => (t.url || t.pendingUrl || '').endsWith(suffix));

try {
  // The user's own tab, which nothing may close.
  const userTab = await openTab(rig, rig.site + '/a?user');

  // 1. Task A.
  ok(cu(1001, '%71', ['open', rig.site + '/b']), 'A open');
  ok(cu(1001, '%71', ['tab', 'new', rig.site + '/c']), 'A tab new');
  const A = sessionOf(1001, '%71');
  const groupsA = await agentGroups();
  step('task A opened two tabs', { session: A, groups: groupsA.map(g => g.title) });
  check(groupsA.length === 1 && groupsA[0].title === A.tabGroup, 'task A has one group named as status says');
  await daemonGone(A.name);
  const afterIdle = await tabs();
  check(byUrl(afterIdle, '/b') && byUrl(afterIdle, '/c'), 'idle exit kept A\'s tabs open');
  step('task A daemon idle-exited, tabs still open');

  // 2. Task B, same pane and directory, another agent.
  const listB = ok(cu(1002, '%71', ['tab', 'list']), 'B tab list');
  const B = sessionOf(1002, '%71');
  step('task B tab list', { session: B, tabs: listB.data });
  check(B.name !== A.name, 'B is its own session');
  check(B.tabGroup === A.tabGroup, `B continues A's group (${B.tabGroup} vs ${A.tabGroup})`);
  const listed = JSON.stringify(listB.data);
  check(listed.includes('/b') && listed.includes('/c'), 'B lists A\'s tabs');
  ok(cu(1002, '%71', ['tab', 'new', rig.site + '/d']), 'B tab new');
  const groupsB = await agentGroups();
  const tB = await tabs();
  step('task B opened a tab', { groups: groupsB.map(g => g.title) });
  check(groupsB.length === 1, 'still one agent group');
  check(['/b', '/c', '/d'].every(u => byUrl(tB, u)?.groupId === groupsB[0].id), 'A\'s tabs and B\'s new tab share the group');

  // 3. B is done and keeps /d.
  const listB2 = ok(cu(1002, '%71', ['tab', 'list']), 'B tab list 2').data;
  const keepId = rowFor(listB2, '/d')?.tabId;
  check(keepId, 'found /d in B\'s tab list');
  const done = ok(cu(1002, '%71', ['done', '--keep', keepId]), 'B done');
  await sleep(500);
  const tDone = await tabs();
  const kept = byUrl(tDone, '/d');
  const owned = await state();
  step('task B done', { reply: done.data, kept, agentGroups: (await agentGroups()).map(g => g.title), owned });
  check(!byUrl(tDone, '/b') && !byUrl(tDone, '/c'), 'done closed the tabs not kept');
  check(kept && kept.groupId === -1, 'kept tab is open and ungrouped');
  check(!owned.includes(kept.id), 'kept tab is no longer the extension\'s');
  check((await agentGroups()).length === 0, 'no agent group left');
  check(byUrl(tDone, '/a?user'), 'the user\'s tab is untouched');

  // 3b. Task E: `done --keep` after the daemon idled out reconnects and
  // keeps the named tab; `done` again, and `done --keep` with nothing open,
  // succeed and change nothing.
  ok(cu(4001, '%74', ['open', rig.site + '/b?e']), 'E open');
  ok(cu(4001, '%74', ['tab', 'new', '--label', 'keepme', rig.site + '/c?e']), 'E tab new');
  const E = sessionOf(4001, '%74');
  await daemonGone(E.name);
  const doneE = ok(cu(4001, '%74', ['done', '--keep', 'keepme']), 'E done --keep after idle exit');
  await sleep(500);
  const tE = await tabs();
  const again = ok(cu(4001, '%74', ['done']), 'E done again');
  const againKeep = ok(cu(4001, '%74', ['done', '--keep']), 'E done --keep with nothing open');
  step('task E done after idle exit', { reply: doneE.data, again: again.data, againKeep: againKeep.data, keptTab: byUrl(tE, '/c?e') });
  check(!byUrl(tE, '/b?e'), 'E\'s other tab was closed');
  check(byUrl(tE, '/c?e')?.groupId === -1, 'E\'s kept tab is open and ungrouped');
  check((await agentGroups()).length === 0, 'no agent group left after E');

  // 4. Task C in another pane; the user looks at one of its tabs.
  ok(cu(2001, '%72', ['open', rig.site + '/b?c1']), 'C open');
  ok(cu(2001, '%72', ['tab', 'new', rig.site + '/c?c2']), 'C tab new');
  const C = sessionOf(2001, '%72');
  const tC = await tabs();
  const c1 = byUrl(tC, '/b?c1'), c2 = byUrl(tC, '/c?c2');
  check(c1 && c2, 'C opened two tabs');
  await rig.ev(`chrome.tabs.update(${c2.id}, { active: true })`);
  await sleep(2500);
  await rig.ev(`chrome.tabs.update(${userTab.tabId}, { active: true })`);
  const touched = await rig.ev(`chrome.storage.session.get('ab_user_touched_tabs').then(g => g.ab_user_touched_tabs || [])`);
  step('user switched to a tab of C and back', { session: C.name, c1: c1.id, c2: c2.id, touched });
  check(touched.includes(c2.id) && !touched.includes(c1.id), 'the extension recorded only the tab the user switched to');
  await daemonGone(C.name);

  // 5. Task D in a third pane; its new daemon sweeps C.
  fs.rmSync(path.join(socketDir, '.group-sweep'), { force: true });
  ok(cu(3001, '%73', ['open', rig.site + '/a?d'], { AGENT_BROWSER_GROUP_CLEANUP_MS: '1000' }), 'D open');
  const swept = await waitFor(async () => { const t = await tabs(); return !byUrl(t, '/b?c1') && t; }, { timeout: 45000, label: 'C swept' });
  const c2After = byUrl(swept, '/c?c2');
  const keptRecord = path.join(socketDir, `${C.name}.kept-tabs.json`);
  step('sweep after D started', { c2: c2After, keptRecord: fs.existsSync(keptRecord) ? JSON.parse(fs.readFileSync(keptRecord, 'utf8')) : null, groups: (await groups()).map(g => g.title) });
  check(c2After && c2After.groupId === -1, 'the tab the user switched to is left, ungrouped');
  check(fs.existsSync(keptRecord), 'it is recorded as kept for the user');
  check(byUrl(swept, '/a?user') && byUrl(swept, '/d'), 'the user\'s tabs and B\'s kept tab are untouched');
  const D = sessionOf(3001, '%73');
  check(byUrl(swept, '/a?d')?.groupId >= 0, 'D\'s own tab is in its group, untouched');

  // 6. Manual cleanup with D still running: nothing of D's or the user's.
  const manual = ok(cu(3001, '%73', ['tabs', 'cleanup']), 'tabs cleanup');
  const tEnd = await tabs();
  step('tabs cleanup', { reply: manual.data, open: tEnd.map(t => t.url) });
  check(byUrl(tEnd, '/a?d') && byUrl(tEnd, '/a?user') && byUrl(tEnd, '/d') && byUrl(tEnd, '/c?c2') && byUrl(tEnd, '/c?e'), 'manual cleanup left the live session and the user\'s tabs');
  ok(cu(3001, '%73', ['done']), 'D done');
  step('PASS', { sessions: [A.name, B.name, C.name, D.name] });
  evidence.pass = true;
} catch (error) {
  step('FAIL', { error: String(error?.stack || error) });
  evidence.pass = false;
  process.exitCode = 1;
} finally {
  save();
  cleanup(rig);
  // The rig's sockets would keep node alive.
  process.exit(process.exitCode ?? 0);
}
