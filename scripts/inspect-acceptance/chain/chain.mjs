// Full chain on one machine: released host + 0.5.57 extension (unmodified
// manifest, activeTab only) in headed Chrome for Testing, a real Alt+Shift+C
// keystroke, a pick, Send, real ocs, delivered to the live session that ran
// `inspect link`.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs'; import http from 'node:http'; import os from 'node:os'; import path from 'node:path';
import { launch, api, waitFor, sleep } from '../lib.mjs';
const dir = path.dirname(new URL(import.meta.url).pathname);
const out = { startedAt: new Date().toISOString() };
const server = http.createServer((q, r) => { r.setHeader('content-type', 'text/html'); r.end(fs.readFileSync(path.join(dir, 'site/index.html'))); }).listen(5899, '127.0.0.1');
const ud = fs.mkdtempSync(path.join(os.tmpdir(), 'chain-'));
fs.mkdirSync(path.join(ud, 'NativeMessagingHosts'));
fs.writeFileSync(path.join(ud, 'NativeMessagingHosts', 'com.agent_browser.connect.json'), JSON.stringify({ name: 'com.agent_browser.connect', description: 'chain', path: path.join(dir, 'nm-host.sh'), type: 'stdio', allowed_origins: ['chrome-extension://ciiljdlhdpfckdcfkphgmfalanpdejep/'] }));
const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'ext/manifest.json'), 'utf8'));
out.manifest = { version: manifest.version, hostPermissions: manifest.host_permissions ?? null, activeTab: manifest.permissions.includes('activeTab') };
const b = await launch({ chrome: process.env.CHROME, port: 9380, userDataDir: ud, args: [`--load-extension=${dir}/ext`, `--disable-extensions-except=${dir}/ext`] });
const A = api(b);
const text = sid => A.evaluate(sid, 'document.getElementById("app").innerText');
const byText = t => `[...document.querySelectorAll('button')].find(b => b.textContent.trim() === ${JSON.stringify(t)})`;
try {
  out.chrome = b.version;
  await A.worker();
  await sleep(4000);
  const page = await A.openPage('http://localhost:5899/');
  await b.send('Page.bringToFront', {}, page.sid);
  execFileSync('peekaboo', ['app', 'switch', '--to', 'Google Chrome for Testing'], { stdio: 'ignore' });
  await sleep(800);
  execFileSync('peekaboo', ['hotkey', '--keys', 'alt,shift,c'], { stdio: 'ignore' });
  await waitFor(() => A.evaluate(page.sid, '[...document.documentElement.children].some(e => /^[A-Z]{5}-[A-Z]{6}$/.test(e.tagName))'), { timeout: 8000, label: 'picker after the real shortcut' });
  out.pickerFromShortcut = true;
  const c = await A.center(page.sid, '.target-button');
  await A.click(page.sid, c.x, c.y);
  const frame = await A.realFrame();
  if (/Enable Inspect & Ask/.test(await text(frame.sid))) await A.clickInFrame(page.sid, frame.sid, byText('Enable Inspect & Ask'));
  await waitFor(async () => /→ /.test(await text(frame.sid)), { label: 'bound target from inspect link' });
  out.target = (await text(frame.sid)).split('\n').find(l => l.startsWith('→'));
  await waitFor(() => A.evaluate(frame.sid, '!!document.querySelector("img.thumb")'), { label: 'screenshot via activeTab' });
  await A.clickInFrame(page.sid, frame.sid, 'document.getElementById("instruction")');
  await b.send('Input.insertText', { text: 'FULL CHAIN TEST (real shortcut, released v1.5.214 host, extension 0.5.57, real ocs): no action needed.' }, page.sid);
  await waitFor(() => A.evaluate(frame.sid, '!document.getElementById("send").disabled'), { label: 'Send enabled' });
  await A.clickInFrame(page.sid, frame.sid, 'document.getElementById("send")');
  await waitFor(async () => /Sent to the session|Stored in ocs/.test(await text(frame.sid)), { timeout: 40000, label: 'delivered' });
  out.notice = (await text(frame.sid)).split('\n').find(l => /Sent to the session|Stored in ocs/.test(l));
  out.ok = true;
} catch (e) { out.ok = false; out.error = String(e.stack || e); }
fs.writeFileSync(path.join(dir, 'chain-results.json'), JSON.stringify(out, null, 2));
console.log(JSON.stringify(out, null, 1));
server.close(); b.close(); fs.rmSync(ud, { recursive: true, force: true }); process.exit(out.ok ? 0 : 1);
