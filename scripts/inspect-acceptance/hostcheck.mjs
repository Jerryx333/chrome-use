// Host-level matching checks (build host): drive the branch host over native
// messaging and the `chrome-use inspect` CLI with the fake ocs, no browser.
// Covers legacy callers, migration, claims and overlaps, config validation,
// unlink identity, forged tab targets and ambiguous configs.
import assert from 'node:assert/strict';
import { spawn, spawnSync, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const dir = path.dirname(new URL(import.meta.url).pathname);
const BIN = path.join(dir, 'bin', 'current');
const OCS = path.join(dir, 'fake-ocs');
const LOG = path.join(dir, 'fake-ocs.log');
const ROSTER = path.join(dir, 'fake-roster.json');
const D = name => fs.realpathSync(path.join(dir, name));
const react = D('react-app');
const results = { startedAt: new Date().toISOString(), host: fs.realpathSync(BIN), checks: [] };
const only = process.env.ONLY ? process.env.ONLY.split(',') : null;
const S = (id, name, cwd) => ({ kind: 'claude', id, name, cwd, status: 'idle', self: false });
const setRoster = entries => fs.writeFileSync(ROSTER, JSON.stringify(entries));
const dms = () => { try { return fs.readFileSync(LOG, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse).filter(x => x.cmd === 'dm'); } catch { return []; } };

let HOME;
function freshHome() {
  HOME = fs.mkdtempSync(path.join(os.tmpdir(), 'inspect-hostcheck-'));
  fs.mkdirSync(path.join(HOME, '.chrome-use', 'inspect'), { recursive: true, mode: 0o700 });
  return HOME;
}
const bindingsPath = () => path.join(HOME, '.chrome-use', 'inspect', 'bindings.json');
const cli = (args, self) => {
  const r = spawnSync(BIN, ['inspect', ...args, '--json'], { env: { ...process.env, HOME, CHROME_USE_INSPECT_OCS: OCS, FAKE_OCS_SELF: self || '' }, encoding: 'utf8', timeout: 30000 });
  let json = null;
  try { json = JSON.parse(r.stdout.trim().split('\n').at(-1)); } catch {}
  return { code: r.status, out: r.stdout + r.stderr, json };
};

// One native-host process per check, speaking length-prefixed JSON.
function host() {
  const p = spawn(BIN, ['__nm-host', 'chrome-extension://ciiljdlhdpfckdcfkphgmfalanpdejep/'], { env: { ...process.env, HOME, CHROME_USE_RELAY_DIR: path.join(HOME, 'relay'), CHROME_USE_INSPECT_OCS: OCS }, stdio: ['pipe', 'pipe', 'ignore'] });
  fs.mkdirSync(path.join(HOME, 'relay'), { recursive: true });
  let buf = Buffer.alloc(0);
  const waiters = new Map();
  p.stdout.on('data', d => {
    buf = Buffer.concat([buf, d]);
    while (buf.length >= 4) {
      const n = buf.readUInt32LE(0);
      if (buf.length < 4 + n) break;
      const m = JSON.parse(buf.subarray(4, 4 + n));
      buf = buf.subarray(4 + n);
      if (m.requestId && waiters.has(m.requestId)) waiters.get(m.requestId)(m);
    }
  });
  let n = 0;
  const call = (operation, params) => new Promise((resolve, reject) => {
    const requestId = 'r' + (++n);
    const t = setTimeout(() => reject(new Error(operation + ' timed out')), 40000);
    waiters.set(requestId, m => { clearTimeout(t); m.error ? reject(new Error(m.error)) : resolve(m.result); });
    const body = Buffer.from(JSON.stringify({ method: 'agentPanel', requestId, operation, params }));
    const h = Buffer.alloc(4); h.writeUInt32LE(body.length);
    p.stdin.write(Buffer.concat([h, body]));
  });
  return { call, close: () => p.kill() };
}

async function check(name, fn) {
  if (only && !only.includes(name)) return;
  freshHome();
  const h = host();
  try {
    const detail = await fn(h);
    results.checks.push({ name, ok: true, detail });
    console.log('PASS', name);
  } catch (error) {
    results.checks.push({ name, ok: false, error: String(error.stack || error) });
    console.log('FAIL', name, String(error.message || error).slice(0, 300));
  } finally {
    h.close();
  }
}

const ORIGIN = 'http://test.inspect.example';
const annotation = { label: 'button "Save"', selector: '.save-button', markdown: '### 1. button' };
// pageUrl's path must equal pagePath when pagePath is given (contract item 6).
const deliverParams = extra => ({ deliveryId: crypto.randomUUID(), origin: ORIGIN, kind: 'edit', instruction: 'x', pageUrl: ORIGIN + (extra.pagePath || '/admin'), env: 'test', annotations: [annotation], images: [], ...extra });

await check('h15-legacy-caller-refused-on-scoped-origin', async h => {
  setRoster([S('claude-aaaa1111', 'react-main', react)]);
  assert.equal(cli(['link', ORIGIN + '/admin', '--path-scope', '/admin'], 'claude-aaaa1111').code, 0);
  const status = await h.call('inspectStatus', { origin: ORIGIN });
  assert.equal(status.upgradeRequired, true);
  assert.equal(status.binding, undefined);
  const before = dms().length;
  const result = await h.call('inspectDeliver', deliverParams({}));
  assert.equal(result.state, 'upgradeRequired');
  assert.equal(dms().length, before, 'nothing sent for a legacy caller');
  const scoped = await h.call('inspectDeliver', deliverParams({ pagePath: '/admin/orders' }));
  assert.equal(scoped.state, 'sent');
  return { legacy: result.state, scoped: scoped.state };
});

await check('h16-v1-bindings-migrate', async h => {
  setRoster([S('claude-aaaa1111', 'react-main', react)]);
  fs.writeFileSync(bindingsPath(), JSON.stringify({ [ORIGIN]: { origin: ORIGIN, root: react, sessionId: 'claude-aaaa1111', sessionCwd: react, consents: { plugin: false, testEnv: false }, updatedAt: 1 } }), { mode: 0o600 });
  const list = await h.call('inspectBindings', {});
  const b = list.bindings.find(x => x.origin === ORIGIN);
  assert.equal(b.pathScope, '/');
  assert.equal(b.source, 'confirmed');
  assert.deepEqual(b.sourceRoots, [react]);
  const resolve = await h.call('inspectResolve', { origin: ORIGIN, pagePath: '/x' });
  assert.equal(resolve.state, 'bound');
  return { ok: true };
});

await check('h17-h23-claims-and-stale-decisions', async h => {
  setRoster([S('claude-a', 'A', react), S('claude-b', 'B', react), S('claude-c', 'C', react)]);
  assert.equal(cli(['link', ORIGIN + '/'], 'claude-a').code, 0);
  assert.equal(cli(['link', ORIGIN + '/admin', '--path-scope', '/admin'], 'claude-b').code, 2, '/ belongs to A: /admin is a claim');
  assert.equal(cli(['link', ORIGIN + '/shop', '--path-scope', '/shop'], 'claude-c').code, 2);
  const admin = await h.call('inspectResolve', { origin: ORIGIN, pagePath: '/admin/x' });
  assert.equal(admin.state, 'bound');
  assert.equal(admin.binding.sessionId, 'claude-a', 'unaccepted claim does not shadow A');
  assert.equal(admin.pendingClaims.length, 1);
  const shop = await h.call('inspectResolve', { origin: ORIGIN, pagePath: '/shop' });
  assert.equal(shop.pendingClaims.length, 1);
  assert.notEqual(shop.pendingClaims[0].claimId, admin.pendingClaims[0].claimId);
  const stale = admin.pendingClaims[0];
  // A new overlapping binding makes the admin card stale.
  assert.equal(cli(['link', ORIGIN + '/admin/deep', '--path-scope', '/admin/deep'], 'claude-a').code, 0);
  await assert.rejects(h.call('inspectClaimDecide', { claimId: stale.claimId, expectedAffects: stale.expectedAffects, accept: true }));
  const fresh = (await h.call('inspectResolve', { origin: ORIGIN, pagePath: '/admin/x' })).pendingClaims[0];
  const decided = await h.call('inspectClaimDecide', { claimId: fresh.claimId, expectedAffects: fresh.expectedAffects, accept: true });
  assert.equal(decided.decided, 'accepted');
  const after = await h.call('inspectResolve', { origin: ORIGIN, pagePath: '/admin/x' });
  assert.equal(after.binding.sessionId, 'claude-b');
  const shopDecided = await h.call('inspectClaimDecide', { claimId: shop.pendingClaims[0].claimId, expectedAffects: shop.pendingClaims[0].expectedAffects, accept: false });
  assert.equal(shopDecided.decided, 'rejected');
  return { ok: true };
});

await check('h22-broader-claim-lists-narrower-scope', async h => {
  setRoster([S('claude-a', 'A', react), S('claude-b', 'B', react)]);
  assert.equal(cli(['link', ORIGIN + '/admin', '--path-scope', '/admin'], 'claude-a').code, 0);
  assert.equal(cli(['link', ORIGIN + '/'], 'claude-b').code, 2);
  const r = await h.call('inspectResolve', { origin: ORIGIN, pagePath: '/' });
  const claim = r.pendingClaims[0];
  assert.ok(claim.affects.some(a => a.pathScope === '/admin'));
  await h.call('inspectClaimDecide', { claimId: claim.claimId, expectedAffects: claim.expectedAffects, accept: true });
  assert.equal((await h.call('inspectResolve', { origin: ORIGIN, pagePath: '/' })).binding.sessionId, 'claude-b');
  assert.equal((await h.call('inspectResolve', { origin: ORIGIN, pagePath: '/admin/x' })).binding.sessionId, 'claude-a', '/admin stays with A');
  return { ok: true };
});

await check('h19-config-validation', async h => {
  const bad = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'inspect-badcfg-')));
  execFileSync('git', ['init', '-q', '-b', 'main', bad]);
  const cases = { symlink: null, big: 'x'.repeat(70 * 1024), version: '{"version":2,"sites":[]}', field: '{"version":1,"sites":[{"origins":["http://bad.inspect.example"],"extra":1}]}' };
  for (const [name, body] of Object.entries(cases)) {
    const d = path.join(bad, 'apps', name, '.chrome-use');
    fs.mkdirSync(d, { recursive: true });
    if (name === 'symlink') {
      fs.writeFileSync(path.join(bad, 'real.json'), '{"version":1,"sites":[{"origins":["http://bad.inspect.example"]}]}');
      fs.symlinkSync(path.join(bad, 'real.json'), path.join(d, 'inspect.json'));
    } else fs.writeFileSync(path.join(d, 'inspect.json'), body);
  }
  execFileSync('git', ['-C', bad, 'add', '-A']);
  execFileSync('git', ['-C', bad, '-c', 'user.name=f', '-c', 'user.email=f@example.invalid', 'commit', '-qm', 'bad']);
  setRoster([S('claude-bad', 'bad', bad)]);
  const r = await h.call('inspectResolve', { origin: 'http://bad.inspect.example', pagePath: '/' });
  const reasons = (r.configErrors || []).map(e => e.path);
  for (const name of Object.keys(cases)) assert.ok(reasons.some(p => p.includes(`/apps/${name}/`)), `config ${name} rejected: ${JSON.stringify(r.configErrors)}`);
  assert.ok(!(r.card?.pairs || []).some(p => (p.evidence || []).includes('config')), 'no config evidence from rejected files');
  return { errors: r.configErrors };
});

await check('h20-unlink-only-own-binding', async h => {
  setRoster([S('claude-a', 'A', react), S('claude-b', 'B', react)]);
  assert.equal(cli(['link', ORIGIN + '/'], 'claude-a').code, 0);
  const r = cli(['unlink', ORIGIN + '/'], 'claude-b');
  assert.equal(r.code, 1, r.out);
  assert.equal((await h.call('inspectResolve', { origin: ORIGIN, pagePath: '/' })).binding.sessionId, 'claude-a');
  assert.equal(cli(['unlink', ORIGIN + '/'], 'claude-a').code, 0);
  return { ok: true };
});

await check('h24-forged-tab-target-refused', async h => {
  setRoster([S('claude-a', 'A', react), S('claude-b', 'B', react)]);
  assert.equal(cli(['link', ORIGIN + '/'], 'claude-a').code, 0);
  const before = dms().length;
  const wide = await h.call('inspectDeliver', deliverParams({ pagePath: '/', target: { root: react, sourceRoots: [react, '/'], sessionId: 'claude-b', sessionCwd: react, fingerprint: '0'.repeat(32) } }));
  assert.equal(wide.state, 'targetChanged');
  assert.equal(dms().length, before);
  return { state: wide.state };
});

await check('h25-two-configs-need-root', async h => {
  const two = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'inspect-twocfg-')));
  execFileSync('git', ['init', '-q', '-b', 'main', two]);
  for (const app of ['one', 'two']) {
    fs.mkdirSync(path.join(two, app, '.chrome-use'), { recursive: true });
    fs.writeFileSync(path.join(two, app, '.chrome-use', 'inspect.json'), '{"version":1,"sites":[{"origins":["http://two.inspect.example"]}]}');
  }
  execFileSync('git', ['-C', two, 'add', '-A']);
  execFileSync('git', ['-C', two, '-c', 'user.name=f', '-c', 'user.email=f@example.invalid', 'commit', '-qm', 'two']);
  setRoster([S('claude-two', 'two', two)]);
  const r = cli(['link', 'http://two.inspect.example/'], 'claude-two');
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /--root/);
  assert.equal(cli(['link', 'http://two.inspect.example/', '--root', path.join(two, 'one')], 'claude-two').code, 0);
  return { ok: true };
});

results.finishedAt = new Date().toISOString();
fs.writeFileSync(path.join(dir, 'results-hostcheck.json'), JSON.stringify(results, null, 2));
const failed = results.checks.filter(c => !c.ok);
console.log(`${results.checks.length - failed.length}/${results.checks.length} host checks passed`);
process.exit(failed.length ? 1 : 0);
