// Live check of per-site permissions (#781, ab-connect 0.5.93) on the build
// host (one browser run per host at a time):
//   HOST_BIN=<chrome-use> CHROME=<Chrome for Testing> EXT_DIR=<extension> node site-permissions-live.mjs
// Chrome for Testing runs headless with its own profile, the unpacked
// extension and the native host in an isolated HOME (rig.mjs). Steps:
//   1. with "ask the first time" on for clicks, `click #act` on the fixture
//      does not run: exit 3, confirmation_required with approval: user, and
//      the request waits in the extension (storage.session);
//   2. the user allows it once from the options page; `confirm <id>` runs the
//      click (the page shows its effect);
//   3. typing into a password field asks by default (sensitive_input); the
//      agent withdraws it with `deny <id>` and the history says so;
//   4. a click the user declines is not run by `confirm`;
//   5. "Always allow on this site" lets the next click through with no
//      question; a blocked site refuses outright;
//   6. stealth: the page's own globals are the same before and after, and
//      nothing named chrome-use was added to the document.
// Evidence goes to $OUT (default <work>/out).
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { startRig, cleanup, EXT_ID } from './rig.mjs';
import { waitFor } from './lib.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url));
const work = process.env.WORK || fs.mkdtempSync('/tmp/sp-');
const out = process.env.OUT || path.join(work, 'out');
fs.mkdirSync(out, { recursive: true });
const evidence = { work, hostBin: process.env.HOST_BIN, steps: [] };
const step = (name, data = {}) => { evidence.steps.push({ name, at: new Date().toISOString(), ...data }); console.log(`[site-permissions] ${name}`, JSON.stringify(data).slice(0, 600)); };
const save = () => fs.writeFileSync(path.join(out, 'evidence.json'), JSON.stringify(evidence, null, 1));
const check = (ok, message) => { if (!ok) { save(); throw new Error(message); } };

const rig = await startRig({ dir, work, hostBin: process.env.HOST_BIN, chrome: process.env.CHROME, extDir: process.env.EXT_DIR, sitePort: 8960, cdpPort: 9421 });
evidence.chrome = rig.chromeVersion; evidence.extension = rig.extensionVersion;
const project = path.join(work, 'proj');
fs.mkdirSync(project, { recursive: true });

function cu(args, extra = {}) {
  const env = { ...process.env, HOME: rig.home, CHROME_USE_NO_UPDATE_CHECK: '1', CLAUDE_PID: '4242', TMUX_PANE: '%81', ...extra };
  for (const k of ['AGENT_BROWSER_SESSION', 'CMUX_SURFACE_ID', 'CMUX_CLAUDE_PID', 'TERM_SESSION_ID', 'ITERM_SESSION_ID', 'GHOSTTY_SURFACE_ID', 'CODEX_THREAD_ID']) delete env[k];
  const r = spawnSync(rig.hostBin, ['--json', ...args], { cwd: project, env, encoding: 'utf8', timeout: 90000 });
  let json = null;
  try { json = JSON.parse(r.stdout.trim().split('\n').at(-1)); } catch {}
  return { code: r.status, json, stdout: r.stdout, stderr: r.stderr };
}
const ok = (r, what) => { check(r.code === 0 && r.json?.success !== false, `${what} failed: ${r.code} ${r.stdout} ${r.stderr}`); return r.json; };
const store = value => rig.ev(`chrome.storage.local.set({ ab_site_permissions: ${JSON.stringify(value)} })`);
const requests = () => rig.ev(`chrome.storage.session.get('ab_site_requests').then(s => s.ab_site_requests || [])`);
const history = () => rig.ev(`chrome.storage.local.get('ab_site_permissions').then(s => (s.ab_site_permissions || {}).history || [])`);
const allowList = () => rig.ev(`chrome.storage.local.get('ab_site_permissions').then(s => (s.ab_site_permissions || {}).allow || [])`);
const pageText = sel => cu(['get', 'text', sel]).json?.data?.text ?? null;
const sleep = ms => new Promise(r => setTimeout(r, ms));

// The person answers from the options page, the way the UI does.
const options = await rig.cdp.openPage(`chrome-extension://${EXT_ID}/options.html`);
const decide = (id, decision) => rig.cdp.evaluate(options.sid, `chrome.runtime.sendMessage({ type: 'ab-site-permission', op: 'decide', id: ${JSON.stringify(id)}, decision: ${JSON.stringify(decision)} })`);
const optionsCards = () => rig.cdp.evaluate(options.sid, `[...document.querySelectorAll('#permPending .approval-card')].map(c => ({ id: c.dataset.requestId, text: c.textContent }))`);

try {
  // Baseline page, stealth probe.
  ok(cu(['open', `${rig.site}/b`]), 'open /b');
  const globals = () => cu(['eval', 'Object.getOwnPropertyNames(window).length']).json?.data?.result;
  const g0 = globals();
  step('baseline', { globals: g0 });

  // 1. First-visit click gate.
  await store({ settings: { firstVisit: ['click'] } });
  await sleep(2500); // the daemon caches the extension's layers for 2 s
  const held = cu(['click', '#act']);
  step('held click', { code: held.code, data: held.json?.data });
  check(held.code === 3, `gated click should exit 3, got ${held.code}: ${held.stdout}`);
  const d = held.json?.data || {};
  check(d.confirmation_required === true && d.approval === 'user' && d.site === '127.0.0.1' && d.reason === 'first_visit', 'confirmation payload');
  check((pageText('#done') || '') === '', 'the held click must not run');
  const pending = await requests();
  check(pending.some(r => r.id === d.confirmation_id && !r.decision), 'request waits in the extension');
  const cards = await waitFor(async () => { const c = await optionsCards(); return c.length ? c : null; }, { timeout: 10000, label: 'options page card' });
  step('options card', { cards });
  check(cards.some(c => c.id === d.confirmation_id), 'the options page shows the request');

  // 2. Allow once, then confirm runs it.
  const answered = await decide(d.confirmation_id, 'once');
  check(answered?.success === true, `decide: ${JSON.stringify(answered)}`);
  const confirmed = cu(['confirm', d.confirmation_id]);
  step('confirm after allow once', { code: confirmed.code, data: confirmed.json?.data });
  check(confirmed.code === 0 && confirmed.json?.data?.confirmed === true, 'confirm runs the allowed click');
  check(pageText('#done') === 'Acted on B', `click effect: ${pageText('#done')}`);
  check((await allowList()).length === 0, 'allow once remembers nothing');

  // 3. Password field asks by default; the agent withdraws.
  await store({});
  await sleep(2500);
  ok(cu(['open', `${rig.site}/login`]), 'open /login');
  const pw = cu(['fill', '#password', 'not-a-real-password']);
  step('password fill', { code: pw.code, data: pw.json?.data });
  check(pw.code === 3 && pw.json?.data?.reason === 'sensitive_input', 'password field asks');
  const withdrawn = cu(['deny', pw.json.data.confirmation_id]);
  check(withdrawn.code === 0 && withdrawn.json?.data?.denied === true, 'deny withdraws');
  check((await history()).some(h => h.decision === 'withdrawn'), 'history records the withdrawal');
  check(!(await requests()).some(r => r.id === pw.json.data.confirmation_id), 'withdrawn request is gone');

  // 4. Declined: confirm does not run it.
  await store({ settings: { firstVisit: ['click'] } });
  await sleep(2500);
  ok(cu(['open', `${rig.site}/b`]), 'open /b again');
  const held2 = cu(['click', '#act']);
  check(held2.code === 3, 'second click held');
  await decide(held2.json.data.confirmation_id, 'decline');
  const declined = cu(['confirm', held2.json.data.confirmation_id]);
  step('confirm after decline', { code: declined.code, data: declined.json?.data });
  check(declined.json?.data?.denied === true, 'a declined click is not run');
  check((pageText('#done') || '') === '', 'declined click left the page alone');

  // 5. Always allow, then block.
  const held3 = cu(['click', '#act']);
  check(held3.code === 3, 'third click held');
  await decide(held3.json.data.confirmation_id, 'always');
  ok(cu(['confirm', held3.json.data.confirmation_id]), 'confirm after always');
  check((await allowList()).includes('127.0.0.1'), 'always adds the site');
  await sleep(2500);
  ok(cu(['open', `${rig.site}/b`]), 'reopen /b');
  const free = cu(['click', '#act']);
  check(free.code === 0, `an always-allowed site does not ask: ${free.stdout}`);
  const blocked = await rig.cdp.evaluate(options.sid, `chrome.runtime.sendMessage({ type: 'ab-site-permission', op: 'sites', list: 'block', host: '127.0.0.1' })`);
  check(blocked?.success === true, 'block from the options page');
  await sleep(2500);
  const refused = cu(['click', '#act']);
  step('blocked click', { code: refused.code, error: refused.json?.error });
  check(refused.code !== 0 && /blocked chrome-use on 127\.0\.0\.1/.test(refused.json?.error || ''), 'a blocked site refuses');

  // 6. Stealth.
  const g1 = globals();
  const html = cu(['eval', 'document.documentElement.outerHTML']).json?.data?.result || '';
  step('stealth', { before: g0, after: g1, mentions: /chrome-use|ab-site|approval-card/.test(html) });
  check(g0 === g1, `page globals changed: ${g0} -> ${g1}`);
  check(!/chrome-use|ab-site|approval-card/.test(html), 'nothing of ours in the document');

  evidence.result = 'pass';
  console.log('[site-permissions] PASS');
} catch (error) {
  evidence.result = 'fail';
  evidence.error = String(error?.stack || error);
  console.error('[site-permissions] FAIL', error);
  process.exitCode = 1;
} finally {
  save();
  cleanup(rig);
}
