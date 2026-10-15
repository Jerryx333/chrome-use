// Live rig: fixture site, Chrome for Testing with the unpacked extension and
// the branch's native host in an isolated HOME, the panel service for the
// extension's profile, and the fault proxy in front of it. Nothing outside the
// run's work directory is touched; the build host's resident Chrome and
// services are never addressed.
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { launch, api } from '../inspect-acceptance/lib.mjs';
import { Service, Proxy, waitFor, sleep } from './lib.mjs';

export const EXT_ID = 'ciiljdlhdpfckdcfkphgmfalanpdejep';

export async function startRig({ dir, work, hostBin, chrome, extDir, sitePort = 8910, cdpPort = 9371, serviceEnv = {} }) {
  const home = path.join(work, 'home');
  const accept = path.join(work, 'accept');
  for (const d of [home, accept, path.join(accept, 'scenarios'), path.join(accept, 'signals'), path.join(work, 'backends'), path.join(work, 'xdg')]) fs.mkdirSync(d, { recursive: true });
  const rig = { dir, work, home, accept, hostBin, extDir, sitePort, site: `http://127.0.0.1:${sitePort}`, children: [] };

  // Fixture backends: the executable override points at these wrappers.
  for (const agent of ['codex', 'claude', 'pi', 'opencode']) {
    const p = path.join(work, 'backends', agent);
    fs.writeFileSync(p, `#!/bin/sh\nexec "${process.execPath}" "${path.join(dir, 'backends', 'fixture-backend.mjs')}" ${agent} "$@"\n`, { mode: 0o755 });
  }
  rig.backend = agent => path.join(work, 'backends', agent);
  // Real OpenCode reads its login from a private data directory holding only
  // a copy of auth.json (its shared database belongs to a newer build).
  if (process.env.REAL_OPENCODE_AUTH && fs.existsSync(process.env.REAL_OPENCODE_AUTH)) {
    fs.mkdirSync(path.join(work, 'xdg', 'opencode'), { recursive: true });
    fs.copyFileSync(process.env.REAL_OPENCODE_AUTH, path.join(work, 'xdg', 'opencode', 'auth.json'));
    fs.chmodSync(path.join(work, 'xdg', 'opencode', 'auth.json'), 0o600);
  }

  rig.siteLog = path.join(work, 'site.log');
  const site = spawn(process.execPath, [path.join(dir, 'site', 'server.mjs'), String(sitePort), rig.siteLog], { stdio: 'ignore' });
  rig.children.push(site);
  await waitFor(async () => (await fetch(rig.site + '/')).ok, { label: 'fixture site' });

  // Native host with this run's HOME.
  const nm = path.join(work, 'nm-host.sh');
  // The service may be started by this native host (when the extension
  // connects) before the driver starts it, and the first one owns the
  // profile: give the host the same environment the driver's service gets.
  const hostEnv = { HOME: home, CHROME_USE_NO_UPDATE_CHECK: '1', PANEL_ACCEPT_DIR: accept, XDG_DATA_HOME: path.join(work, 'xdg'), CODEX_HOME: process.env.REAL_CODEX_HOME || path.join(home, '.codex') };
  const quote = v => `'${String(v).replace(/'/g, `'\\''`)}'`;
  fs.writeFileSync(nm, `#!/bin/sh\n${Object.entries(hostEnv).map(([k, v]) => `export ${k}=${quote(v)}`).join('\n')}\nexec ${quote(hostBin)} __nm-host "$@"\n`, { mode: 0o755 });
  const userDataDir = path.join(work, 'chrome-profile');
  fs.mkdirSync(path.join(userDataDir, 'NativeMessagingHosts'), { recursive: true });
  fs.writeFileSync(path.join(userDataDir, 'NativeMessagingHosts', 'com.agent_browser.connect.json'), JSON.stringify({ name: 'com.agent_browser.connect', description: 'panel acceptance host', path: nm, type: 'stdio', allowed_origins: [`chrome-extension://${EXT_ID}/`] }));
  rig.browser = await launch({ chrome, port: cdpPort, userDataDir, args: ['--headless=new', `--load-extension=${extDir}`, `--disable-extensions-except=${extDir}`, '--window-size=1280,900'] });
  rig.cdp = api(rig.browser);
  rig.chromeVersion = rig.browser.version;
  const sw = await rig.cdp.worker();
  rig.sw = sw;
  rig.ev = expr => rig.cdp.evaluate(sw.sid, expr);
  rig.profile = await waitFor(async () => (await rig.ev(`chrome.storage.local.get('ab_profile_id')`)).ab_profile_id, { timeout: 30000, label: 'extension profile id' });
  await waitFor(() => fs.existsSync(path.join(home, '.chrome-use', 'relay-cdp-url')), { timeout: 30000, label: 'relay up' });
  rig.extensionVersion = await rig.ev('chrome.runtime.getManifest().version');
  await instrumentDebugger(rig);

  // The panel service for this profile, started by the driver (not from the
  // native host, so it never inherits Chrome's process tree).
  rig.root = path.join(home, '.chrome-use', 'agent-panel');
  rig.profileDir = path.join(rig.root, 'profiles', rig.profile);
  rig.serviceJson = path.join(rig.profileDir, 'service.json');
  rig.serviceEnv = {
    ...process.env, HOME: home, CHROME_USE_NO_UPDATE_CHECK: '1', PANEL_ACCEPT_DIR: accept,
    XDG_DATA_HOME: path.join(work, 'xdg'), CODEX_HOME: process.env.REAL_CODEX_HOME || path.join(home, '.codex'),
    ...serviceEnv,
  };
  await startService(rig);
  return rig;
}

// The extension's native host may start the profile's service (through
// launchd) before the driver does, and the first one owns the profile: the
// driver's own then exits ("Another agent service owns this profile") and
// the run talks to a service with another environment and process tree. So
// a service the driver did not start is stopped first, and a start that
// lost the race again is retried until the driver's process owns the
// profile.
export async function startService(rig, { bin = rig.hostBin, env = {} } = {}) {
  const out = fs.openSync(path.join(rig.work, 'service.log'), 'a');
  let proc;
  for (let attempt = 1; ; attempt++) {
    await stopOtherService(rig);
    proc = spawn(bin, ['__panel-service', '--profile', rig.profile, '--data-dir', rig.root], { env: { ...rig.serviceEnv, ...env }, stdio: ['ignore', out, out], detached: true });
    proc.unref();
    let exited = false;
    proc.once('exit', () => { exited = true; });
    const owner = await waitFor(() => exited ? 'lost' : ownerPid(rig) === proc.pid && 'ours', { timeout: 20000, label: 'service.json' });
    if (owner === 'ours') break;
    if (attempt === 5) throw new Error('another service kept the profile after five starts');
    await sleep(200);
  }
  rig.servicePid = proc.pid;
  rig.proxy?.stop();
  rig.proxy = new Proxy(rig.serviceJson, path.join(rig.work, 'proxy.jsonl'));
  await rig.proxy.start();
  rig.service = new Service(rig.serviceJson);
  await waitFor(async () => (await rig.service.call('health', {})) && true, { label: 'service health' });
  return proc;
}

function ownerPid(rig) {
  try { return JSON.parse(fs.readFileSync(rig.serviceJson, 'utf8')).pid; } catch { return null; }
}

const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };

// Stop the profile's service when it is not the driver's (or a stale file
// names a process that is gone), and wait until it released the profile.
async function stopOtherService(rig) {
  const pid = ownerPid(rig);
  if (pid && alive(pid)) {
    await new Service(rig.serviceJson).tryCall('serviceStop', {}, 10000);
    await waitFor(() => !alive(pid), { timeout: 20000, label: 'the other service to exit' }).catch(() => { try { process.kill(pid, 'SIGKILL'); } catch {} });
  }
  fs.rmSync(rig.serviceJson, { force: true });
}

export async function stopService(rig) {
  const list = await rig.service.call('conversationList', {}).catch(() => ({ conversations: [] }));
  for (const c of list.conversations || []) await rig.service.tryCall('conversationStop', { conversationId: c.id });
  await rig.service.tryCall('serviceStop', {});
  await waitFor(() => { try { process.kill(rig.servicePid, 0); return false; } catch { return true; } }, { timeout: 30000, label: 'service exit' }).catch(() => { try { process.kill(rig.servicePid, 'SIGKILL'); } catch {} });
  rig.proxy?.stop();
}

// Record every CDP method the extension sends for the relay. This patches
// chrome.debugger.sendCommand inside the extension worker (driver-side
// instrumentation through the worker target), so pages see nothing.
export async function instrumentDebugger(rig) {
  await rig.ev(`(() => {
    if (globalThis.__panelAcceptCdp) return 'already';
    globalThis.__panelAcceptCdp = [];
    const original = chrome.debugger.sendCommand.bind(chrome.debugger);
    chrome.debugger.sendCommand = (target, method, params, cb) => {
      globalThis.__panelAcceptCdp.push({ at: Date.now(), tabId: target && target.tabId, session: !!(target && target.sessionId), method });
      return original(target, method, params, cb);
    };
    return 'patched';
  })()`);
}
export async function cdpLog(rig) {
  const log = await rig.ev('globalThis.__panelAcceptCdp || null');
  if (log === null) { await instrumentDebugger(rig); return { restarted: true, log: [] }; }
  return { restarted: false, log };
}

// Browser-level tab helpers: no attach to web pages (stealth check 16).
export async function openTab(rig, url) {
  const { targetId } = await rig.browser.send('Target.createTarget', { url });
  const tab = await waitFor(async () => (await rig.ev(`chrome.tabs.query({})`)).find(t => (t.url || t.pendingUrl) === url && t.status === 'complete'), { timeout: 15000, label: 'tab ' + url });
  return { targetId, tabId: tab.id, url, title: tab.title };
}
export async function activate(rig, tab) { await rig.browser.send('Target.activateTarget', { targetId: tab.targetId }); await rig.ev(`chrome.tabs.update(${tab.tabId}, {active: true})`); }
export async function chromeTabs(rig) { return rig.ev('chrome.tabs.query({})'); }

export function cleanup(rig) {
  for (const c of rig.children) { try { c.kill('SIGKILL'); } catch {} }
  try { rig.browser?.close(); } catch {}
  // Daemons the conversations' MCP servers started under this HOME.
  spawnSync(rig.hostBin, ['close', '--all', '--force'], { env: { ...process.env, HOME: rig.home }, stdio: 'ignore', timeout: 30000 });
  // The panel services this run started (and any the extension's native host
  // started for its profile) run binaries copied into this run's directory.
  spawnSync('pkill', ['-f', `^${rig.work}/`], { stdio: 'ignore' });
}
