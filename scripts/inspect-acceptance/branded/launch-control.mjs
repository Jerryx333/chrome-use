// Control: branded Chrome launched the way the real-Chrome rig launches it
// (pipe + unsafe extension debugging), with NO extension, versus a port launch.
import fs from 'node:fs'; import os from 'node:os'; import path from 'node:path';
import { launchPipe, launch, sleep } from '../lib.mjs';
const results = {};
async function probe(name, b, attach) {
  const t = await b.send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await b.send('Target.attachToTarget', { targetId: t.targetId, flatten: true });
  const ev = async e => (await b.send('Runtime.evaluate', { expression: e, returnByValue: true, awaitPromise: true }, sessionId)).result.value;
  const r = { webdriver: await ev('navigator.webdriver') };
  await b.send('Page.navigate', { url: 'https://bot.sannysoft.com/' }, sessionId);
  await sleep(8000);
  r.sannysoftFailed = await ev('[...document.querySelectorAll("td.failed, td[class*=failed]")].map(td => (td.parentElement.cells[0]||{}).innerText).filter(Boolean)');
  results[name] = r;
}
const chrome = process.env.CHROME;
{ const ud = fs.mkdtempSync(path.join(os.tmpdir(), 'ctl-')); const b = await launchPipe({ chrome, userDataDir: ud }); await probe('pipe+unsafeExtDebug, no extension', b); b.close(); }
{ const ud = fs.mkdtempSync(path.join(os.tmpdir(), 'ctl-')); const b = await launch({ chrome, port: 9371, userDataDir: ud, args: [] }); await probe('port, no extension', b); b.close(); }
results.chrome = chrome;
console.log(JSON.stringify(results, null, 1));
