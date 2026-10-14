// Headed check on the build host's GUI session: the real manifest (activeTab,
// no host permissions), a real Alt+Shift+C keystroke sent by peekaboo, then a
// pick, a screenshot (proves the activeTab grant) and a delivery.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { launch, api, waitFor, sleep, byText } from './lib.mjs';

const dir = path.dirname(new URL(import.meta.url).pathname);
const out = { startedAt: new Date().toISOString() };
const ud = fs.mkdtempSync(path.join(os.tmpdir(), 'inspect-accept-headed-'));
fs.mkdirSync(path.join(ud, 'NativeMessagingHosts'));
fs.writeFileSync(path.join(ud, 'NativeMessagingHosts', 'com.agent_browser.connect.json'), JSON.stringify({ name: 'com.agent_browser.connect', description: 'x', path: path.join(dir, 'nm-host.sh'), type: 'stdio', allowed_origins: ['chrome-extension://ciiljdlhdpfckdcfkphgmfalanpdejep/'] }));
const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'ext/manifest.json'), 'utf8'));
assert.equal(manifest.host_permissions, undefined, 'real manifest has no host permissions');
assert.ok(manifest.permissions.includes('activeTab'));
const b = await launch({ chrome: process.env.CHROME, port: 9350, userDataDir: ud, args: [`--load-extension=${dir}/ext`, `--disable-extensions-except=${dir}/ext`], env: { CHROME_USE_INSPECT_OCS: path.join(dir, 'fake-ocs') } });
process.on('exit', () => b.close());
const A = api(b);
try {
  out.chrome = b.version;
  const sw = await A.worker();
  await sleep(4000);
  const page = await A.openPage('http://localhost:5173/?headed');
  await b.send('Page.bringToFront', {}, page.sid);
  // Shortcut configured by the manifest on a fresh profile.
  out.commands = await A.evaluate(sw.sid, 'chrome.commands.getAll().then(c => c.map(x => x.name + "=" + x.shortcut))');
  execFileSync('peekaboo', ['app', 'switch', '--to', 'Google Chrome for Testing'], { stdio: 'ignore' });
  await sleep(800);
  execFileSync('peekaboo', ['hotkey', '--keys', 'alt,shift,c'], { stdio: 'ignore' });
  await waitFor(() => A.evaluate(page.sid, '[...document.documentElement.children].some(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName))'), { timeout: 8000, label: 'picker after the real shortcut' });
  out.pickerFromShortcut = true;
  const c = await A.center(page.sid, '.save-button');
  await A.click(page.sid, c.x, c.y);
  const frame = await A.realFrame();
  if (/Enable Inspect & Ask/.test(await A.evaluate(frame.sid, 'document.body.innerText'))) await A.clickInFrame(page.sid, frame.sid, byText('Enable Inspect & Ask'));
  await waitFor(() => A.evaluate(frame.sid, '!!document.querySelector("img.thumb")'), { label: 'screenshot via activeTab' });
  out.screenshotViaActiveTab = true;
  out.frame = (await A.evaluate(frame.sid, 'document.getElementById("app").innerText')).slice(0, 400);
  out.ok = true;
} catch (error) {
  out.ok = false;
  out.error = String(error.stack || error);
}
fs.writeFileSync(path.join(dir, 'headed-results.json'), JSON.stringify(out, null, 2));
console.log(JSON.stringify(out, null, 2));
b.close();
process.exit(out.ok ? 0 : 1);
