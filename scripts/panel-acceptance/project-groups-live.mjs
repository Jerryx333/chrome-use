// Live check of project tab groups (CLI 1.5.243, ab-connect 0.5.77) on the
// build host (one browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node project-groups-live.mjs
// Chrome for Testing runs headless with its own profile, the unpacked
// extension and the native host in an isolated HOME (rig.mjs). Agents are
// plain `chrome-use` processes with an agent id (CLAUDE_PID) and a terminal
// id (TMUX_PANE) of their own, working in a git repository. Steps:
//   1. two terminals in one repository share one `✦ <repo>` group;
//   2. while both run, neither takes the other's tabs: the second one's
//      `open` of a page the first one has open opens its own tab;
//   3. one session's `open` of a page it already has switches to that tab;
//   4. `done` in one session closes only its own tabs;
//   5. once the last session of the project idles out (30 s here), its
//      tabs leave the group, so Chrome keeps no saved group (#692);
//   6. the project's next session takes the ended session's tabs over,
//      groups them again and reuses its tab for the same page instead of
//      opening another;
//   7. a session with a per-session group from an older build moves its tab
//      under its project's group;
//   7b. sessions that only read, did nothing, or were killed leave no blank
//      tab or empty group behind;
//   8. with the extension relay unreachable and a process list that cannot
//      see Chrome (a sandbox), no second Chrome is started; with one that
//      can, chrome-use decides to start it (the rig has no profile to start).
// Evidence goes to $OUT (default <work>/out).
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup, openTab } from './rig.mjs';
import { waitFor, sleep } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/pg-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[project] ${name}`, JSON.stringify(data).slice(0, 600)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok) { save(); throw new Error(message); } };

const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8960, cdpPort: 9421 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const repo = path.join(work, 'repo');
const sub = path.join(repo, 'packages', 'web');
fs.mkdirSync(path.join(repo, '.git'), { recursive: true });
fs.mkdirSync(sub, { recursive: true });
const other = path.join(work, 'older');
fs.mkdirSync(path.join(other, '.git'), { recursive: true });
const socketDir = path.join(rig.home, '.chrome-use');

function cu(agent, pane, args, { cwd = repo, extra = {} } = {}) {
  const env = { ...process.env, HOME: rig.home, CHROME_USE_NO_UPDATE_CHECK: '1', CLAUDE_PID: String(agent), TMUX_PANE: pane, AGENT_BROWSER_IDLE_TIMEOUT_MS: '30000', ...extra };
  for (const k of ['AGENT_BROWSER_SESSION', 'CMUX_SURFACE_ID', 'CMUX_CLAUDE_PID', 'TERM_SESSION_ID', 'ITERM_SESSION_ID', 'GHOSTTY_SURFACE_ID', 'CODEX_THREAD_ID']) if (!(k in extra)) delete env[k];
  const r = spawnSync(rig.hostBin, ['--json', ...args], { cwd, env, encoding: 'utf8', timeout: 120000 });
  let json = null;
  try { json = JSON.parse(r.stdout.trim().split('\n').at(-1)); } catch {}
  return { code: r.status, json, stdout: r.stdout, stderr: r.stderr };
}
const ok = (r, what) => { check(r.code === 0 && r.json?.success !== false, `${what} failed: ${r.stdout} ${r.stderr}`); return r.json; };
const tabs = () => rig.ev('chrome.tabs.query({})');
const agentGroups = async () => (await rig.ev('chrome.tabGroups.query({})')).filter(g => /^✦/.test(g.title || ''));
const sessionOf = (agent, pane, opts) => ok(cu(agent, pane, ['status'], opts), 'status').data.currentSession;
const daemonGone = name => waitFor(() => !fs.existsSync(path.join(socketDir, `${name}.sock`)), { timeout: 70000, label: `daemon ${name} idle exit` });
const byUrl = (list, suffix) => list.filter(t => (t.url || t.pendingUrl || '').endsWith(suffix));
const urlOf = (agent, pane, opts) => ok(cu(agent, pane, ['get', 'url'], opts), 'get url').data;

try {
  const userTab = await openTab(rig, rig.site + '/a?user');

  // 1. Two terminals, one repository (one of them in a subfolder).
  ok(cu(1001, '%81', ['open', rig.site + '/b']), 'A open');
  ok(cu(2001, '%82', ['open', rig.site + '/c'], { cwd: sub }), 'B open');
  const A = sessionOf(1001, '%81');
  const B = sessionOf(2001, '%82', { cwd: sub });
  const g1 = await agentGroups();
  const t1 = await tabs();
  step('two terminals in one repository', { A: A.name, B: B.name, groupA: A.tabGroup, groupB: B.tabGroup, groups: g1.map(g => g.title) });
  check(A.name !== B.name, 'two sessions');
  check(A.tabGroup === '✦ repo' && B.tabGroup === '✦ repo', 'both name the project group');
  check(g1.length === 1 && byUrl(t1, '/b')[0]?.groupId === g1[0].id && byUrl(t1, '/c')[0]?.groupId === g1[0].id, 'their tabs share one group');

  // 2. B opens the page A has open: its own tab, A keeps its own.
  const bNew = ok(cu(2001, '%82', ['open', '--new-tab', rig.site + '/e'], { cwd: sub }), 'B open --new-tab');
  const tNew = (await tabs()).map(t => `${t.id} ${t.groupId} ${t.url}`);
  const bOpen = ok(cu(2001, '%82', ['open', rig.site + '/b'], { cwd: sub }), 'B open A\'s page');
  const t2 = await tabs();
  step('debug', { t1: t1.map(t => `${t.id} ${t.groupId} ${t.url}`), bNew: bNew.data, tNew, bOpen: bOpen.data, t2: t2.map(t => `${t.id} ${t.groupId} ${t.url}`) });
  const listB = JSON.stringify(ok(cu(2001, '%82', ['tab', 'list'], { cwd: sub }), 'B tab list').data);
  const aUrl = urlOf(1001, '%81');
  step('B opened the page A has', { bTabs: byUrl(t2, '/b').length, aUrl, listB: listB.slice(0, 300) });
  check(byUrl(t2, '/b').length === 2, 'B opened its own tab for the page A holds');
  check(String(aUrl?.url ?? aUrl).endsWith('/b'), 'A still drives its own tab');
  check(!listB.includes('/d'), 'B does not list A\'s tabs');

  // 3. A opens another page, then the first again: no duplicate.
  ok(cu(1001, '%81', ['open', '--new-tab', rig.site + '/d']), 'A open --new-tab');
  const before = byUrl(await tabs(), '/b').length;
  const reuse = ok(cu(1001, '%81', ['open', rig.site + '/b#section']), 'A open same page again');
  const after = byUrl(await tabs(), '/b').length;
  step('A opened a page it already has', { reply: reuse.data, before, after });
  check(reuse.data.reusedTab === true && after === before, 'A switched to its tab instead of opening another');

  // 4. B is done: only B's tabs close.
  const doneB = ok(cu(2001, '%82', ['done'], { cwd: sub }), 'B done');
  const t4 = await tabs();
  step('B done', { closed: doneB.data.tabsClosed?.map(t => t.url), open: t4.map(t => t.url) });
  check(byUrl(t4, '/c').length === 0 && byUrl(t4, '/e').length === 0, 'B\'s tabs closed');
  check(byUrl(t4, '/b').length === 1 && byUrl(t4, '/d').length === 1, 'A\'s tabs untouched');

  // 5. A idles out: nobody works in the group any more. Its tabs leave the
  //    group, so Chrome keeps no saved group when the window closes (#692).
  await daemonGone(A.name);
  await waitFor(async () => (await agentGroups()).length === 0, { timeout: 15000, label: 'group gone' });
  const t5 = await tabs();
  step('last session idled out', { tabs: t5.map(t => `${t.groupId} ${t.url}`) });
  check(byUrl(t5, '/b').length === 1 && byUrl(t5, '/d').length === 1, 'A\'s tabs stay open');
  check([...byUrl(t5, '/b'), ...byUrl(t5, '/d')].every(t => t.groupId === -1), 'outside any group');

  // 6. The project's next session takes A's tabs over and reuses A's tab.
  //    A ran a command moments ago, so it holds its lease for LEASE_GRACE
  //    (#687): age its activity stamps past it.
  const aged = new Date(Date.now() - 5 * 60 * 1000);
  for (const f of [`${A.name}.activity`, `${A.name}.created-targets.json`]) {
    const p = path.join(socketDir, f);
    if (fs.existsSync(p)) fs.utimesSync(p, aged, aged);
  }
  ok(cu(3001, '%83', ['open', rig.site + '/b']), 'C open the page A left');
  const C = sessionOf(3001, '%83');
  const t6 = await tabs();
  const listC = JSON.stringify(ok(cu(3001, '%83', ['tab', 'list']), 'C tab list').data);
  step('next session of the project', { C: C.name, group: C.tabGroup, bTabs: byUrl(t6, '/b').length, listC: listC.slice(0, 300) });
  check(byUrl(t6, '/b').length === 1, 'C reused A\'s tab for the same page');
  check(listC.includes('/d'), 'C took over A\'s other tab too');
  const g6 = await agentGroups();
  check(g6.length === 1 && g6[0].title === '✦ repo' && [...byUrl(t6, '/b'), ...byUrl(t6, '/d')].every(t => t.groupId === g6[0].id), 'C put the tabs it took over back in the project group');
  check(fs.existsSync(path.join(socketDir, `${A.name}.moved-to`)) || !fs.existsSync(path.join(socketDir, `${A.name}.created-targets.json`)), 'A\'s record moved to C');
  ok(cu(3001, '%83', ['done']), 'C done');

  // 7. A session from an older build: tabs in a per-session group.
  ok(cu(4001, '%84', ['open', rig.site + '/a?old'], { cwd: other }), 'L open');
  const L = sessionOf(4001, '%84', { cwd: other });
  await daemonGone(L.name);
  fs.rmSync(path.join(socketDir, `${L.name}.group`));
  // An older build left its tab in a group titled after the session.
  const legacyTab = byUrl(await tabs(), '/a?old')[0];
  await rig.ev(`chrome.tabs.group({ tabIds: ${legacyTab.id} }).then(g => chrome.tabGroups.update(g, { title: ${JSON.stringify('✦ ' + L.name)} }))`);
  const listL = JSON.stringify(ok(cu(4001, '%84', ['tab', 'list'], { cwd: other }), 'L tab list after upgrade').data);
  const g7 = await agentGroups();
  const oldTab = byUrl(await tabs(), '/a?old')[0];
  step('legacy session migrated', { listL: listL.slice(0, 300), groups: g7.map(g => g.title), pointer: fs.existsSync(path.join(socketDir, `${L.name}.group`)) });
  check(listL.includes('/a?old'), 'the session still has its tab');
  check(g7.some(g => g.title === '✦ older' && g.id === oldTab.groupId), 'its tab is under the project group');
  check(!g7.some(g => g.title === '✦ ' + L.name), 'no per-session group left');
  ok(cu(4001, '%84', ['done'], { cwd: other }), 'L done');

  // 7b. Sessions that never loaded a page leave nothing behind: one that
  // only read (`get title`), one that did nothing but list its tabs, and one
  // whose daemon was killed (no idle exit), cleaned by the next daemon's sweep.
  const mk = name => { const d = path.join(work, name); fs.mkdirSync(path.join(d, '.git'), { recursive: true }); return d; };
  const readOnly = mk('readonly'), idleOnly = mk('nothing'), crashed = mk('crashed'), next = mk('next');
  ok(cu(6001, '%86', ['get', 'title'], { cwd: readOnly }), 'R get title');
  ok(cu(6002, '%87', ['tab', 'list'], { cwd: idleOnly }), 'N tab list');
  ok(cu(6003, '%88', ['get', 'url'], { cwd: crashed }), 'K get url');
  const R = sessionOf(6001, '%86', { cwd: readOnly }), N = sessionOf(6002, '%87', { cwd: idleOnly }), K = sessionOf(6003, '%88', { cwd: crashed });
  const groupsBefore = (await agentGroups()).map(g => g.title);
  const kPid = Number(fs.readFileSync(path.join(socketDir, `${K.name}.pid`), 'utf8'));
  process.kill(kPid, 'SIGKILL');
  await daemonGone(R.name);
  await daemonGone(N.name);
  const afterIdle = (await agentGroups()).map(g => g.title);
  fs.rmSync(path.join(socketDir, '.group-sweep'), { force: true });
  ok(cu(6004, '%89', ['open', rig.site + '/a?next'], { cwd: next }), 'Z open');
  const swept = await waitFor(async () => { const g = (await agentGroups()).map(g => g.title); return !g.includes('✦ crashed') && g; }, { timeout: 30000, label: 'crashed session swept' });
  const blanks = (await tabs()).filter(t => !t.url || t.url === 'about:blank');
  step('sessions that never loaded a page', { groupsBefore, afterIdle, swept, blanks: blanks.map(t => `${t.id} ${t.groupId}`) });
  check(groupsBefore.includes('✦ readonly') && groupsBefore.includes('✦ nothing') && groupsBefore.includes('✦ crashed'), 'each of them had opened a blank tab in its group');
  check(!afterIdle.includes('✦ readonly') && !afterIdle.includes('✦ nothing'), 'their idle exit closed the blank tab and the group went with it');
  check(blanks.every(t => t.groupId === -1), 'no agent group holds a blank tab');
  ok(cu(6004, '%89', ['done'], { cwd: next }), 'Z done');

  // 8. Relay unreachable, process list that cannot see Chrome: no Chrome start.
  const fake = path.join(work, 'fakebin');
  fs.mkdirSync(fake, { recursive: true });
  const openLog = path.join(work, 'open.log');
  fs.writeFileSync(path.join(fake, 'open'), `#!/bin/sh\necho "$@" >> ${openLog}\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(fake, 'pkill'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  const nmDir = path.join(rig.home, 'Library', 'Application Support', 'Google', 'Chrome', 'NativeMessagingHosts');
  fs.mkdirSync(nmDir, { recursive: true });
  fs.copyFileSync(path.join(work, 'chrome-profile', 'NativeMessagingHosts', 'com.agent_browser.connect.json'), path.join(nmDir, 'com.agent_browser.connect.json'));
  const hidden = path.join(work, 'relay-hidden');
  fs.mkdirSync(hidden, { recursive: true });
  const relayFiles = fs.readdirSync(socketDir).filter(f => f.startsWith('relay-cdp-url'));
  for (const f of relayFiles) fs.renameSync(path.join(socketDir, f), path.join(hidden, f));
  const guard = pgrep => {
    fs.writeFileSync(path.join(fake, 'pgrep'), `#!/bin/sh\n${pgrep}\n`, { mode: 0o755 });
    fs.rmSync(openLog, { force: true });
    const r = cu(5001, '%85', ['open', rig.site + '/a?guard'], { extra: { PATH: `${fake}:${process.env.PATH}`, AGENT_BROWSER_SESSION: 'guard' } });
    return { stderr: r.stderr.slice(-400), opened: fs.existsSync(openLog) ? fs.readFileSync(openLog, 'utf8') : '' };
  };
  // A sandbox: pgrep sees nothing, not even launchd.
  const sandboxed = guard('exit 1');
  // Control: pgrep sees launchd and no Chrome — Chrome would be started.
  const visible = guard('[ "$2" = launchd ] && exit 0\nexit 1');
  for (const f of relayFiles) fs.renameSync(path.join(hidden, f), path.join(socketDir, f));
  step('Chrome start guard', { relayFiles, sandboxed, visible });
  check(sandboxed.opened === '' && /cannot tell/.test(sandboxed.stderr), 'no Chrome start when the process list is hidden');
  // The rig's HOME has no Chrome profile to start, so the control stops at
  // "no profile could be started" — after deciding to start Chrome.
  check(visible.opened.includes('-na') || /Chrome is not running/.test(visible.stderr), 'control: with Chrome provably not running it would start one');
  check(byUrl(await tabs(), '/a?user').length === 1, 'the user\'s tab is untouched');

  step('PASS', { sessions: [A.name, B.name, C.name, L.name] });
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
