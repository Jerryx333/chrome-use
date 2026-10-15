// Step 1 of a UI install: launch branded Chrome on a fresh profile, enable
// developer mode, open the "Load unpacked" picker, then wait for it to close
// and print the installed extensions. The picker is driven separately.
import fs from 'node:fs';
import { launch, sleep } from '../lib.mjs';
const [ud] = process.argv.slice(2);
const b = await launch({ chrome: process.env.CHROME, port: 9361, userDataDir: ud });
fs.writeFileSync('/tmp/uiinst.pid', String(b.proc.pid));
const t = await b.send('Target.createTarget', { url: 'chrome://extensions' });
const { sessionId } = await b.send('Target.attachToTarget', { targetId: t.targetId, flatten: true });
await sleep(2000);
const ev = (e, gesture) => b.send('Runtime.evaluate', { expression: e, awaitPromise: true, returnByValue: true, userGesture: !!gesture }, sessionId);
console.log('devmode', (await ev('new Promise(r => chrome.developerPrivate.updateProfileConfiguration({inDeveloperMode: true}, () => r(chrome.runtime.lastError ? chrome.runtime.lastError.message : "ok")))')).result.value);
await ev('chrome.developerPrivate.loadUnpacked({failQuietly: true}, () => { window.__picked = chrome.runtime.lastError ? chrome.runtime.lastError.message : "done"; }), "opened"', true);
console.log('picker opened');
for (let i = 0; i < 600 && !(await ev('window.__picked || ""')).result.value; i++) await sleep(500);
console.log('picked', (await ev('window.__picked || "timeout"')).result.value);
const list = await ev('new Promise(r => chrome.developerPrivate.getExtensionsInfo({includeDisabled: true}, l => r(l.map(x => ({ id: x.id, version: x.version, location: x.location, state: x.state })))))');
console.log('extensions', JSON.stringify(list.result.value));
await b.send('Browser.close').catch(() => {});
await sleep(4000);
b.close();
