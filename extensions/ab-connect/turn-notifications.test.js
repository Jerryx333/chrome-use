import './test-i18n.js';
import assert from 'node:assert/strict';
import test from 'node:test';
import { TurnNotifications, openFromNotification, turnEvent, turnMessage } from './turn-notifications.js';

const ID = '11111111-2222-4333-8444-555555555555';
function fixture(now = 100) {
  const notices = [], cleared = [], badges = [];
  let saved = {}, clock = now;
  const options = {
    storage: { async get() { return saved; }, async set(value) { saved = structuredClone(value); } },
    notifications: { async create(id, value) { notices.push({ id, value }); }, async clear(id) { cleared.push(id); } },
    badge: text => badges.push(text),
    now: () => clock,
  };
  return { options, notices, cleared, badges, saved: () => saved, tick(at) { clock = at; } };
}
const chat = (fields = {}) => ({ id: ID, agentId: 'codex', title: 'Compare prices', phase: 'running', generation: 1, updated: 90, ...fields });

test('a finished turn notifies once across polls and worker restarts, with its outcome and preview', async () => {
  const f = fixture();
  const c = chat();
  const read = async () => ({ conversations: [c] });
  let notifier = new TurnNotifications(f.options);
  await notifier.poll(read); assert.equal(f.notices.length, 0);
  Object.assign(c, { phase: 'idle', lastTurn: { messageId: 'm1', outcome: 'completed', ended: 120, preview: 'Cheapest is the second shop.' } });
  await notifier.poll(read); await notifier.poll(read);
  assert.equal(f.notices.length, 1);
  assert.deepEqual(f.notices[0], { id: `panel-turn:${ID}`, value: { type: 'basic', iconUrl: 'icons/icon128.png', title: 'Compare prices', message: 'Done: Cheapest is the second shop.' } });
  f.tick(500); notifier = new TurnNotifications(f.options); await notifier.poll(read);
  assert.equal(f.notices.length, 1);
  // The next turn of a conversation the cursor knows notifies even though it
  // ended before this worker started.
  Object.assign(c, { phase: 'failed', lastError: 'Backend process ended before the turn completed\nstack', lastTurn: { messageId: 'm2', outcome: 'failed', ended: 400 } });
  await notifier.poll(read);
  assert.equal(f.notices[1].value.message, 'Failed: Backend process ended before the turn completed');
  // Only ids and turn tokens are stored.
  const stored = JSON.stringify(f.saved());
  for (const text of ['Compare prices', 'Cheapest', 'Backend process']) assert.equal(stored.includes(text), false);
});

test('turns that ended before startup do not notify; later ones do', async () => {
  const f = fixture(1000);
  const old = chat({ phase: 'idle', lastTurn: { messageId: 'old', outcome: 'completed', ended: 900 } });
  const fresh = chat({ id: 'aaaaaaaa-2222-4333-8444-555555555555', phase: 'interrupted', lastTurn: { messageId: 'new', outcome: 'interrupted', ended: 1001 } });
  await new TurnNotifications(f.options).poll(async () => ({ conversations: [old, fresh] }));
  assert.deepEqual(f.notices.map(n => [n.id, n.value.message]), [[`panel-turn:${fresh.id}`, 'Stopped']]);
});

test('a turn the focused panel is showing does not notify or count', async () => {
  const f = fixture();
  const c = chat({ phase: 'idle', lastTurn: { messageId: 'm1', outcome: 'completed', ended: 120 } });
  const asked = [];
  await new TurnNotifications(f.options).poll(async () => ({ conversations: [c] }), { showing: async id => { asked.push(id); return true; } });
  assert.deepEqual(asked, [ID]);
  assert.equal(f.notices.length, 0);
  assert.equal(f.badges.at(-1), '');
});

test('an approval wait notifies once per request, never for full access or scheduled runs', async () => {
  const f = fixture();
  const c = chat({ phase: 'waitingInput', waitingRequest: 7, updated: 110 });
  const scheduled = chat({ id: 'bbbbbbbb-2222-4333-8444-555555555555', scheduleId: 's', phase: 'idle', lastTurn: { messageId: 'x', outcome: 'completed', ended: 130 } });
  const auto = chat({ id: 'cccccccc-2222-4333-8444-555555555555', fullAccess: true, phase: 'waitingInput', waitingRequest: 3, updated: 130 });
  const read = async () => ({ conversations: [c, scheduled, auto] });
  const notifier = new TurnNotifications(f.options);
  await notifier.poll(read); await notifier.poll(read);
  assert.deepEqual(f.notices.map(n => n.value.message), ['Needs your attention: waiting for your approval']);
  c.waitingRequest = 9; await notifier.poll(read);
  assert.equal(f.notices.length, 2);
  // An unconfirmed outcome after a service restart needs the user too.
  Object.assign(c, { phase: 'recoveryRequired', waitingRequest: null, activeMessageId: 'm1', updated: 140 });
  await notifier.poll(read);
  assert.equal(f.notices[2].value.message, 'Needs your attention: the outcome is unconfirmed');
});

test('the badge counts unseen turns and clears when the panel shows them or a new turn starts', async () => {
  const f = fixture();
  const a = chat({ phase: 'idle', lastTurn: { messageId: 'm1', outcome: 'completed', ended: 120 } });
  const b = chat({ id: 'dddddddd-2222-4333-8444-555555555555', phase: 'failed', lastTurn: { messageId: 'm2', outcome: 'failed', ended: 130 } });
  const read = async () => ({ conversations: [a, b] });
  const notifier = new TurnNotifications(f.options);
  await notifier.poll(read);
  assert.equal(f.badges.at(-1), '2');
  await notifier.seen(ID);
  assert.equal(f.badges.at(-1), '1');
  assert.deepEqual(f.cleared, [`panel-turn:${ID}`]);
  // A restarted worker keeps the count.
  const restarted = new TurnNotifications(f.options);
  b.phase = 'running'; await restarted.poll(read);
  assert.equal(f.badges.at(-1), '');
  // Turning the setting off records turns silently and clears the badge.
  Object.assign(b, { phase: 'idle', lastTurn: { messageId: 'm3', outcome: 'completed', ended: 140 } });
  await restarted.poll(read, { enabled: false });
  assert.equal(f.notices.length, 2); assert.equal(f.badges.at(-1), '');
  await restarted.poll(read);
  assert.equal(f.notices.length, 2);
});

test('summaries map to events and plain-words messages', () => {
  assert.equal(turnEvent(chat()), null);
  assert.equal(turnEvent(chat({ phase: 'idle' })), null, 'an old host reports no lastTurn');
  assert.equal(turnEvent(chat({ phase: 'waitingInput', waitingRequest: null })), null);
  assert.equal(turnEvent(chat({ phase: 'idle', lastTurn: { messageId: 'm', outcome: 'completed', ended: 5 } })).token, 't:m');
  const long = 'x'.repeat(300);
  assert.equal(turnMessage({ lastTurn: { preview: long } }, 'completed').length, 120);
  assert.equal(turnMessage({}, 'unknown'), 'Needs your attention: the outcome is unconfirmed');
});

test('overlapping checks do not duplicate a notification', async () => {
  let resolve; const pause = new Promise(r => { resolve = r; }); let count = 0;
  const notifier = new TurnNotifications({ storage: { async get() { return {}; }, async set() {} }, notifications: { async create() { count++; await pause; } }, now: () => 100 });
  const read = async () => ({ conversations: [chat({ phase: 'idle', lastTurn: { messageId: 'm', outcome: 'completed', ended: 110 } })] });
  const first = notifier.poll(read); await new Promise(r => setImmediate(r));
  await notifier.poll(read); assert.equal(count, 1); resolve(); await first;
});

test('a click opens the side panel on the conversation before awaiting anything', async () => {
  const calls = [];
  const chrome = {
    storage: { local: { set: async value => { calls.push(['store', value]); } } },
    sidePanel: { open: options => { calls.push(['open', options]); return Promise.resolve(); } },
    windows: { update: async (id, options) => { calls.push(['focus', id, options]); } },
    notifications: { clear: async id => { calls.push(['clear', id]); } },
  };
  const here = { posted: [], postMessage(message) { this.posted.push(message); } };
  const elsewhere = { posted: [], postMessage(message) { this.posted.push(message); } };
  const panels = new Map([[here, { windowId: 4 }], [elsewhere, { windowId: 5 }]]);
  let pending = null;
  assert.equal(openFromNotification(`panel-turn:${ID}`, { windowId: 4, chrome, panels, pending: value => { pending = value; } }), true);
  // Synchronous: the open call carries the click's gesture.
  assert.deepEqual(calls.slice(0, 2), [['store', { ab_panel_conversation: ID }], ['open', { windowId: 4 }]]);
  assert.deepEqual(here.posted, [{ type: 'select', conversationId: ID }]);
  assert.deepEqual(elsewhere.posted, []);
  assert.equal(pending.conversationId, ID); assert.equal(pending.windowId, 4);
  assert.deepEqual(calls.slice(2), [['focus', 4, { focused: true }], ['clear', `panel-turn:${ID}`]]);
  assert.equal(openFromNotification(`schedule-run:${ID}`, { windowId: 4, chrome, panels }), true);
  assert.equal(openFromNotification('connection', { windowId: 4, chrome, panels }), false);
  // A click that woke the worker looks the window up first.
  calls.length = 0; chrome.windows.getLastFocused = async () => ({ id: 9 });
  openFromNotification(`panel-turn:${ID}`, { windowId: null, chrome, panels, pending: value => { pending = value; } });
  await new Promise(r => setImmediate(r));
  assert.deepEqual(calls.find(call => call[0] === 'open'), ['open', { windowId: 9 }]);
  assert.equal(pending.windowId, 9);
});

test('acknowledging a recovery does not re-announce the last finished turn', async () => {
  const f = fixture();
  const c = chat({ phase: 'idle', lastTurn: { messageId: 'm1', outcome: 'completed', ended: 120 } });
  const read = async () => ({ conversations: [c] });
  const notifier = new TurnNotifications(f.options);
  await notifier.poll(read); assert.equal(f.notices.length, 1);
  // The service restarted during the next turn; the user then acknowledged it.
  Object.assign(c, { phase: 'recoveryRequired', activeMessageId: 'm2', updated: 150 });
  await notifier.poll(read); assert.equal(f.notices.length, 2);
  Object.assign(c, { phase: 'idle', activeMessageId: null, updated: 160 });
  await notifier.poll(read); assert.equal(f.notices.length, 2);
  Object.assign(c, { phase: 'idle', lastTurn: { messageId: 'm3', outcome: 'completed', ended: 170 } });
  await notifier.poll(read); assert.equal(f.notices.length, 3);
});

test('a turn seen running notifies even if it ends while the worker is stopped', async () => {
  const f = fixture(100);
  const c = chat({ phase: 'running', activeMessageId: 'm1', updated: 50 });
  const read = async () => ({ conversations: [c] });
  await new TurnNotifications(f.options).poll(read);
  Object.assign(c, { phase: 'idle', activeMessageId: null, lastTurn: { messageId: 'm1', outcome: 'completed', ended: 150 } });
  f.tick(1000); await new TurnNotifications(f.options).poll(read);
  assert.equal(f.notices.length, 1);
});

test('agent replies to Inspect & Ask batches notify once, count on the badge and clear when shown', async () => {
  const f = fixture();
  const replies = [{ channel: 'dm-a', seq: 3, from: 'old-agent', text: 'before', receivedAt: 50 }];
  const read = async () => ({ replies });
  let notifier = new TurnNotifications(f.options);
  // The first poll only marks what the host already held.
  await notifier.pollReplies(read);
  assert.equal(f.notices.length, 0);
  replies.push({ deliveryId: ID, channel: 'dm-a', seq: 4, from: 'chrome-use-91', text: 'Fixed the button\nsecond line', receivedAt: 200 });
  await notifier.pollReplies(read); await notifier.pollReplies(read);
  assert.deepEqual(f.notices, [{ id: 'inspect-reply:dm-a#4', value: { type: 'basic', iconUrl: 'icons/icon128.png', title: 'Reply from chrome-use-91', message: 'Fixed the button' } }]);
  assert.equal(f.badges.at(-1), '1');
  // A conversation poll keeps the reply's badge entry.
  await notifier.poll(async () => ({ conversations: [] }));
  assert.equal(f.badges.at(-1), '1');
  // A restarted worker does not announce it again.
  notifier = new TurnNotifications(f.options);
  await notifier.pollReplies(read);
  assert.equal(f.notices.length, 1);
  await notifier.repliesSeen();
  assert.equal(f.badges.at(-1), '');
  assert.deepEqual(f.cleared, ['inspect-reply:dm-a#4']);
  assert.equal(JSON.stringify(f.saved()).includes('Fixed the button'), false, 'reply text is never stored');
  // Disabled: nothing is shown, and the cursor still moves past it.
  replies.push({ channel: 'dm-a', seq: 5, from: 'x', text: 'quiet', receivedAt: 300 });
  await notifier.pollReplies(read, { enabled: false });
  await notifier.pollReplies(read);
  assert.equal(f.notices.length, 1);
});

test('a reply notification opens the side panel at its replies', () => {
  const calls = [];
  const chrome = {
    storage: { local: { set: async value => { calls.push(['store', Object.keys(value)]); } } },
    sidePanel: { open: options => { calls.push(['open', options]); return Promise.resolve(); } },
    windows: { update: async (id, options) => { calls.push(['focus', id, options]); } },
    notifications: { clear: async id => { calls.push(['clear', id]); } },
  };
  const here = { posted: [], postMessage(message) { this.posted.push(message); } };
  assert.equal(openFromNotification('inspect-reply:dm-a#4', { windowId: 4, chrome, panels: new Map([[here, { windowId: 4 }]]) }), true);
  assert.deepEqual(calls, [['store', ['ab_panel_show_replies']], ['open', { windowId: 4 }], ['focus', 4, { focused: true }], ['clear', 'inspect-reply:dm-a#4']]);
  assert.deepEqual(here.posted, [{ type: 'showReplies' }]);
});

test('with one panel per tab, a finished turn opens its conversation\'s own tab, else binds the tab the user is on', () => {
  const calls = [];
  const chrome = { storage: { local: { set: async value => calls.push(['store', Object.keys(value)]) } }, windows: { update: async (id, value) => calls.push(['focus', id, value]) }, notifications: { clear: async id => calls.push(['clear', id]) }, sidePanel: { open: async value => calls.push(['global-open', value]) } };
  const bound = new Map([[21, ID]]);
  const tabs = { tabFor: id => [...bound].find(([, c]) => c === id)?.[0] ?? null, activeTab: () => 30, bind: (tabId, id) => { calls.push(['bind', tabId, id]); bound.set(tabId, id); }, open: tabId => calls.push(['open', tabId]), show: tabId => { calls.push(['show', tabId]); return { id: tabId, windowId: 4 }; } };
  const there = { posted: [], postMessage(m) { this.posted.push(m); } }, other = { posted: [], postMessage(m) { this.posted.push(m); } };
  const panels = new Map([[there, { windowId: 4, tabId: 21 }], [other, { windowId: 4, tabId: 30 }]]);
  assert.equal(openFromNotification(`panel-turn:${ID}`, { windowId: 4, chrome, panels, tabs }), true);
  // Synchronous, so open() still has the click's gesture.
  assert.deepEqual(calls.slice(0, 2), [['show', 21], ['open', 21]]);
  assert.deepEqual(there.posted, [{ type: 'select', conversationId: ID }]);
  assert.deepEqual(other.posted, []);
  assert.ok(!calls.some(call => call[0] === 'global-open'));
  // A conversation no tab shows opens in the tab the user is on.
  calls.length = 0; bound.clear();
  const SECOND = 'aaaaaaaa-2222-4333-8444-555555555555';
  openFromNotification(`schedule-run:${SECOND}`, { windowId: 4, chrome, panels, tabs });
  assert.deepEqual(calls.slice(0, 2), [['bind', 30, SECOND], ['open', 30]]);
  assert.deepEqual(other.posted, [{ type: 'select', conversationId: SECOND }]);
  // Replies open the panel of the tab the user is on.
  calls.length = 0;
  openFromNotification('inspect-reply:dm-a#4', { windowId: 4, chrome, panels, tabs });
  assert.deepEqual(calls.slice(0, 2), [['store', ['ab_panel_show_replies']], ['open', 30]]);
});

// Use the shipped background wiring so activation must return Chrome's Tab.
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const backgroundSource = readFileSync(new URL('./background.js', import.meta.url), 'utf8');
function notificationActivation(update) {
  const start = backgroundSource.indexOf('const notificationPanels = {');
  const end = backgroundSource.indexOf('async function panelShowing', start);
  assert.ok(start >= 0 && end > start);
  return vm.runInNewContext(`${backgroundSource.slice(start, end)}; notificationPanels`, {
    chrome: { tabs: { update } },
  }).show;
}

for (const kind of ['panel-turn', 'schedule-run']) {
  test(`${kind} click focuses the activated tab's current window without delaying panel open`, async () => {
    const calls = [];
    let activated;
    const chrome = {
      windows: { update: async (id, options) => calls.push(['focus', id, options]) },
      notifications: { clear: async () => {} },
    };
    const tabs = {
      tabFor: () => 21,
      open: id => calls.push(['open', id]),
      show: notificationActivation((id, options) => {
        calls.push(['activate', id, options]);
        return new Promise(resolve => { activated = resolve; });
      }),
    };
    // Stored panel presence can be stale if the tab moved to a different window.
    const panels = new Map([[{ postMessage() {} }, { tabId: 21, windowId: 7 }]]);
    assert.equal(openFromNotification(`${kind}:${ID}`, { windowId: 4, chrome, panels, tabs }), true);
    assert.equal(calls[0][0], 'activate');
    assert.deepEqual(calls[1], ['open', 21], 'open is synchronous with the notification gesture');
    assert.deepEqual(calls.filter(([type]) => type === 'focus'), []);
    activated({ id: 21, windowId: 9, active: true });
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(calls.filter(([type]) => type === 'focus'), [['focus', 9, { focused: true }]]);
  });
}

for (const failure of ['rejected', 'missing tab']) {
  test(`a ${failure} activation never focuses the unrelated last active window`, async t => {
    const focused = [], warnings = [], opened = [];
    t.mock.method(console, 'warn', (...args) => warnings.push(args));
    const chrome = { windows: { update: async id => focused.push(id) }, notifications: { clear: async () => {} } };
    const tabs = { tabFor: () => 21, open: id => opened.push(id), show: notificationActivation(async () => {
      if (failure === 'rejected') throw new Error('No tab with id: 21');
      return undefined;
    }) };
    openFromNotification(`panel-turn:${ID}`, { windowId: 4, chrome, panels: new Map(), tabs });
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(opened, [21]);
    assert.deepEqual(focused, []);
    assert.equal(warnings.length, 1, 'activation uncertainty is reported');
  });
}

for (const kind of ['panel-turn', 'schedule-run']) {
  for (const windowId of [null, undefined]) {
    test(`${kind} routes to its bound tab before the current window is known (${windowId})`, async () => {
      const calls = [];
      const chrome = {
        storage: { local: { set: async value => calls.push(['store', value]) } },
        windows: { getLastFocused: async () => { calls.push(['last-focused']); return { id: 99 }; }, update: async id => calls.push(['focus', id]) },
        sidePanel: { open: async value => calls.push(['global-open', value]) },
        notifications: { clear: async () => {} },
      };
      const tabs = {
        tabFor: () => 21,
        activeTab: () => { throw new Error('A bound tab must not use the current-window fallback'); },
        show: notificationActivation(async id => { calls.push(['show', id]); return { id, windowId: 7 }; }),
        open: id => calls.push(['open', id]),
      };
      assert.equal(openFromNotification(`${kind}:${ID}`, { windowId, chrome, panels: new Map(), tabs }), true);
      assert.deepEqual(calls.slice(0, 2), [['show', 21], ['open', 21]], 'side panel opens synchronously in the notification gesture');
      await new Promise(resolve => setImmediate(resolve));
      assert.deepEqual(calls, [['show', 21], ['open', 21], ['focus', 7]]);
    });
  }
}

for (const id of [`panel-turn:${ID}`, 'inspect-reply:dm-a#4']) {
  test(`an unbound ${id.split(':')[0]} keeps its last-window fallback when no current window is known`, async () => {
    const calls = [];
    const chrome = {
      storage: { local: { set: async () => {} } },
      windows: { getLastFocused: async () => ({ id: 99 }), update: async id => calls.push(['focus', id]) },
      sidePanel: { open: async value => calls.push(['open', value]) }, notifications: { clear: async () => {} },
    };
    const tabs = { tabFor: () => null, activeTab: () => { throw new Error('No current window is available'); } };
    assert.equal(openFromNotification(id, { windowId: null, chrome, panels: new Map(), tabs }), true);
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(calls, [['open', { windowId: 99 }], ['focus', 99]]);
  });
}
