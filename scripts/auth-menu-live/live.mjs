// Live check: `auth login --bwu` on an App Store Connect-like page (a two-step
// sign-in in a same-site iframe) in an agent tab in the background of the
// user's window, with a password manager whose inline menu blocks the tab
// (fakepm/, modelled on Bitwarden 2026.10). Build host only, one browser run
// per host at a time:
//   HOST_BIN=<chrome-use> [BASE_BIN=<released chrome-use>] CHROME=<Chrome for Testing> \
//   EXT_DIR=<ab-connect> [HEADED=1] [FRONT_APP=cft|<app>] [AGENT_FRONT=1] node live.mjs
// AGENT_FRONT=1: instead of the login, fill and click on a sign-up form in an
// agent tab that is in front of the user's window (the user watching, #750).
// HEADED=1 runs a visible Chrome (on the build host's screen); run windows.swift
// beside it and screen-check.py afterwards to see what appeared on screen.
// Chrome for Testing runs headless with its own profile, the unpacked
// extension and the native host in an isolated HOME. The vault is not used:
// the login runs as the child half of `auth login --bwu` (the values come in
// through the CU_BWU_* environment, as `bwu run` would put them there).
// Checks, for HOST_BIN: the login is confirmed, the menu opened and was closed
// by another tab's activation, and the user's window kept its front tab with
// no tab activated in it. BASE_BIN only records what an older build did.
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { launch, api, waitFor, sleep } from '../inspect-acceptance/lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/aml-');
const out = process.env.OUT || path.join(work, 'out');
const home = path.join(work, 'home');
const profile = path.join(work, 'profile');
for (const d of [out, home, profile]) fs.mkdirSync(d, { recursive: true });
const EXT_ID = 'ciiljdlhdpfckdcfkphgmfalanpdejep';
const evidence = { work, hostBin: process.env.HOST_BIN, baseBin: process.env.BASE_BIN || null, runs: [] };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const failures = [];
const check = (ok, message) => { console.log(`${ok ? '✓' : '✗'} ${message}`); if (!ok) failures.push(message); };

// The fixture: the page adds the sign-in frame from another origin of the
// same site after 800 ms; the frame's one button reveals and focuses the
// password field, then signs in, and the page shows the app in place (as a
// single-page app does; a navigation in a background tab of this headless rig
// can stall, with or without the password manager).
const FRAME = fs.readFileSync(path.join(dir, 'signin-frame.html'), 'utf8');
const serve = (handler) => new Promise((resolve) => {
  const server = http.createServer(handler);
  server.listen(0, '127.0.0.1', () => resolve(server));
});
const frameServer = await serve((req, res) => { res.setHeader('Content-Type', 'text/html'); res.end(FRAME); });
const framePort = frameServer.address().port;
const topServer = await serve((req, res) => {
  res.setHeader('Content-Type', 'text/html');
  if (req.url.startsWith('/signup')) {
    // #750: a plain sign-up form in the top document; the password manager
    // opens its menu on each focused field.
    res.end(`<!doctype html><title>Sign up</title><form onsubmit="event.preventDefault(); document.title = 'created:' + handle.value + ':' + email.value">
<input id="handle" type="text" aria-label="Handle"> <input id="email" type="email" aria-label="Email"> <button id="create">Create</button></form>`);
  } else if (req.url.startsWith('/login')) {
    res.end(`<!doctype html><title>App Store Connect</title><div id="host"></div><script>
setTimeout(() => { const f = document.createElement('iframe'); f.src = 'http://idmsa.test.localhost:${framePort}/appleauth/auth/signin'; f.width = 480; f.height = 320; document.getElementById('host').append(f); }, 800);
addEventListener('message', (e) => { if (e.data && e.data.signedIn) { history.pushState({}, '', '/home'); document.body.innerHTML = '<h1>Apps</h1>'; } });
</script>`);
  } else {
    res.end('<!doctype html><title>Apps</title><h1>Apps</h1>');
  }
});
const origin = `http://app.test.localhost:${topServer.address().port}`;

// Native host with this run's HOME, for the extension's relay.
const nm = path.join(work, 'nm-host.sh');
fs.writeFileSync(nm, `#!/bin/sh\nexport HOME='${home}'\nexport CHROME_USE_NO_UPDATE_CHECK=1\nexec '${process.env.HOST_BIN}' __nm-host "$@"\n`, { mode: 0o755 });
fs.mkdirSync(path.join(profile, 'NativeMessagingHosts'), { recursive: true });
fs.writeFileSync(path.join(profile, 'NativeMessagingHosts', 'com.agent_browser.connect.json'), JSON.stringify({ name: 'com.agent_browser.connect', description: 'auth-menu-live host', path: nm, type: 'stdio', allowed_origins: [`chrome-extension://${EXT_ID}/`] }));
// NO_FAKEPM=1: no password manager, to tell its effects from the rig's.
const exts = [process.env.EXT_DIR, ...(process.env.NO_FAKEPM ? [] : [path.join(dir, 'fakepm')])].join(',');
const browser = await launch({ chrome: process.env.CHROME, port: 9433, userDataDir: profile, args: [...(process.env.HEADED ? [] : ['--headless=new']), `--load-extension=${exts}`, `--disable-extensions-except=${exts}`] });
evidence.chrome = browser.version;
const A = api(browser);
try {
  const ext = await A.worker();
  const inExt = (expr) => A.evaluate(ext.sid, expr);
  const pmTarget = process.env.NO_FAKEPM ? null : await waitFor(async () => (await A.targets()).find((t) => t.type === 'service_worker' && t.url.endsWith('/bg.js')), { timeout: 20000, label: 'fakepm worker' });
  const pm = pmTarget && await A.attach(pmTarget.targetId);
  const inPm = async (expr) => (pm ? A.evaluate(pm, expr) : { events: [] });
  await waitFor(() => fs.existsSync(path.join(home, '.chrome-use', 'relay-cdp-url')), { timeout: 30000, label: 'relay up' });
  // The user's window and the tab they are on.
  const user = await inExt(`chrome.tabs.query({ active: true, lastFocusedWindow: true }).then(([t]) => ({ tab: t.id, window: t.windowId }))`);
  evidence.user = user;

  const cli = (b, args, env = {}) => {
    const r = spawnSync(b, args, { env: { ...process.env, HOME: home, CHROME_USE_NO_UPDATE_CHECK: '1', ...env }, encoding: 'utf8', timeout: 120000 });
    return { code: r.status, stdout: r.stdout, stderr: r.stderr };
  };
  // AGENT_FRONT=1 (#750): the agent tab in front of the user's window, fill
  // and click on a sign-up form while the menu keeps opening on each field.
  const frontSignup = async (b, session) => {
    await inPm(`resetEvents().then(() => true)`);
    const opened = cli(b, ['--session', session, '--json', 'open', `${origin}/signup`, '--wait-until', 'none']);
    await waitFor(() => cli(b, ['--session', session, 'eval', 'document.readyState']).stdout.includes('complete'), { timeout: 20000, label: 'sign-up page loaded' });
    const agentTab = await inExt(`chrome.tabs.query({}).then((ts) => ts.find((t) => (t.url || t.pendingUrl || '').startsWith(${JSON.stringify(origin)})))`);
    await inExt(`chrome.tabs.update(${agentTab.id}, { active: true }).then(() => true)`);
    await sleep(500);
    await inPm(`resetEvents().then(() => true)`);
    const step = (args) => {
      const r = cli(b, ['--session', session, '--json', ...args]);
      let json = null;
      try { json = JSON.parse(r.stdout.trim().split('\n').pop()); } catch {}
      return { args, code: r.code, json, stderr: r.stderr.slice(-800) };
    };
    const steps = [
      step(['fill', '#handle', 'leo-agent']),
      step(['fill', '#email', 'agent@example.com']),
      step(['click', '#create']),
    ];
    await sleep(500);
    const title = step(['eval', 'document.title']);
    const events = (await inPm(`readEvents()`)).events || [];
    const front = await inExt(`chrome.tabs.query({ active: true, windowId: ${user.window} }).then(([t]) => t && t.id)`);
    const run = { label: 'front', open: opened.code, agentTab: { id: agentTab.id, window: agentTab.windowId }, steps, title, events, front };
    evidence.runs.push(run);
    save();
    for (const s of steps) console.log(`[front] ${s.args.join(' ')}: ${JSON.stringify(s.json).slice(0, 400)}`);
    check(agentTab.windowId === user.window, 'the agent tab is in the user\'s window');
    check(events.some((e) => e.kind === 'open'), 'the password manager opened its menu on a field');
    check(events.some((e) => e.kind === 'close'), 'the menu was closed without the user');
    check(steps.every((s) => s.json?.success === true), `fill, fill and click succeeded (${steps.map((s) => s.json?.error || 'ok').join(' | ').slice(0, 600)})`);
    check(String(title.json?.data?.result || '') === 'created:leo-agent:agent@example.com', `the form got both values (${JSON.stringify(title.json?.data)})`);
    check(front === agentTab.id, 'the agent tab is in front of the user\'s window again');
    check(!events.some((e) => e.kind === 'activated' && e.tab === user.tab), 'the user\'s own tab was never activated');
    cli(b, ['--session', session, 'close']);
  };
  const runs = [['new', process.env.HOST_BIN]];
  if (process.env.BASE_BIN) runs.unshift(['base', process.env.BASE_BIN]);
  for (const [label, b] of runs) {
    const session = `aml-${label}`;
    const run0 = {};
    if (process.env.AGENT_FRONT) {
      if (label !== 'new') continue;
      await frontSignup(b, session);
      continue;
    }
    await inPm(`resetEvents().then(() => true)`);
    const version = cli(b, ['--version']).stdout.trim();
    // `--wait-until none`: in this headless rig a relay `open` that waits for
    // `load` does not see it arrive (a navigation in an existing tab), though
    // the page loads; readiness is read from the page instead.
    const opened = cli(b, ['--session', session, '--json', 'open', `${origin}/login`, '--wait-until', 'none']);
    await waitFor(() => cli(b, ['--session', session, 'eval', 'document.readyState']).stdout.includes('complete'), { timeout: 20000, label: 'login page loaded' });
    // In this headless rig the sign-in frame of a background tab can stay
    // uncommitted (url "") for a long time; wait until it has loaded, so the
    // run checks the menu recovery rather than the rig's loading.
    const framed = await waitFor(() => cli(b, ['--session', session, '--json', 'frames']).stdout.includes('idmsa.test.localhost'), { timeout: 30000, label: 'sign-in frame loaded' }).catch(() => false);
    const agentTab = await inExt(`chrome.tabs.query({}).then((ts) => ts.find((t) => (t.url || t.pendingUrl || '').startsWith(${JSON.stringify(origin)})))`);
    // FRONT_APP=<app>: put another app in front first (the user working elsewhere).
    // FRONT_APP=cft: this Chrome for Testing in front, on the user's tab (the
    // user working in their own Chrome window).
    if (process.env.FRONT_APP === 'cft') {
      const userPage = (await browser.send('Target.getTargets')).targetInfos.find((t) => t.type === 'page' && t.url === 'about:blank');
      await browser.send('Page.bringToFront', {}, await A.attach(userPage.targetId));
      await sleep(1000);
    } else if (process.env.FRONT_APP) { spawnSync('open', ['-a', process.env.FRONT_APP]); await sleep(1000); }
    await sleep(300);
    run0.loginStartedAt = Date.now();
    const login = cli(b, ['--session', session, '--json', 'auth', 'login', '--bwu'], {
      CU_BWU_CHILD: '1', CU_BWU_USER: 'user@example.com', CU_BWU_PW: 'super-secret',
      CU_BWU_ORIGIN: origin, CU_BWU_ITEM: 'fixture', CU_BWU_ITEM_ID: 'fixture-id',
    });
    const events = (await inPm(`readEvents()`)).events || [];
    const front = await inExt(`chrome.tabs.query({ active: true, windowId: ${user.window} }).then(([t]) => t && t.id)`);
    const windows = await inExt(`chrome.windows.getAll({ populate: true }).then((ws) => ws.map((w) => ({ id: w.id, focused: w.focused, tabs: w.tabs.map((t) => ({ id: t.id, active: t.active, url: t.url })) })))`);
    const tabList = cli(b, ['--session', session, '--json', 'tab', 'list']).stdout;
    let result = null;
    try { result = JSON.parse(login.stdout.trim().split('\n').pop()); } catch {}
    const run = { ...run0, loginEndedAt: Date.now(), tabList: tabList.slice(0, 3000), label, version, framed, open: { code: opened.code, stdout: opened.stdout.slice(-1500), stderr: opened.stderr.slice(-1500) }, agentTab: agentTab && { id: agentTab.id, window: agentTab.windowId, active: agentTab.active }, login: { code: login.code, result, stderr: login.stderr.slice(-2000) }, events, front, windows };
    evidence.runs.push(run);
    save();
    if (opened.code !== 0) console.log(`[${label}] open failed:`, opened.stdout.slice(-800), opened.stderr.slice(-800));
    console.log(`[${label}] ${version}: exit ${login.code}`, JSON.stringify(result).slice(0, 600));
    if (label === 'new') {
      check(agentTab && agentTab.windowId === user.window && !agentTab.active, 'the agent tab is a background tab in the user\'s window');
      check(events.some((e) => e.kind === 'open'), 'the password manager opened its menu during the login');
      check(events.some((e) => e.kind === 'close' && e.why === 'elsewhere'), 'the menu was closed by a tab activated in another window');
      check(result?.success === true && result.data?.signedIn === true, `the login was confirmed (${result?.error || ''})`);
      check(front === user.tab, 'the user\'s window kept its front tab');
      check(!events.some((e) => e.kind === 'activated' && e.window === user.window), 'no tab was activated in the user\'s window');
      let remembered = null;
      try { remembered = JSON.parse(fs.readFileSync(path.join(home, '.chrome-use', 'bwu-items.json'), 'utf8')); } catch {}
      run.remembered = remembered;
      check(remembered?.hosts?.['app.test.localhost']?.id === 'fixture-id', 'the confirmed login remembered its vault item for the host');
    }
    cli(b, ['--session', session, 'close']);
    if (label === 'new') {
      await sleep(500);
      const left = await inExt(`chrome.windows.getAll({ populate: true }).then((ws) => ws.map((w) => ({ id: w.id, state: w.state, tabs: w.tabs.length })))`);
      run.windowsAfterClose = left;
      save();
      check(left.length === 1 && left[0].id === user.window, `closing the session left only the user's window (${JSON.stringify(left)})`);
    }
  }
} finally {
  save();
  browser.close();
  frameServer.close();
  topServer.close();
  // Daemons this run started, then anything else running from its directory.
  for (const b of [process.env.HOST_BIN, process.env.BASE_BIN].filter(Boolean)) {
    spawnSync(b, ['close', '--all', '--force'], { env: { ...process.env, HOME: home }, stdio: 'ignore', timeout: 30000 });
  }
  spawnSync('pkill', ['-f', `^${work}/`], { stdio: 'ignore' });
}
console.log(`evidence: ${path.join(out, 'evidence.json')}`);
process.exit(failures.length ? 1 : 0);
