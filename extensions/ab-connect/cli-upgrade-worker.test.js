// The worker half of keeping the CLI current: the "Update CLI" button posts
// `upgradeCli`, a refusal or a failed background install becomes a failure
// notice, and a newer binary on disk is switched to only when the relay is
// idle, once per version. The real background.js section runs in a vm.
import './test-i18n.js'
import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import vm from 'node:vm'
import { t } from './i18n.js'
import { cliSwitchPending, compareVersions } from './version-notice.js'

const source = readFileSync(new URL('./background.js', import.meta.url), 'utf8')
const from = source.indexOf('// ---- keeping the CLI and this extension current')
const to = source.indexOf('function connectHost()')
const section = source.slice(from, to)

function worker({ cli, plan = { apply: true, appliesInMs: 0 }, pending = 0 } = {}) {
  const posted = [], timers = [], calls = { disconnect: 0, connect: 0, gone: [], published: 0 }
  const host = { cli }
  const port = { disconnect() { calls.disconnect++ } }
  const context = {
    Date, JSON, Number,
    t, cliSwitchPending, compareVersions,
    setTimeout: (fn, ms) => { const timer = { fn, ms }; timers.push(timer); return timer },
    clearTimeout: timer => { const i = timers.indexOf(timer); if (i >= 0) timers.splice(i, 1) },
    port,
    hostConnection: { snapshot: () => ({ cli: host.cli }) },
    postToHost: msg => posted.push(msg),
    publishHostStatus: () => { calls.published++ },
    currentUpdatePlan: () => plan,
    updateStateSnapshot: () => ({ pending: false }),
    updatePending: false, applyingUpdate: false,
    agentPanelBridge: { pending: { size: pending } },
    hostGone: (p, why) => { calls.gone.push(why); context.port = null },
    connectHost: () => { calls.connect++ },
    chrome: { runtime: { getManifest: () => ({ version: '0.5.72' }) } },
  }
  vm.createContext(context)
  vm.runInContext(section, context)
  return { context, posted, timers, calls, host, run: expr => vm.runInContext(expr, context) }
}

const record = (fields = {}) => ({ version: '1.5.235', latest: '1.5.240', onDisk: '1.5.235', autoUpgrade: false, selfUpgrade: 'ready', lastAutoUpgrade: null, ...fields })

test('the button asks the host and pings until the result is in', () => {
  const w = worker({ cli: record() })
  w.run('startCliUpgrade()')
  assert.deepEqual(JSON.parse(JSON.stringify(w.posted)), [{ method: 'upgradeCli' }])
  assert.equal(w.run('cliUpgrade.state'), 'running')
  w.timers.shift().fn()
  assert.equal(w.posted.at(-1).method, 'ping')
  // A second click while it runs sends nothing new.
  w.run('startCliUpgrade()')
  assert.equal(w.posted.filter(m => m.method === 'upgradeCli').length, 1)
})

test('a refused upgrade becomes a failure with the manual command', () => {
  const w = worker({ cli: record() })
  w.run('startCliUpgrade()')
  w.run(`onUpgradeCliResult({ started: false, error: 'not writable', command: 'chrome-use upgrade' })`)
  assert.deepEqual(JSON.parse(w.run('JSON.stringify(cliUpgrade)')), { state: 'failed', error: 'not writable', command: 'chrome-use upgrade', at: w.run('cliUpgrade.at') })
  assert.equal(w.timers.length, 0, 'pings stop')
})

test('a failed background install reported in the pong ends a running upgrade', () => {
  const w = worker({ cli: record() })
  w.run('startCliUpgrade()')
  w.host.cli = record({ lastAutoUpgrade: { at: Date.now() + 1, ok: false, error: 'curl: (6)' } })
  w.run('afterCliStatus()')
  assert.equal(w.run('cliUpgrade.state'), 'failed')
  assert.equal(w.run('cliUpgrade.error'), 'curl: (6)')
})

test('a newer binary on disk is switched to when idle, once per version', () => {
  const w = worker({ cli: record() })
  w.run('startCliUpgrade()')
  w.host.cli = record({ onDisk: '1.5.240' })
  w.run('afterCliStatus()')
  assert.equal(w.run('cliUpgrade'), null, 'installed: the switch notice takes over')
  assert.equal(w.calls.disconnect, 1)
  assert.equal(w.calls.connect, 1)
  assert.equal(w.calls.gone.length, 1)
  // The new host still reports the old version (installed elsewhere): no loop.
  w.context.port = {}
  w.run('afterCliStatus()')
  assert.equal(w.calls.connect, 1)
})

test('a busy relay or a side panel request in flight holds the switch', () => {
  const busy = worker({ cli: record({ onDisk: '1.5.240' }), plan: { apply: false, appliesInMs: 40_000 } })
  busy.run('afterCliStatus()')
  assert.equal(busy.calls.disconnect, 0)
  assert.equal(busy.timers.at(-1).ms, 40_250, 're-checked when the quiet period ends')
  const panel = worker({ cli: record({ onDisk: '1.5.240' }), pending: 1 })
  panel.run('afterCliStatus()')
  assert.equal(panel.calls.disconnect, 0)
})

test('version state carries the upgrade record and whether an update can apply now', () => {
  const w = worker({ cli: record() })
  const state = JSON.parse(w.run('JSON.stringify(versionState())'))
  assert.deepEqual(state, { cliUpgrade: null, update: { pending: false, canApplyNow: false }, extensionVersion: '0.5.72' })
})
