import './test-i18n.js'
import assert from 'node:assert/strict'
import test from 'node:test'
import { t } from './i18n.js'
import {
  MIN_CLI_VERSION, UPDATED_NOTICE_MS, WINDOWS_UPGRADE_COMMAND, INSTALL_COMMAND, installCommand, compareVersions, parseCliStatus, cliSwitchPending, versionNotices,
} from './version-notice.js'
import { renderVersionNotices } from './version-banner.js'

const cli = (fields = {}) => parseCliStatus({ version: MIN_CLI_VERSION, latest: MIN_CLI_VERSION, onDisk: MIN_CLI_VERSION, autoUpgrade: true, selfUpgrade: 'ready', lastAutoUpgrade: null, ...fields })
const bump = (v, n = 1) => v.replace(/\d+$/, p => String(Number(p) + n))
const ids = notices => notices.map(n => n.id)

test('versions compare numerically, a release outranks its pre-release, junk is not a version', () => {
  assert.equal(compareVersions('1.5.10', '1.5.9'), 1)
  assert.equal(compareVersions('v1.5.9', '1.5.9'), 0)
  assert.equal(compareVersions('1.5.9-rc.1', '1.5.9'), -1)
  assert.equal(compareVersions('1.5', '1.5.0'), null)
  assert.equal(compareVersions(null, '1.5.0'), null)
})

test('a cli record is validated field by field', () => {
  assert.equal(parseCliStatus(null), null)
  assert.equal(parseCliStatus({ version: 'x' }), null)
  const parsed = parseCliStatus({ version: '1.5.1', latest: 'junk', onDisk: '1.5.2', selfUpgrade: 'weird', lastAutoUpgrade: { at: 5, ok: false, error: 'boom' } })
  assert.deepEqual(parsed, { version: '1.5.1', latest: null, onDisk: '1.5.2', autoUpgrade: false, selfUpgrade: 'disabled', lastAutoUpgrade: { at: 5, ok: false, error: 'boom' } })
  assert.equal(cliSwitchPending(parsed), true)
})

test('nothing to say for a current CLI and no pending update', () => {
  assert.deepEqual(versionNotices({ cli: cli(), update: { pending: false } }), [])
  assert.deepEqual(versionNotices({}), [])
})

test('an outdated CLI offers the button; one that cannot replace itself shows the command', () => {
  const [ready] = versionNotices({ cli: cli({ latest: bump(MIN_CLI_VERSION) }) })
  assert.equal(ready.id, 'cli-outdated')
  assert.equal(ready.action, 'upgradeCli')
  assert.equal(ready.level, 'info', 'auto-upgrade will handle it')
  const [unwritable] = versionNotices({ cli: cli({ latest: bump(MIN_CLI_VERSION), selfUpgrade: 'unwritable', autoUpgrade: false }) })
  assert.equal(unwritable.action, undefined)
  assert.equal(unwritable.command, 'chrome-use upgrade')
  assert.equal(unwritable.level, 'warn')
  const [windows] = versionNotices({ cli: cli({ latest: bump(MIN_CLI_VERSION), selfUpgrade: 'windows' }) })
  assert.equal(windows.command, WINDOWS_UPGRADE_COMMAND)
})

test('a CLI below the minimum says it must be updated', () => {
  const old = '1.0.0'
  const [notice] = versionNotices({ cli: cli({ version: old, onDisk: old, latest: null }) })
  assert.equal(notice.id, 'cli-too-old')
  assert.deepEqual(notice.args, [old, MIN_CLI_VERSION])
  assert.equal(notice.level, 'warn')
})

test('a new binary on disk, a running upgrade and a failed one each win over "outdated"', () => {
  const latest = bump(MIN_CLI_VERSION)
  assert.deepEqual(ids(versionNotices({ cli: cli({ latest, onDisk: latest }) })), ['cli-switch'])
  assert.deepEqual(ids(versionNotices({ cli: cli({ latest }), cliUpgrade: { state: 'running' } })), ['cli-upgrading'])
  const [failed] = versionNotices({ cli: cli({ latest }), cliUpgrade: { state: 'failed', error: 'no network', command: 'x' } })
  assert.equal(failed.id, 'cli-failed')
  assert.equal(failed.command, 'x')
  assert.equal(failed.action, 'upgradeCli')
  const [auto] = versionNotices({ cli: cli({ latest, lastAutoUpgrade: { at: 1, ok: false, error: 'curl failed' } }) })
  assert.equal(auto.id, 'cli-auto-failed')
  assert.equal(auto.command, 'chrome-use upgrade')
})

test('a downloaded extension update can be applied only when the worker says so', () => {
  const [held] = versionNotices({ update: { pending: true, version: '0.5.80', reason: 'command_in_flight', canApplyNow: false } })
  assert.equal(held.disabled, true)
  assert.equal(held.reasonKey, 'ext_version_hold_busy')
  const [ready] = versionNotices({ update: { pending: true, version: '0.5.80', reason: 'idle', canApplyNow: true } })
  assert.equal(ready.disabled, undefined)
  assert.equal(ready.action, 'applyUpdate')
})

test('"updated to" shows for a day after an update, not after an install', () => {
  const now = 10 * UPDATED_NOTICE_MS
  const installed = { version: '0.5.80', reason: 'update', at: now - 1000 }
  assert.deepEqual(ids(versionNotices({ update: { pending: false, installed }, extensionVersion: '0.5.80', now })), ['ext-updated'])
  assert.deepEqual(versionNotices({ update: { pending: false, installed: { ...installed, at: now - UPDATED_NOTICE_MS } }, now }), [])
  assert.deepEqual(versionNotices({ update: { pending: false, installed: { ...installed, reason: 'install' } }, now }), [])
})

class Node {
  constructor(tag) { this.tag = tag; this.children = []; this.dataset = {}; this.listeners = {}; this.textContent = ''; this.hidden = false; this.disabled = false }
  append(...nodes) { this.children.push(...nodes) }
  replaceChildren(...nodes) { this.children = nodes }
  addEventListener(type, fn) { this.listeners[type] = fn }
}
const document = { createElement: tag => new Node(tag) }

test('banners render text, the hand command and a button that reports its action', async () => {
  const container = new Node('div')
  const actions = []
  renderVersionNotices(container, versionNotices({ cli: cli({ latest: bump(MIN_CLI_VERSION) }) }), { document, t, onAction: a => { actions.push(a) } })
  assert.equal(container.hidden, false)
  const [banner] = container.children
  assert.match(banner.children[0].textContent, new RegExp(bump(MIN_CLI_VERSION).replace(/\./g, '\\.')))
  const button = banner.children.find(n => n.tag === 'button')
  assert.equal(button.textContent, 'Update CLI')
  button.listeners.click()
  await new Promise(r => setImmediate(r))
  assert.deepEqual(actions, ['upgradeCli'])
  renderVersionNotices(container, [], { document, t })
  assert.equal(container.hidden, true)
})

test('a legacy CLI gets the strong notice with the reinstall line and no button', () => {
  const [mac] = versionNotices({ legacyCli: true, platform: 'macOS' })
  assert.equal(mac.id, 'cli-legacy')
  assert.equal(mac.level, 'warn')
  assert.equal(mac.action, undefined)
  assert.equal(mac.command, INSTALL_COMMAND)
  assert.equal(versionNotices({ legacyCli: true, platform: 'Win32' })[0].command, WINDOWS_UPGRADE_COMMAND)
  assert.equal(installCommand('Windows'), WINDOWS_UPGRADE_COMMAND)
  assert.equal(installCommand('Linux x86_64'), INSTALL_COMMAND)
  assert.equal(installCommand('MacIntel'), INSTALL_COMMAND)
  // A cli record always wins over the legacy flag.
  assert.deepEqual(versionNotices({ legacyCli: true, cli: cli() }), [])
})
