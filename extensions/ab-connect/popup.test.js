import './test-i18n.js'
import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import vm from 'node:vm'
import { t, applyI18n } from './i18n.js'
import { versionNotices } from './version-notice.js'
import { renderVersionNotices } from './version-banner.js'

// popup.js is a module; vm runs it as a script with its i18n import supplied.
const source = readFileSync(new URL('./popup.js', import.meta.url), 'utf8')
  .replace(/^import .*\n/gm, '')

function popup(initial, extensionAvailable = true, deferred = false) {
  const callbacks = [], timers = []
  const nodes = new Map()
  let changed
  const context = {
    document: {
      getElementById(id) {
        if (!nodes.has(id)) nodes.set(id, { textContent: '', style: {}, children: [],
          classList: { add() {}, remove() {} }, replaceChildren(...c) { this.children = c } })
        return nodes.get(id)
      },
      querySelectorAll() { return [] },
      createElement: tag => ({ tag, children: [], dataset: {}, textContent: '', append(...c) { this.children.push(...c) }, addEventListener() {} }),
    },
    chrome: { runtime: {
      sendMessage(_request, callback) { if (deferred) callbacks.push(callback); else callback(initial) },
      onMessage: { addListener(fn) { changed = fn } },
    } },
    setTimeout(callback, delay) { timers.push({ callback, delay }) },
    t, applyI18n, versionNotices, renderVersionNotices,
  }
  if (!extensionAvailable) delete context.chrome
  vm.runInNewContext(source, context)
  return { nodes, change(state) { changed({ type: 'ab-host-state', ...state }) },
    reply(index, state) { callbacks[index](state) }, pollAgain() { timers.find(t => t.delay === 700).callback() } }
}

test('popup waits for confirmation and receives a late confirmed connection', () => {
  const p = popup({ connected: false, connectionState: 'connecting' })
  assert.equal(p.nodes.get('statusLabel').textContent, 'Connecting…')
  p.change({ connected: true, connectionState: 'connected', tabCount: 2 })
  assert.equal(p.nodes.get('statusLabel').textContent, 'Connected')
  assert.equal(p.nodes.get('tabPill').textContent, '2 tabs')
})

test('native host failure updates an open popup with the actual error', () => {
  const p = popup({ connected: false, connectionState: 'connecting' })
  p.change({ connected: false, connectionState: 'disconnected',
    connectionError: 'Specified native messaging host not found.' })
  assert.equal(p.nodes.get('statusLabel').textContent, 'Not paired')
  assert.equal(p.nodes.get('statusSub').textContent, 'Specified native messaging host not found.')
})


test('standalone preview does not invent a connection or tab count', () => {
  const p = popup(null, false)
  assert.equal(p.nodes.get('statusLabel').textContent, 'Not paired')
  assert.equal(p.nodes.get('statusSub').textContent, 'Open this popup from the chrome-use extension')
  assert.equal(p.nodes.get('hint').style.display, 'none')
  assert.equal(p.nodes.get('hintCommand').textContent, '')
})


test('a disconnected installed host is diagnosed without suggesting reinstallation', () => {
  const p = popup({ connected: true, connectionState: 'connected' })
  p.change({ connected: false, connectionState: 'disconnected', connectionError: 'Native host has exited.' })
  assert.equal(p.nodes.get('hintCommand').textContent, 'chrome-use status')
  p.change({ connected: false, connectionState: 'disconnected', connectionError: 'Specified native messaging host not found.' })
  assert.equal(p.nodes.get('hintCommand').textContent, 'chrome-use extension install')
})


test('a late startup response cannot overwrite a newer pushed connection state', () => {
  const p = popup(null, true, true)
  p.change({ connected: true, connectionState: 'connected' })
  p.reply(0, { connected: false, connectionState: 'connecting' })
  assert.equal(p.nodes.get('statusLabel').textContent, 'Connected')
})

test('an older query response cannot overwrite the latest query response', () => {
  const p = popup(null, true, true)
  p.pollAgain()
  p.reply(1, { connected: true, connectionState: 'connected' })
  p.reply(0, { connected: false, connectionState: 'disconnected' })
  assert.equal(p.nodes.get('statusLabel').textContent, 'Connected')
})

test('popup text follows the Chrome UI language', async () => {
  const { installI18n } = await import('./test-i18n.js')
  installI18n('zh_CN')
  try {
    const p = popup({ connected: true, connectionState: 'connected', tabCount: 2 })
    assert.equal(p.nodes.get('statusLabel').textContent, '已连接')
    assert.equal(p.nodes.get('tabPill').textContent, '2 个标签页')
  } finally { installI18n('en') }
})

test('popup shows an outdated CLI and a waiting extension update', () => {
  const p = popup({ connected: true, connectionState: 'connected', tabCount: 0,
    cli: { version: '1.5.235', latest: '1.5.240', onDisk: '1.5.235', autoUpgrade: false, selfUpgrade: 'ready', lastAutoUpgrade: null },
    update: { pending: true, version: '0.5.80', reason: 'recent_activity', canApplyNow: false } })
  const box = p.nodes.get('versionNotices')
  assert.equal(box.hidden, false)
  const texts = box.children.map(b => b.children[0].textContent)
  assert.match(texts[0], /1\.5\.240 is available \(you have 1\.5\.235\)/)
  assert.match(texts[1], /0\.5\.80 is downloaded/)
  p.change({ connected: true, connectionState: 'connected', tabCount: 0 })
  assert.equal(box.hidden, true)
})
