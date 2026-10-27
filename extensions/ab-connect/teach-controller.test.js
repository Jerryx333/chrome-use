import assert from 'node:assert/strict'
import test from 'node:test'
import { TeachController, cleanEvent, navEvent, teachKey } from './teach-controller.js'

function fakeChrome({ url = 'https://shop.test/cart', refuse = false } = {}) {
  const store = new Map()
  const calls = []
  const on = () => ({ addListener() {} })
  return {
    calls,
    store,
    runtime: { id: 'ext' },
    tabs: { get: async () => ({ id: 7, url }), onRemoved: on() },
    webNavigation: { onCommitted: on(), onHistoryStateUpdated: on(), onDOMContentLoaded: on() },
    storage: { session: {
      get: async () => Object.fromEntries(store),
      set: async (o) => { for (const [k, v] of Object.entries(o)) store.set(k, v) },
      remove: async (k) => { store.delete(k) },
    } },
    scripting: { executeScript: async (opts) => {
      calls.push(opts)
      if (refuse) throw new Error('Cannot access contents of the page. Extension manifest must request permission')
      return opts.func ? [{ result: true }] : [{}]
    } },
  }
}

test('recording injects the recorder into the top frame only and collects that frame\'s events', async () => {
  const chrome = fakeChrome()
  const c = new TeachController({ chrome, now: () => 5 })
  assert.deepEqual(await c.start(7), { started: true })
  assert.deepEqual(chrome.calls[0], { target: { tabId: 7, frameIds: [0] }, files: ['teach-recorder.js'] })
  assert.ok(chrome.store.has(teachKey(7)))
  const sender = { id: 'ext', tab: { id: 7 }, frameId: 0 }
  assert.equal(c.onEvent({ event: { t: 'click', loc: { css: '#a' }, ts: 1 } }, sender), true)
  assert.equal(c.onEvent({ event: { t: 'click' } }, { ...sender, frameId: 3 }), false, 'child frames are ignored')
  assert.equal(c.onEvent({ event: { t: 'click' } }, { ...sender, id: 'other' }), false)
  assert.equal(c.onEvent({ event: { t: 'evil' } }, sender), false)
  c.onCommitted({ tabId: 7, frameId: 0, url: 'https://shop.test/pay', transitionType: 'link' })
  c.onCommitted({ tabId: 7, frameId: 0, url: 'https://shop.test/', transitionType: 'typed' })
  const { recording } = await c.stop(7)
  assert.equal(recording.startUrl, 'https://shop.test/cart')
  assert.deepEqual(recording.events.map((e) => e.t + (e.reason || '')), ['click', 'nav', 'navtyped'])
  assert.equal(chrome.store.has(teachKey(7)), false)
  assert.ok(chrome.calls.at(-1).func, 'stop uninstalls the recorder')
  await assert.rejects(c.stop(7), /Nothing is being recorded/)
})

test('a site the extension may not script asks for permission; non-web pages are refused', async () => {
  const c = new TeachController({ chrome: fakeChrome({ refuse: true }) })
  assert.deepEqual(await c.start(7), { needsPermission: 'https://shop.test/*' })
  assert.equal(c.isRecording(7), false)
  await assert.rejects(new TeachController({ chrome: fakeChrome({ url: 'chrome://settings' }) }).start(7), /web pages/)
})

test('secret values never survive into a recording', () => {
  assert.equal(cleanEvent({ t: 'input', secret: true, value: 'hunter2', ts: 1 }, 2).value, undefined)
  assert.equal(cleanEvent(null, 1), null)
  assert.deepEqual(navEvent({ url: 'u', transitionType: 'reload' }, 3), { t: 'nav', url: 'u', reason: 'reload', ts: 3 })
})

test('a secret typed into a GET form is blanked from the recorded URLs', async () => {
  const { redactRecording } = await import('./teach-controller.js')
  const r = redactRecording({ startUrl: 'https://a.test/', events: [
    { t: 'input', secret: true, field: 'pw' },
    { t: 'nav', url: 'https://a.test/done?name=Ada&pw=hunter2' },
  ] })
  assert.equal(r.events[1].url, 'https://a.test/done?name=Ada&pw=__redacted__')
})
