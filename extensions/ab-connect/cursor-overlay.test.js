import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { createCursorOverlay, cursorOverlayExpression, REMOVE_CURSOR_EXPRESSION } from './cursor-overlay.js';
import { STEALTH_DEFAULT, cursorAllowed } from './stealth-mode.js';

// Executes the actual injected expressions. This fixture verifies DOM ownership
// and scheduling, not Chrome rendering or debugger API compatibility.
function page() {
  class Element {
    constructor(root = false) { this.root = root; this.children = []; this.style = {}; }
    get isConnected() { return this.root || !!this.parent?.isConnected; }
    appendChild(child) { this.children.push(child); child.parent = this; return child; }
    attachShadow() { const shadow = new Element(); shadow.parent = this; return shadow; }
    getBoundingClientRect() { return {}; }
    remove() { if (this.parent) this.parent.children = this.parent.children.filter(child => child !== this); this.parent = null; }
  }
  const root = new Element(true), timers = new Map(), frames = new Map();
  let id = 0;
  const context = vm.createContext({
    window: { siteMarker: 'untouched' }, document: { documentElement: root, createElement: () => new Element() },
    setTimeout(fn) { timers.set(++id, fn); return id; }, clearTimeout(id) { timers.delete(id); },
    requestAnimationFrame(fn) { frames.set(++id, fn); return id; }, cancelAnimationFrame(id) { frames.delete(id); },
  });
  return { root, timers, frames, context, run: expression => vm.runInContext(expression, context) };
}

function fixture(intercept) {
  const pages = new Map([[1, page()], [2, page()]]), calls = [], warnings = [];
  const evaluate = async (id, expression) => {
    calls.push({ id, expression });
    const run = () => ({ result: { value: pages.get(id).run(expression) } });
    return intercept ? intercept(id, expression, run) : run();
  };
  const cursor = createCursorOverlay({ evaluate, warn: message => warnings.push(message) });
  return { pages, calls, warnings, cursor, evaluate };
}

function clean(p) {
  assert.deepEqual(Object.getOwnPropertyNames(p.context.window), ['siteMarker']);
  assert.equal(p.root.children.length, 0);
  assert.equal(p.timers.size, 0);
  assert.equal(p.frames.size, 0);
}

const settle = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };

test('default stealth sends no cursor commands and creates no artifacts', async () => {
  const f = fixture();
  await f.cursor.setEnabled(cursorAllowed({ ab_cursor: true }));
  assert.equal(await f.cursor.draw(1, 10, 20, true), false);
  assert.equal(await f.cursor.visible(1, true), false);
  assert.equal(f.calls.length, 0);
  clean(f.pages.get(1));
});

for (const settings of [{ ab_cursor: true, ab_stealth: true }, { ab_cursor: false, ab_stealth: false }]) {
  test(`disabling via ${JSON.stringify(settings)} removes nodes, globals and timers`, async () => {
    const f = fixture();
    await f.cursor.setEnabled(true);
    assert.equal(await f.cursor.draw(1, 10, 20, true), true);
    const old = f.pages.get(1).context.window.__abCursor;
    assert.ok(old.host.isConnected);
    assert.equal(old.sr.children.length, 3, 'cursor, glow and ripple are present');
    await f.cursor.setEnabled(cursorAllowed(settings));
    clean(f.pages.get(1));
    assert.equal(old.host.isConnected, false);
    assert.deepEqual(f.cursor.pendingTabs(), []);
    const count = f.calls.length;
    assert.equal(await f.cursor.draw(1, 30, 40), false);
    assert.equal(f.calls.length, count);
  });
}

test('cleanup follows an in-flight draw and skips queued draws and capture restores', async () => {
  const gate = deferred();
  const f = fixture(async (id, expr, run) => {
    if (expr !== REMOVE_CURSOR_EXPRESSION) await gate.promise;
    return run();
  });
  await f.cursor.setEnabled(true);
  const drawing = f.cursor.draw(1, 10, 20, true);
  await settle();
  const queued = f.cursor.draw(1, 30, 40);
  const restore = f.cursor.visible(1, true);
  const removing = f.cursor.setEnabled(false);
  assert.deepEqual(f.cursor.pendingTabs(), [1]);
  gate.resolve();
  await Promise.all([drawing, removing]);
  assert.equal(await queued, false);
  assert.equal(await restore, false);
  assert.equal(f.calls.length, 2);
  assert.equal(f.calls[1].expression, REMOVE_CURSOR_EXPRESSION);
  clean(f.pages.get(1));
});

test('rapid disable/re-enable does not revive a draw queued under old settings', async () => {
  const gate = deferred();
  let first = true;
  const f = fixture(async (id, expr, run) => {
    if (first) { first = false; await gate.promise; }
    return run();
  });
  await f.cursor.setEnabled(true);
  const a = f.cursor.draw(1, 10, 20);
  await settle();
  const stale = f.cursor.draw(1, 30, 40);
  const remove = f.cursor.setEnabled(false);
  await f.cursor.setEnabled(true);
  const fresh = f.cursor.draw(1, 50, 60);
  gate.resolve();
  await Promise.all([a, remove, fresh]);
  assert.equal(await stale, false);
  assert.equal(f.calls.length, 3);
  assert.equal(f.calls[1].expression, REMOVE_CURSOR_EXPRESSION);
  assert.equal(f.pages.get(1).root.children.length, 1);
  assert.equal(f.pages.get(1).context.window.__abCursor.cur.style.transform, 'translate(50px,60px)');
});

test('a detached tab retains an explicit pending cleanup until normal reattachment', async () => {
  let detached = false;
  const f = fixture((id, expr, run) => {
    if (detached) throw new Error('Debugger is not attached to the tab');
    return run();
  });
  await f.cursor.setEnabled(true);
  await f.cursor.draw(1, 10, 20);
  detached = true;
  await f.cursor.setEnabled(false);
  assert.deepEqual(f.cursor.pendingTabs(), [1]);
  assert.match(f.warnings[0], /cleanup unconfirmed.*reload/i);
  assert.equal(f.pages.get(1).root.children.length, 1);
  detached = false;
  await f.cursor.attached(1);
  clean(f.pages.get(1));
  assert.deepEqual(f.cursor.pendingTabs(), []);
});

test('a false result or page exception never confirms cleanup', async () => {
  for (const reply of [{ result: { value: false } }, { result: { value: true }, exceptionDetails: { text: 'failed' } }]) {
    const f = fixture(() => reply);
    assert.equal(await f.cursor.attached(1), false);
    assert.deepEqual(f.cursor.pendingTabs(), [1]);
    assert.equal(f.warnings.length, 1);
  }
});

test('startup and reattachment remove legacy overlays without previous worker records', async () => {
  const f = fixture();
  const p = f.pages.get(1);
  p.run(cursorOverlayExpression(10, 20, false));
  delete p.context.window.__abCursor.timers;
  delete p.context.window.__abCursor.frames;
  await f.cursor.setEnabled(false, [1]);
  clean(p);
  p.run(cursorOverlayExpression(10, 20, false));
  await f.cursor.attached(1);
  clean(p);
});

test('one stalled tab does not hold up cleanup of another tab', async () => {
  const gate = deferred();
  const f = fixture(async (id, expr, run) => { if (id === 1) await gate.promise; return run(); });
  await f.cursor.setEnabled(true);
  const slow = f.cursor.draw(1, 10, 20);
  await f.cursor.draw(2, 30, 40);
  const removed = f.cursor.setEnabled(false);
  await settle();
  clean(f.pages.get(2));
  gate.resolve();
  await Promise.all([slow, removed]);
  clean(f.pages.get(1));
});

test('navigation before cleanup is harmless; closing cancels work not dispatched', async () => {
  const f = fixture();
  await f.cursor.setEnabled(true);
  await f.cursor.draw(1, 10, 20);
  f.pages.set(1, page());
  await f.cursor.setEnabled(false);
  clean(f.pages.get(1));
  await f.cursor.setEnabled(true);
  const queued = f.cursor.draw(2, 30, 40);
  f.cursor.forget(2);
  assert.equal(await queued, false);
  clean(f.pages.get(2));
});

test('capture visibility and drawing recover correctly after enabling again', async () => {
  const f = fixture();
  await f.cursor.setEnabled(true);
  assert.equal(await f.cursor.visible(1, true), false);
  assert.equal(await f.cursor.draw(1, NaN, 20), false);
  await f.cursor.draw(1, 10, 20);
  assert.equal(await f.cursor.visible(1, false), true);
  assert.equal(f.pages.get(1).context.window.__abCursor.host.style.display, 'none');
  await f.cursor.draw(1, 30, 40);
  assert.equal(f.pages.get(1).context.window.__abCursor.host.style.display, '');
  await f.cursor.setEnabled(false);
  await f.cursor.setEnabled(true);
  await f.cursor.draw(1, 50, 60);
  assert.equal(f.pages.get(1).root.children.length, 1);
});

function backgroundFixture({ delaySettings = false } = {}) {
  const f = fixture();
  let changed, load;
  const source = readFileSync(new URL('./background.js', import.meta.url), 'utf8');
  const start = source.indexOf('// ---- settings + UX');
  const end = source.indexOf('// ---- native messaging transport', start);
  assert.ok(start >= 0 && end > start);
  const worker = vm.createContext({
    createCursorOverlay, STEALTH_DEFAULT, cursorAllowed, IDLE_DETACH_DEFAULT_SECS: 0,
    idleDetachMsFrom: () => 0, withRelayTimeout: promise => promise,
    tabs: new Map([[1, { attached: true }]]),
    chrome: {
      debugger: { sendCommand: (target, method, params) => {
        assert.equal(method, 'Runtime.evaluate');
        return f.evaluate(target.tabId, params.expression);
      } },
      storage: {
        sync: { get: (defaults, callback) => {
          load = () => callback({ ...defaults, ab_cursor: true, ab_stealth: false });
          if (!delaySettings) load();
        } },
        onChanged: { addListener: callback => { changed = callback; } },
      },
    },
  });
  vm.runInContext(source.slice(start, end), worker);
  const driveStart = source.indexOf("  if (method === 'ABExt.driveCursor')");
  const driveEnd = source.indexOf('// On-demand adopt', driveStart);
  assert.ok(driveStart >= 0 && driveEnd > driveStart);
  Object.assign(worker, { resolveSessionTab: () => 1, sessionToTab: new Map(), childSessionToTab: new Map() });
  vm.runInContext(`async function cursorCommand(method, params) { const sessionId = 'test'; ${source.slice(driveStart, driveEnd)} }`, worker);
  return { ...f, worker, changed: (...args) => changed(...args), load: () => load(), run: expr => vm.runInContext(expr, worker) };
}

test('background settings wire both transitions to cleanup on existing connections', async () => {
  const f = backgroundFixture();
  for (const key of ['ab_stealth', 'ab_cursor']) {
    f.changed({ ab_stealth: { newValue: false }, ab_cursor: { newValue: true } }, 'sync');
    f.run("maybeDriveCursor(1, 'Input.dispatchMouseEvent', {x:10,y:20,type:'mousePressed'})");
    await settle();
    assert.equal(f.pages.get(1).root.children.length, 1);
    f.changed({ [key]: { newValue: key === 'ab_stealth' } }, 'sync');
    await settle();
    clean(f.pages.get(1));
  }
});

test('a delayed startup read cannot undo a newer stealth or cursor setting', async () => {
  for (const key of ['ab_stealth', 'ab_cursor']) {
    const f = backgroundFixture({ delaySettings: true });
    f.changed({ [key]: { newValue: key === 'ab_stealth' } }, 'sync');
    f.load();
    f.run("maybeDriveCursor(1, 'Input.dispatchMouseEvent', {x:10,y:20,type:'mousePressed'})");
    await settle();
    clean(f.pages.get(1));
    assert.equal(f.run('cursorEnabled'), false);
  }
});

test('explicit cursor commands report their observed effect and refuse under stealth', async () => {
  const f = backgroundFixture();
  assert.equal((await f.run("cursorCommand('ABExt.driveCursor', {x:10,y:20,click:true})")).drawn, true);
  assert.equal((await f.run("cursorCommand('ABExt.setCursorVisible', {visible:false})")).applied, true);
  assert.equal(f.pages.get(1).context.window.__abCursor.host.style.display, 'none');
  f.worker.chrome.debugger.sendCommand = async () => { throw new Error('Detached'); };
  assert.equal((await f.run("cursorCommand('ABExt.driveCursor', {x:10,y:20})")).reason, 'unconfirmed');
  assert.equal((await f.run("cursorCommand('ABExt.setCursorVisible', {visible:true})")).applied, false);
  f.worker.chrome.debugger.sendCommand = (target, method, params) => f.evaluate(target.tabId, params.expression);
  f.changed({ ab_stealth: { newValue: true } }, 'sync');
  await settle();
  assert.equal((await f.run("cursorCommand('ABExt.driveCursor', {x:10,y:20})")).drawn, false);
  assert.equal((await f.run("cursorCommand('ABExt.setCursorVisible', {visible:true})")).applied, false);
  clean(f.pages.get(1));
});


test('restored tabs enter cleanup inventory even while drawing is enabled', async () => {
  const f = fixture();
  f.pages.get(1).run(cursorOverlayExpression(10, 20, true));
  await f.cursor.setEnabled(true);
  await f.cursor.attached(1);
  assert.equal(f.calls.length, 0, 'registration does not force debugger access');
  await f.cursor.setEnabled(false);
  clean(f.pages.get(1));
});

test('reattachment retries pending cleanup after drawing is enabled again', async () => {
  let detached = false;
  const f = fixture((_id, _expr, run) => {
    if (detached) throw new Error('detached');
    return run();
  });
  await f.cursor.setEnabled(true);
  await f.cursor.draw(1, 10, 20);
  detached = true;
  await f.cursor.setEnabled(false);
  await f.cursor.setEnabled(true);
  detached = false;
  await f.cursor.attached(1);
  clean(f.pages.get(1));
  assert.deepEqual(f.cursor.pendingTabs(), []);
});

test('lazy restoration registers cursor ownership without a debugger attachment', async () => {
  const source = readFileSync(new URL('./background.js', import.meta.url), 'utf8');
  const start = source.indexOf('async function announceOwnedTabLazily(');
  const end = source.indexOf('async function announceAttachedTab(', start);
  const registered = [], tabs = new Map();
  const context = vm.createContext({
    tabs, removedTabs: new Set(), sessionToTab: new Map(), port: null,
    chrome: { debugger: { getTargets: async () => [], detach: async () => {} } },
    withRelayTimeout: p => p, targetInfoForTab: () => ({ targetId: 'T1' }),
    newTabEntry: (_id, sessionId, targetId) => ({ sessionId, targetId, attached: false }),
    cursorOverlay: { attached: async id => registered.push(id) },
    rememberSessionTarget() {}, setBadge() {}, announceAttachedTab: async () => {},
    attachTab: async () => { throw new Error('must not attach'); },
  });
  vm.runInContext(source.slice(start, end), context);
  await context.announceOwnedTabLazily(1);
  assert.deepEqual(registered, [1]);
  assert.equal(tabs.get(1).attached, false);
});
