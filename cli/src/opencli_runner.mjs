// chrome-use ⇄ OpenCLI bridge. Runs one OpenCLI adapter with OpenCLI's own
// runtime (registry, argument coercion, pipeline executor, BasePage helpers)
// and a page whose transport is chrome-use, so the adapter drives the user's
// real, logged-in Chrome through the chrome-use daemon.
//
// Invoked by `chrome-use site <site>/<cmd>` when no chrome-use adapter has
// that name: node opencli_runner.mjs <request.json>
// request: { pkgDir, clisDir?, chromeUse, session, site, name, modulePath, kwargs }
// stdout: one JSON line { success, data | error, hint? }
//
// `node opencli_runner.mjs --list <request.json>` with { pkgDir, clisDir }
// imports every adapter module under clisDir (a source the user added with
// `site add`) and prints { success, data: [manifest entries] }, shaped like the
// package's cli-manifest.json with modulePath relative to clisDir.
import { spawn } from 'node:child_process';
import { readFileSync, readdirSync, writeFileSync, existsSync, mkdtempSync, rmSync } from 'node:fs';
import * as nodeModule from 'node:module';
import { tmpdir } from 'node:os';
import { join, relative, sep } from 'node:path';
import { pathToFileURL } from 'node:url';

const listMode = process.argv[2] === '--list';
const req = JSON.parse(readFileSync(process.argv[listMode ? 3 : 2], 'utf8'));
const dist = join(req.pkgDir, 'dist', 'src');
const mod = (p) => import(pathToFileURL(join(dist, p)).href);

const { BasePage } = await mod('browser/base-page.js');
const { buildEvaluateExpression } = await mod('browser/utils.js');
const { getRegistry } = await mod('registry-api.js');
const { executePipeline } = await mod('pipeline/index.js');
const { prepareCommandArgs } = await mod('execution.js');
const { shouldUseBrowserSession } = await mod('capabilityRouting.js');

function cu(args, stdin) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env };
    if (req.session) env.AGENT_BROWSER_SESSION = req.session;
    const child = spawn(req.chromeUse, ['--json', ...args], { env, stdio: ['pipe', 'pipe', 'pipe'] });
    let out = '';
    let err = '';
    child.stdout.on('data', (d) => { out += d; });
    child.stderr.on('data', (d) => { err += d; });
    child.on('error', reject);
    child.on('close', () => {
      let resp;
      try { resp = JSON.parse(out.trim().split('\n').pop()); } catch (_) {
        return reject(new Error(`chrome-use ${args[0]}: ${(err || out).trim().slice(0, 500)}`));
      }
      if (!resp.success) return reject(new Error(`chrome-use ${args[0]}: ${resp.error}`));
      resolve(resp.data);
    });
    child.stdin.end(stdin ?? '');
  });
}

class ChromeUsePage extends BasePage {
  async goto(url, options) {
    await cu(['open', url]);
    this._lastUrl = url;
    if (options?.waitUntil !== 'none') {
      const { waitForDomStableJs } = await mod('browser/dom-helpers.js');
      const maxMs = options?.settleMs ?? 1000;
      await this.evaluate(waitForDomStableJs(maxMs, Math.min(500, maxMs))).catch(() => {});
    }
  }
  // One Runtime.evaluate over the extension relay is cut off after ~8s, and
  // adapters often await a search or a load inside the page for longer.
  // `eval --background` hands an expression to chrome-use's adapter runner,
  // which starts it in the page and polls for the value. Statement code
  // (evaluateWithArgs blocks) can't be wrapped as an expression without
  // page-side eval(), which CSP blocks on many sites, so it runs directly.
  async evaluate(input, ...args) {
    const src = buildEvaluateExpression(input, args);
    const data = isExpression(src)
      ? await cu(['eval', '--background', '--stdin'], src)
      : await cu(['eval', '--stdin'], src);
    return data?.result;
  }
  async getCookies(opts = {}) {
    const a = ['cookies', 'get'];
    if (opts.url) a.push('--url', opts.url);
    const data = await cu(a);
    const cookies = Array.isArray(data?.cookies) ? data.cookies : [];
    const d = opts.domain?.replace(/^\./, '');
    return d ? cookies.filter((c) => { const cd = String(c.domain || '').replace(/^\./, ''); return cd === d || cd.endsWith('.' + d) || d.endsWith('.' + cd); }) : cookies;
  }
  async screenshot(options = {}) {
    const dir = mkdtempSync(join(tmpdir(), 'cu-oc-'));
    const file = options.path || join(dir, 'shot.png');
    const a = ['screenshot', file];
    if (options.fullPage) a.push('--full');
    await cu(a);
    const b64 = readFileSync(file).toString('base64');
    rmSync(dir, { recursive: true, force: true });
    return b64;
  }
  async tabs() {
    const data = await cu(['tab', 'list']);
    return data?.tabs ?? [];
  }
  async selectTab(target) {
    await cu(['tab', String(target)]);
  }
  async newTab(url) {
    const data = await cu(url ? ['tab', 'new', url] : ['tab', 'new']);
    return data?.tabId;
  }
  async closeTab(target) {
    await cu(target == null ? ['tab', 'close'] : ['tab', 'close', String(target)]);
  }
  async getCurrentUrl() {
    const data = await cu(['get', 'url']);
    return data?.url ?? null;
  }
  async setFileInput(files, selector) {
    await cu(['upload', selector || 'input[type=file]', ...files]);
  }
  async insertText(text) {
    await this.evaluate((t) => document.execCommand('insertText', false, t), text);
  }
}

function isExpression(src) {
  try {
    // Syntax check only; never called.
    new Function(`return (${src}\n);`);
    return true;
  } catch (_) {
    return false;
  }
}

function hostMatches(url, domain) {
  try { const h = new URL(url).hostname; return h === domain || h.endsWith('.' + domain); } catch (_) { return false; }
}

function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + '\n');
}

// Same test OpenCLI's discovery applies before importing a file.
const MODULE_PATTERN = /\b(?:cli|registerSiteAuthCommands|onStartup|onBeforeExecute|onAfterExecute)\s*\(/;

function adapterFiles(dir) {
  const out = [];
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.name.startsWith('.') || e.name === 'node_modules') continue;
    const p = join(dir, e.name);
    if (e.isDirectory()) out.push(...adapterFiles(p));
    else if (e.name.endsWith('.js') && !e.name.endsWith('.test.js') && !e.name.endsWith('.d.js')) out.push(p);
  }
  return out.sort();
}

// TypeScript-only plugins: OpenCLI compiles their .ts with esbuild when it
// installs a plugin. Node 22.13+ can strip the types itself, so each .ts
// without a .js beside it gets one, in chrome-use's own copy of the source.
function compileTypeScript(dir, errors) {
  const walk = (d) => readdirSync(d, { withFileTypes: true }).flatMap((e) => {
    if (e.name.startsWith('.') || e.name === 'node_modules') return [];
    const p = join(d, e.name);
    return e.isDirectory() ? walk(p) : [p];
  });
  const sources = walk(dir).filter((p) => p.endsWith('.ts') && !p.endsWith('.d.ts') && !p.endsWith('.test.ts'));
  for (const ts of sources) {
    const js = ts.slice(0, -3) + '.js';
    if (existsSync(js)) continue;
    if (typeof nodeModule.stripTypeScriptTypes !== 'function') {
      errors.push(`${relative(dir, ts)}: TypeScript needs Node.js 22.13+ (or a compiled .js beside it)`);
      continue;
    }
    const code = readFileSync(ts, 'utf8');
    // 'strip' everywhere; older Node also has 'transform' (enums, namespaces).
    let out;
    let err;
    for (const opts of [undefined, { mode: 'transform' }]) {
      try { out = nodeModule.stripTypeScriptTypes(code, opts); break; } catch (e) { err = e; }
    }
    if (out === undefined) errors.push(`${relative(dir, ts)}: ${err?.message || err}`);
    else writeFileSync(js, out);
  }
}

async function list() {
  const registry = getRegistry();
  const entries = [];
  const errors = [];
  process.removeAllListeners('warning');
  compileTypeScript(req.clisDir, errors);
  for (const file of adapterFiles(req.clisDir)) {
    if (!MODULE_PATTERN.test(readFileSync(file, 'utf8'))) continue;
    const before = new Set(registry.values());
    try {
      await import(pathToFileURL(file).href);
    } catch (e) {
      errors.push(`${relative(req.clisDir, file)}: ${e?.message || e}`);
      continue;
    }
    const modulePath = relative(req.clisDir, file).split(sep).join('/');
    for (const cmd of new Set(registry.values())) {
      if (before.has(cmd)) continue;
      entries.push({
        site: cmd.site, name: cmd.name, description: cmd.description, access: cmd.access,
        domain: cmd.domain, strategy: cmd.strategy, browser: cmd.browser,
        args: cmd.args ?? [], columns: cmd.columns, type: 'js', modulePath,
      });
    }
  }
  emit({ success: true, data: entries, errors });
}

if (listMode) {
  try {
    await list();
  } catch (e) {
    emit({ success: false, error: e?.message || String(e) });
    process.exitCode = 1;
  }
  // Pipe writes are asynchronous on macOS: exit once the line is flushed.
  // (The exit itself is needed: an imported module may keep handles open.)
  process.stdout.write('', () => process.exit());
  await new Promise(() => {});
}

try {
  await import(pathToFileURL(join(req.clisDir ?? join(req.pkgDir, 'clis'), req.modulePath)).href);
  const cmd = getRegistry().get(`${req.site}/${req.name}`);
  if (!cmd) throw new Error(`opencli: ${req.site}/${req.name} is not defined in ${req.modulePath}`);
  // Mirrors OpenCLI's executeCommand/runCommand (execution.js), minus its own
  // browser transport: same arg preparation, same browser-or-not routing, the
  // same pre-navigation rule (only a URL string in navigateBefore), and the
  // same func signatures — func(kwargs) for browser:false, func(page, kwargs).
  const kwargs = prepareCommandArgs(cmd, req.kwargs ?? {});
  const page = shouldUseBrowserSession(cmd) ? new ChromeUsePage() : null;
  if (page && typeof cmd.navigateBefore === 'string') {
    const current = await page.getCurrentUrl().catch(() => null);
    if (!(cmd.domain && hostMatches(current, cmd.domain))) await page.goto(cmd.navigateBefore);
  }
  let result;
  if (typeof cmd.func === 'function') {
    result = cmd.browser === false ? await cmd.func(kwargs) : await cmd.func(page, kwargs);
  } else if (Array.isArray(cmd.pipeline)) {
    result = await executePipeline(page, cmd.pipeline, { args: kwargs });
  } else {
    throw new Error(`opencli: ${req.site}/${req.name} has neither func nor pipeline`);
  }
  emit({ success: true, data: result ?? null });
} catch (e) {
  emit({ success: false, error: e?.message || String(e), hint: e?.hint, code: e?.code });
  process.exitCode = 1;
}
