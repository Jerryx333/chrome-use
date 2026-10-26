import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { shouldForwardEvent } from './cdp-event-filter.js';

// Exercise the real event handler, including its routing and stale-tab guards.
const source = readFileSync(new URL('./background.js', import.meta.url), 'utf8');
const start = source.indexOf('chrome.debugger.onEvent.addListener(');
const end = source.indexOf('// Tabs `ABExt.releaseTab`', start);
assert.ok(start >= 0 && end > start);

function fixture({ denied = false } = {}) {
  const calls = [], events = [], pending = [];
  const tabs = new Map([[12, { sessionId: 'cb-tab-12' }]]);
  const childSessionToTab = new Map();
  let handler;
  vm.runInNewContext(source.slice(start, end), {
    chrome: { debugger: { onEvent: { addListener(fn) { handler = fn; } } } },
    whenReady: fn => fn(), tabs, childSessionToTab, shouldForwardEvent,
    postToHost: value => events.push(JSON.parse(JSON.stringify(value))),
    trackedDebuggerCommand: (target, method, params) => {
      calls.push(JSON.parse(JSON.stringify({ target, method, params })));
      const result = denied ? Promise.reject(Error('child target unavailable')) : Promise.resolve({});
      pending.push(result.catch(() => {}));
      return result;
    },
  });
  return { calls, events, tabs, childSessionToTab,
    async emit(source, method, params) { handler(source, method, params); await Promise.all(pending); },
  };
}

test('each newly attached iframe arms its children using its own flat session', async () => {
  const f = fixture();
  await f.emit({ tabId: 12 }, 'Target.attachedToTarget', { sessionId: 'child', targetInfo: { type: 'iframe' } });
  await f.emit({ tabId: 12, sessionId: 'child' }, 'Target.attachedToTarget', { sessionId: 'grandchild', targetInfo: { type: 'iframe' } });
  assert.deepEqual(f.calls, ['child', 'grandchild'].map(sessionId => ({
    target: { tabId: 12, sessionId }, method: 'Target.setAutoAttach',
    params: { autoAttach: true, flatten: true, waitForDebuggerOnStart: false,
      filter: [{ type: 'iframe', exclude: false }] },
  })));
  assert.deepEqual([...f.childSessionToTab], [['child', 12], ['grandchild', 12]]);
  assert.deepEqual(f.events.map(e => e.params.sessionId), ['cb-tab-12', 'child']);
  await f.emit({ tabId: 12, sessionId: 'grandchild' }, 'Page.frameNavigated', { frame: { id: 'inner' } });
  assert.equal(f.events.at(-1).params.sessionId, 'grandchild');
});

test('workers, detached sessions and unknown tabs do not initiate frame attachment', async () => {
  const f = fixture();
  await f.emit({ tabId: 12 }, 'Target.attachedToTarget', { sessionId: 'worker', targetInfo: { type: 'worker' } });
  await f.emit({ tabId: 12 }, 'Target.detachedFromTarget', { sessionId: 'worker' });
  assert.equal(f.childSessionToTab.size, 0);
  await f.emit({ tabId: 13 }, 'Target.attachedToTarget', { sessionId: 'foreign', targetInfo: { type: 'iframe' } });
  f.tabs.delete(12);
  await f.emit({ tabId: 12 }, 'Target.attachedToTarget', { sessionId: 'stale', targetInfo: { type: 'iframe' } });
  assert.deepEqual(f.calls, []);
  assert.equal(f.childSessionToTab.size, 0);
});

test('a child setup rejection does not suppress the attached event or stop sibling routing', async () => {
  const f = fixture({ denied: true });
  await f.emit({ tabId: 12 }, 'Target.attachedToTarget', { sessionId: 'denied', targetInfo: { type: 'iframe' } });
  await f.emit({ tabId: 12 }, 'Page.frameNavigated', { frame: { id: 'top' } });
  assert.deepEqual(f.events.map(e => e.params.method), ['Target.attachedToTarget', 'Page.frameNavigated']);
  assert.equal(f.childSessionToTab.get('denied'), 12);
  await f.emit({ tabId: 12 }, 'Target.detachedFromTarget', { sessionId: 'denied' });
  assert.equal(f.childSessionToTab.size, 0);
});
