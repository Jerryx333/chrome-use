// Inspect & Ask matching acceptance (build host): page ↔ project ↔ session.
// Same rig as accept.mjs (Chrome for Testing, acceptance extension copy, the
// branch host in an isolated HOME, fake ocs) plus fixtures from
// setup-matching.sh. Each scenario rewrites the fake roster and, when needed,
// the bindings file, then drives the panel like a user. Exit non-zero on any
// failure; results in results-matching.json.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { launch, api, waitFor, sleep, byText } from './lib.mjs';

const dir = path.dirname(new URL(import.meta.url).pathname);
const HOME = path.join(dir, 'home');
const BIN = path.join(dir, 'bin', 'current');
const OCS = path.join(dir, 'fake-ocs');
const LOG = path.join(dir, 'fake-ocs.log');
const ROSTER = path.join(dir, 'fake-roster.json');
const BINDINGS = path.join(HOME, '.chrome-use', 'inspect', 'bindings.json');
const only = process.env.ONLY ? process.env.ONLY.split(',') : null;
const results = { startedAt: new Date().toISOString(), host: fs.realpathSync(BIN), scenarios: [] };

const D = name => fs.realpathSync(path.join(dir, name));
const S = (id, name, cwd, kind = 'claude') => (kind === 'claude' ? { kind, id, name, cwd, status: 'idle', self: false } : { kind, target: id, threadId: 't', cwd, self: false });
const setRoster = entries => fs.writeFileSync(ROSTER, JSON.stringify(entries));
const resetBindings = () => fs.rmSync(BINDINGS, { force: true });
const dms = () => { try { return fs.readFileSync(LOG, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse).filter(x => x.cmd === 'dm'); } catch { return []; } };
const cli = (args, self) => {
  const r = spawnSync(BIN, ['inspect', ...args, '--json'], { env: { ...process.env, HOME, CHROME_USE_INSPECT_OCS: OCS, FAKE_OCS_SELF: self || '' }, encoding: 'utf8', timeout: 30000 });
  let json = null;
  try { json = JSON.parse(r.stdout.trim().split('\n').at(-1)); } catch {}
  return { code: r.status, stdout: r.stdout, stderr: r.stderr, json };
};

function serveFile(port, pick) {
  return http.createServer((req, res) => {
    const file = pick(new URL(req.url, 'http://x').pathname);
    if (!file || !fs.existsSync(file)) { res.writeHead(404); return res.end('nf'); }
    res.writeHead(200, { 'content-type': file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    fs.createReadStream(file).pipe(res);
  }).listen(port, '127.0.0.1');
}
const staticDir = root => p => { const f = path.join(root, decodeURIComponent(p)); return f.startsWith(root) && fs.existsSync(f) && fs.statSync(f).isFile() ? f : path.join(root, 'index.html'); };
serveFile(8899, staticDir(path.join(dir, 'react-app/dist-test')));
serveFile(8894, p => path.join(dir, 'mono/apps', p.startsWith('/shop') ? 'shop' : 'admin', 'index.html'));
serveFile(8893, () => path.join(dir, 'wild/index.html'));
serveFile(8892, () => path.join(dir, 'rev-site/index.html'));

fs.rmSync(LOG, { force: true });
fs.writeFileSync(path.join(dir, 'fake-ocs-mode'), 'sent');
const userDataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'inspect-accept-'));
fs.mkdirSync(path.join(userDataDir, 'NativeMessagingHosts'));
fs.writeFileSync(path.join(userDataDir, 'NativeMessagingHosts', 'com.agent_browser.connect.json'), JSON.stringify({ name: 'com.agent_browser.connect', description: 'acceptance', path: path.join(dir, 'nm-host.sh'), type: 'stdio', allowed_origins: ['chrome-extension://ciiljdlhdpfckdcfkphgmfalanpdejep/'] }));
const b = await launch({
  chrome: process.env.CHROME, port: 9345, userDataDir,
  args: ['--headless=new', `--load-extension=${path.join(dir, 'ext-acc')}`, `--disable-extensions-except=${path.join(dir, 'ext-acc')}`,
    '--host-resolver-rules=MAP test.inspect.example 127.0.0.1:8899, MAP apps.inspect.example 127.0.0.1:8894, MAP *.wild.inspect.example 127.0.0.1:8893, MAP rev.inspect.example 127.0.0.1:8892'],
  env: { CHROME_USE_INSPECT_OCS: OCS },
});
process.on('exit', () => b.close());
results.chrome = b.version;
const A = api(b);
const sw = await A.worker();
// These scenarios cover the terminal destination (ocs); the side-panel
// destination, the default since 0.5.68, has its own live check.
const terminalFor = tabId => `(async () => {
  let origin = null;
  for (let i = 0; i < 50 && !origin; i++) {
    const t = await chrome.tabs.get(${tabId});
    try { origin = new URL(t.url || t.pendingUrl).origin; } catch { await new Promise(r => setTimeout(r, 100)); }
  }
  const s = await chrome.storage.local.get('inspectDestinations');
  await chrome.storage.local.set({ inspectDestinations: { ...(s.inspectDestinations || {}), [origin]: 'terminal' } });
})()`;
const hook = expr => A.evaluate(sw.sid, `globalThis.__chromeUseAcceptance.${expr}`);
await waitFor(() => hook('inspectState(-1)').then(s => s.hostReady), { timeout: 30000, label: 'native host' });
results.resolveReady = await hook('inspectState(-1)').then(s => s.resolveReady);
await sleep(3000);

const opened = [];
async function scenario(name, fn) {
  if (only && !only.includes(name)) return;
  for (const page of opened.splice(0)) await b.send('Target.closeTarget', { targetId: page.targetId }).catch(() => {});
  const started = Date.now();
  try {
    const detail = await Promise.race([fn(), new Promise((_, reject) => setTimeout(() => reject(new Error('scenario timed out after 150 s')), 150000))]);
    results.scenarios.push({ name, ok: true, ms: Date.now() - started, detail });
    console.log('PASS', name);
  } catch (error) {
    let frameText = null;
    try { frameText = lastCtx ? await A.evaluate(lastCtx.frame.sid, 'document.getElementById("app").innerText') : null; } catch {}
    results.scenarios.push({ name, ok: false, ms: Date.now() - started, error: String(error.stack || error), frameText });
    console.log('FAIL', name, String(error.message || error).slice(0, 300));
  }
}

let enabled = false;
let lastCtx = null;
// Open a page, start the picker as the shortcut would, pick, return the frame.
async function pickOn(url, selector, existing) {
  const page = existing || await A.openPage(url);
  if (!existing) opened.push(page);
  await b.send('Page.bringToFront', {}, page.sid);
  // Identify the tab by a marker in this page, not by URL (tabs can share one).
  const marker = 'm' + Math.random().toString(36).slice(2);
  await A.evaluate(page.sid, `document.title = ${JSON.stringify(marker)}; 1`);
  const tabId = await waitFor(() => A.evaluate(sw.sid, `chrome.tabs.query({}).then(ts => ts.find(t => t.title === ${JSON.stringify(marker)})?.id)`), { label: 'tab ' + url });
  // A dev server that just started may reload the page once (dependency
  // pre-bundling); retry on an error page instead of failing on that race.
  let started = null;
  for (let attempt = 0; attempt < 4; attempt++) {
    await A.evaluate(sw.sid, terminalFor(tabId));
    started = await A.evaluate(sw.sid, `globalThis.__chromeUseAcceptance.startInspect(${tabId}).then(r => JSON.stringify(r), e => 'ERR ' + e.message)`);
    if (started === '{"started":true}' || !/error page|http\(s\) pages only/.test(started)) break;
    await sleep(1500);
    await b.send('Page.reload', {}, page.sid).catch(() => {});
    await waitFor(() => A.evaluate(page.sid, 'document.readyState === "complete" && location.protocol.startsWith("http")'), { label: 'page ready' }).catch(() => {});
    await A.evaluate(page.sid, `document.title = ${JSON.stringify(marker)}; 1`).catch(() => {});
  }
  assert.equal(started, '{"started":true}', started);
  await waitFor(() => A.evaluate(sw.sid, `chrome.tabs.query({ active: true }).then(ts => ts.some(t => t.id === ${tabId}))`), { label: 'active' }).catch(() => {});
  const c = await A.center(page.sid, selector);
  await A.click(page.sid, c.x, c.y);
  const frame = await A.realFrame();
  if (!enabled && /Enable Inspect & Ask/.test(await text(frame))) { await A.clickInFrame(page.sid, frame.sid, byText('Enable Inspect & Ask')); enabled = true; }
  enabled = true;
  await waitFor(async () => /→|Send requests from|Choose where|No running agent|asks to take over|Target: not confirmed/.test(await text(frame)), { label: 'matching state' });
  lastCtx = { page, frame, tabId };
  return lastCtx;
}
// The panel shows the home folder as ~ (shortPath in inspect-frame-logic.js).
const tilde = p => p.replace(/^\/(?:Users|home)\/[^/]+/, '~');
const text = frame => A.evaluate(frame.sid, 'document.getElementById("app").innerText');
async function send(ctx, words) {
  const before = dms().length;
  await A.clickInFrame(ctx.page.sid, ctx.frame.sid, 'document.getElementById("instruction")');
  await b.send('Input.insertText', { text: words }, ctx.page.sid);
  await waitFor(() => A.evaluate(ctx.frame.sid, '!document.getElementById("send").disabled'), { label: 'Send enabled' });
  await A.clickInFrame(ctx.page.sid, ctx.frame.sid, 'document.getElementById("send")');
  await waitFor(() => dms().length === before + 1, { label: 'dm for ' + words });
  return dms().at(-1);
}
async function tryEnterWithoutTarget(ctx) {
  const before = dms().length;
  await A.clickInFrame(ctx.page.sid, ctx.frame.sid, 'document.getElementById("instruction")');
  await b.send('Input.insertText', { text: 'should not send' }, ctx.page.sid);
  await A.key(ctx.page.sid, 'Enter', 'Enter', 13, '\r');
  await sleep(1500);
  assert.equal(dms().length, before, 'nothing delivered without a confirmed target');
}
const click = async (ctx, label) => {
  await A.clickInFrame(ctx.page.sid, ctx.frame.sid, byText(label));
  if (label === 'Use for this site') await waitText(ctx, /→/, 'bound after Use for this site');
  if (label === 'This tab only') await waitText(ctx, /this tab only/, 'tab target set');
};
async function waitText(ctx, re, label) { await waitFor(async () => re.test(await text(ctx.frame)), { label }); return text(ctx.frame); }

// Run with LEGACY_HOST=1 and bin/current pointing at a v1.5.213 host: the new
// extension falls back to the earlier two-step chooser.
await scenario('m14-new-extension-old-host', async () => {
  if (!process.env.LEGACY_HOST) return { skipped: 'set LEGACY_HOST=1 with an older host' };
  assert.equal(results.resolveReady, false, 'old host has no inspectResolve');
  setRoster([S('claude-aaaa1111', 'react-main', D('react-app'))]);
  const page = await A.openPage('http://test.inspect.example/?legacy');
  opened.push(page);
  const tabId = await waitFor(() => A.evaluate(sw.sid, `chrome.tabs.query({}).then(ts => ts.find(t => (t.url || '').endsWith('?legacy') && t.status === 'complete')?.id)`), { label: 'tab' });
  await A.evaluate(sw.sid, terminalFor(tabId));
  await A.evaluate(sw.sid, `globalThis.__chromeUseAcceptance.startInspect(${tabId})`);
  const c = await A.center(page.sid, '.save-button');
  await A.click(page.sid, c.x, c.y);
  const frame = await A.realFrame();
  if (/Enable Inspect & Ask/.test(await text(frame))) await A.clickInFrame(page.sid, frame.sid, byText('Enable Inspect & Ask'));
  await waitFor(async () => /Which project is/.test(await text(frame)), { label: 'legacy chooser' });
  return { ok: true };
});
if (process.env.LEGACY_HOST) { results.finishedAt = new Date().toISOString(); fs.writeFileSync(path.join(dir, 'results-legacy-host.json'), JSON.stringify(results, null, 2)); const failed = results.scenarios.filter(s => !s.ok); console.log(`${results.scenarios.length - failed.length}/${results.scenarios.length} legacy passed`); b.close(); process.exit(failed.length ? 1 : 0); }

const react = D('react-app'), wt2 = D('react-app-wt2'), mono = D('mono'), vue = D('vue-app'), wild = D('wild');

await scenario('m01-local-dev-unique-session', async () => {
  resetBindings();
  setRoster([S('claude-aaaa1111', 'react-main', react)]);
  const ctx = await pickOn('http://localhost:5173/?m1', '.save-button');
  const t = await waitText(ctx, /Send requests from localhost:5173 to react-main/, 'preselected card');
  assert.match(t, /dev server/);
  await click(ctx, 'Use for this site');
  await waitText(ctx, /→\s*react-main/, 'bound target');
  const second = await pickOn('http://localhost:5173/?m1b', '.save-button');
  const t2 = await text(second.frame);
  assert.doesNotMatch(t2, /Use for this site/, 'second visit needs no confirmation');
  assert.equal((await send(second, 'm1 change')).target, 'claude-aaaa1111');
  return { ok: true };
});

await scenario('m02-port-reused-by-another-worktree', async () => {
  // Restart the dev server from the second worktree on the same port.
  spawnSync('pkill', ['-f', 'vite --port 5173']);
  const vite = spawn('npx', ['vite', '--port', '5173', '--strictPort'], { cwd: wt2, stdio: 'ignore', detached: true });
  try {
    await waitFor(async () => (await fetch('http://localhost:5173/')).ok, { timeout: 30000, label: 'vite from wt2' });
    setRoster([S('claude-aaaa1111', 'react-main', react), S('claude-wt2', 'wt2-main', wt2)]);
    const ctx = await pickOn('http://localhost:5173/?m2', '.save-button');
    await waitText(ctx, /dev server on this port now runs from another folder/, 'portMoved');
    await tryEnterWithoutTarget(ctx);
  } finally {
    // Put the original dev server back: wait for the port to be free (the old
    // process can take a moment), then start it, retrying a strict-port race.
    try { process.kill(-vite.pid); } catch {}
    spawnSync('pkill', ['-f', 'vite --port 5173']);
    await waitFor(() => spawnSync('lsof', ['-nP', '-iTCP:5173', '-sTCP:LISTEN']).status !== 0, { timeout: 20000, label: 'port 5173 free' });
    let up = false;
    for (let attempt = 0; attempt < 3 && !up; attempt++) {
      spawn('npx', ['vite', '--port', '5173', '--strictPort'], { cwd: react, stdio: 'ignore', detached: true }).unref();
      up = await waitFor(async () => (await fetch('http://localhost:5173/')).ok, { timeout: 30000, label: 'vite restored' }).then(() => true, () => false);
    }
    assert.ok(up, 'react dev server restored on 5173');
  }
  return { ok: true };
});

await scenario('m03-test-domain-config-one-click', async () => {
  resetBindings();
  setRoster([S('claude-aaaa1111', 'react-main', react)]);
  const ctx = await pickOn('http://test.inspect.example/', '.save-button');
  const t = await waitText(ctx, /Send requests from test.inspect.example to react-main/, 'preselected by config');
  assert.match(t, /project config/);
  await click(ctx, 'Use for this site');
  await waitText(ctx, /→\s*react-main/, 'bound');
  // Page data cannot move a bound target: forged marks and revision.
  await A.evaluate(ctx.page.sid, `(() => { const m = document.createElement('meta'); m.name = 'chrome-use-revision'; m.content = 'deadbeef'; document.head.append(m); document.querySelector('.save-button').setAttribute('data-insp-path', ${JSON.stringify(wt2 + '/src/OrderForm.jsx:1:1:button')}); return 1; })()`);
  const again = await pickOn('http://test.inspect.example/?forged', '.save-button');
  assert.equal((await send(again, 'm3 change')).target, 'claude-aaaa1111');
  return { ok: true };
});

await scenario('m04-same-origin-two-apps', async () => {
  resetBindings();
  setRoster([S('claude-adm1', 'admin', path.join(mono, 'apps/admin')), S('claude-shp1', 'shop', path.join(mono, 'apps/shop'))]);
  const admin = await pickOn('http://apps.inspect.example/admin/', '.admin-save');
  await waitText(admin, /apps.inspect.example\/admin to admin/, 'admin preselected');
  await click(admin, 'Use for this site');
  const shop = await pickOn('http://apps.inspect.example/shop/', '.shop-buy');
  await waitText(shop, /apps.inspect.example\/shop to shop/, 'shop preselected');
  await click(shop, 'Use for this site');
  assert.equal((await send(shop, 'shop change')).target, 'claude-shp1');
  const admin2 = await pickOn('http://apps.inspect.example/admin/orders', '.admin-save');
  assert.equal((await send(admin2, 'admin change')).target, 'claude-adm1');
  const other = await pickOn('http://apps.inspect.example/administrator', '.admin-save');
  const t = await text(other.frame);
  assert.doesNotMatch(t, /→\s*admin\b/, '/administrator is not inside /admin');
  assert.match(t, /Choose where requests from this page go/, 'no preselection');
  return { ok: true };
});

await scenario('m05-worktrees-ranked-by-revision', async () => {
  resetBindings();
  setRoster([S('claude-main1', 'main', react), S('claude-wt2', 'wt2', wt2)]);
  const ctx = await pickOn('http://rev.inspect.example/', '.rev-save');
  const t = await waitText(ctx, /Choose where requests from this page go/, 'no preselection across worktrees');
  const first = t.indexOf(tilde(wt2)), second = t.indexOf(tilde(react) + '\n');
  assert.ok(first >= 0 && (second < 0 || first < second), 'worktree containing the deployed commit ranks first');
  assert.match(t, /deployed build is your local HEAD/);
  assert.match(t, /behind the deployed build/);
  return { ok: true };
});

await scenario('m06-two-sessions-one-worktree', async () => {
  resetBindings();
  setRoster([S('claude-aaaa1111', 'react-main', react), S('claude-bbbb2222', 'react-second', react)]);
  const ctx = await pickOn('http://localhost:5173/?m6', '.save-button');
  await waitText(ctx, /Choose where requests from this page go/, 'no preselection with two sessions');
  await A.clickInFrame(ctx.page.sid, ctx.frame.sid, `[...document.querySelectorAll('label')].find(l => /react-second/.test(l.textContent)).querySelector('input')`);
  await click(ctx, 'Use for this site');
  const next = await pickOn('http://localhost:5173/?m6b', '.save-button');
  assert.equal((await send(next, 'm6')).target, 'claude-bbbb2222');
  return { ok: true };
});

await scenario('m07-session-restarted', async () => {
  setRoster([S('claude-aaaa1111', 'react-main', react), S('claude-bbbb9999', 'react-second', react)]);
  const ctx = await pickOn('http://localhost:5173/?m7', '.save-button');
  await waitText(ctx, /react-second is no longer running/, 'sessionEnded');
  await tryEnterWithoutTarget(ctx);
  return { ok: true };
});

await scenario('m08-session-moved', async () => {
  resetBindings();
  setRoster([S('claude-aaaa1111', 'react-main', react)]);
  let ctx = await pickOn('http://localhost:5173/?m8', '.save-button');
  await click(ctx, 'Use for this site');
  setRoster([S('claude-aaaa1111', 'react-main', vue)]);
  ctx = await pickOn('http://localhost:5173/?m8b', '.save-button');
  await waitText(ctx, /now works in another folder/, 'sessionMoved');
  await tryEnterWithoutTarget(ctx);
  return { ok: true };
});

await scenario('m09-link-from-session-zero-clicks', async () => {
  resetBindings();
  setRoster([S('claude-aaaa1111', 'react-main', react)]);
  const r = cli(['link', 'http://test.inspect.example/'], 'claude-aaaa1111');
  assert.equal(r.code, 0, r.stdout + r.stderr);
  const ctx = await pickOn('http://test.inspect.example/?m9', '.save-button');
  assert.doesNotMatch(await text(ctx.frame), /Use for this site/);
  assert.equal((await send(ctx, 'm9')).target, 'claude-aaaa1111');
  return { link: r.json };
});

await scenario('m10-link-conflict-needs-browser-ok', async () => {
  setRoster([S('claude-aaaa1111', 'react-main', react), S('claude-bbbb2222', 'react-second', react)]);
  const r = cli(['link', 'http://test.inspect.example/'], 'claude-bbbb2222');
  assert.equal(r.code, 2, 'pending claim: ' + r.stdout + r.stderr);
  const ctx = await pickOn('http://test.inspect.example/?m10', '.save-button');
  await waitText(ctx, /react-second asks to take over test.inspect.example/, 'claim banner');
  assert.equal((await send(ctx, 'still old')).target, 'claude-aaaa1111', 'claimant gets nothing before Replace');
  await click(ctx, 'New batch').catch(() => {});
  await click(ctx, 'Replace');
  await waitText(ctx, /Claim accepted/, 'accepted');
  const next = await pickOn('http://test.inspect.example/?m10b', '.save-button');
  assert.equal((await send(next, 'now new')).target, 'claude-bbbb2222');
  return { ok: true };
});

await scenario('m11-tab-only-target', async () => {
  resetBindings();
  setRoster([S('claude-aaaa1111', 'react-main', react), S('claude-bbbb2222', 'react-second', react)]);
  assert.equal(cli(['link', 'http://test.inspect.example/'], 'claude-aaaa1111').code, 0);
  const tabA = await pickOn('http://test.inspect.example/?tabA', '.save-button');
  await A.clickInFrame(tabA.page.sid, tabA.frame.sid, `[...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Change')`);
  await waitText(tabA, /react-second/, 'pairs listed');
  await A.clickInFrame(tabA.page.sid, tabA.frame.sid, `[...document.querySelectorAll('label')].find(l => /react-second/.test(l.textContent)).querySelector('input')`);
  await click(tabA, 'This tab only');
  assert.equal((await send(tabA, 'tab A')).target, 'claude-bbbb2222');
  const tabB = await pickOn('http://test.inspect.example/?tabB', '.save-button');
  assert.equal((await send(tabB, 'tab B')).target, 'claude-aaaa1111', 'other tabs keep the site default');
  await b.send('Page.reload', {}, tabA.page.sid);
  await waitFor(() => A.evaluate(tabA.page.sid, 'document.readyState === "complete"'), { label: 'reloaded' });
  const tabA2 = await pickOn('http://test.inspect.example/?tabA', '.save-button', tabA.page);
  assert.equal((await send(tabA2, 'after reload')).target, 'claude-aaaa1111', 'reload ends the tab override');
  return { ok: true };
});

await scenario('m12-monorepo-app-root-and-forged-marks', async () => {
  resetBindings();
  setRoster([S('claude-mono', 'mono-root', mono)]);
  const server = spawn('python3', ['-m', 'http.server', '5175', '--bind', '127.0.0.1'], { cwd: path.join(mono, 'apps/admin'), stdio: 'ignore' });
  try {
    await waitFor(async () => (await fetch('http://localhost:5175/index.html')).ok, { timeout: 15000, label: 'admin dev server' });
    const ctx = await pickOn('http://localhost:5175/index.html', '.admin-save');
    const t = await waitText(ctx, /Send requests from localhost:5175 to mono-root/, 'preselected via port');
    assert.match(t, new RegExp(tilde(path.join(mono, 'apps/admin')).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
    await click(ctx, 'Use for this site');
    await click(ctx, '+ Add element');
    await A.click(ctx.page.sid, ...Object.values(await A.center(ctx.page.sid, '.forged-shop')), 8);
    await A.click(ctx.page.sid, ...Object.values(await A.center(ctx.page.sid, '.forged-shared')), 8);
    const dm = await send(ctx, 'monorepo');
    assert.equal(dm.target, 'claude-mono');
    const lines = dm.text.split('\n');
    assert.ok(lines.some(l => /apps\/admin\/src\/App\.tsx|src\/App\.tsx/.test(l) && /verified/.test(l) && !/unverified/.test(l)), 'admin mark verified');
    assert.ok(lines.some(l => /Forged shop/.test(l) && /unverified/.test(l)), 'shop mark stays unverified');
    assert.ok(lines.some(l => /Forged shared/.test(l) && /unverified/.test(l)), 'shared mark stays unverified');
  } finally { server.kill(); }
  return { ok: true };
});

await scenario('m18-wildcard-config', async () => {
  resetBindings();
  setRoster([S('claude-wild', 'wild', wild)]);
  const a = await pickOn('http://a.wild.inspect.example/', '.wild-go');
  await waitText(a, /Send requests from a.wild.inspect.example to wild/, 'wildcard preselect');
  await click(a, 'Use for this site');
  const list = JSON.parse(fs.readFileSync(BINDINGS, 'utf8'));
  assert.ok((list.bindings || []).some(x => x.origin === 'http://a.wild.inspect.example'), 'binding stores the exact origin');
  const bsub = await pickOn('http://b.wild.inspect.example/', '.wild-go');
  await waitText(bsub, /Send requests from b.wild.inspect.example to wild/, 'other subdomain asks again');
  const deep = await pickOn('http://a.b.wild.inspect.example/', '.wild-go');
  assert.doesNotMatch(await text(deep.frame), /Send requests from a.b.wild/, 'two labels do not match the wildcard');
  return { ok: true };
});

await scenario('m19-spa-route-change', async () => {
  resetBindings();
  setRoster([S('claude-adm1', 'admin', path.join(mono, 'apps/admin')), S('claude-shp1', 'shop', path.join(mono, 'apps/shop'))]);
  const shop = await pickOn('http://apps.inspect.example/shop/', '.shop-buy');
  await click(shop, 'Use for this site');
  const admin = await pickOn('http://apps.inspect.example/admin/', '.admin-save');
  await click(admin, 'Use for this site');
  // Same document, new path: the panel re-resolves and says so.
  await A.evaluate(admin.page.sid, "history.pushState({}, '', '/shop/cart'); 1");
  await waitText(admin, /The page moved to another path/, 'route change notice');
  await waitText(admin, /→\s*shop/, 'target follows the new path');
  assert.equal((await send(admin, 'after route change')).target, 'claude-shp1');
  return { ok: true };
});

await scenario('m20-target-changed-after-panel-showed-it', async () => {
  resetBindings();
  setRoster([S('claude-aaaa1111', 'react-main', react), S('claude-bbbb2222', 'react-second', react)]);
  assert.equal(cli(['link', 'http://test.inspect.example/'], 'claude-aaaa1111').code, 0);
  const ctx = await pickOn('http://test.inspect.example/?m20', '.save-button');
  await waitText(ctx, /→\s*react-main/, 'panel shows A');
  // Elsewhere: A unlinks, B links (no claim needed now) while the panel still shows A.
  assert.equal(cli(['unlink', 'http://test.inspect.example/'], 'claude-aaaa1111').code, 0);
  assert.equal(cli(['link', 'http://test.inspect.example/'], 'claude-bbbb2222').code, 0);
  const before = dms().length;
  await A.clickInFrame(ctx.page.sid, ctx.frame.sid, 'document.getElementById("instruction")');
  await b.send('Input.insertText', { text: 'meant for A' }, ctx.page.sid);
  await A.clickInFrame(ctx.page.sid, ctx.frame.sid, 'document.getElementById("send")');
  await waitText(ctx, /target changed since the panel showed it/, 'refused');
  assert.equal(dms().length, before, 'nothing sent to B without the user seeing B');
  await waitText(ctx, /→\s*react-second/, 'panel now shows B');
  return { ok: true };
});

results.finishedAt = new Date().toISOString();
fs.writeFileSync(path.join(dir, 'results-matching.json'), JSON.stringify(results, null, 2));
const failed = results.scenarios.filter(s => !s.ok);
console.log(`${results.scenarios.length - failed.length}/${results.scenarios.length} passed on ${results.chrome}`);
b.close();
process.exit(failed.length ? 1 : 0);
