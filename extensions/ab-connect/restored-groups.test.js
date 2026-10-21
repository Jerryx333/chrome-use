import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

import { isAgentGroupTitle, ungroupRestoredAgentGroups } from './restored-groups.js'

test('agent group titles match the CLI', () => {
  for (const title of ['✦ piggo', '✦ chrome-use', '⬉ cu-web', 'cu-web 🤖', 'cu-piggo-4738f0', 'cu-0e261c', ' ✦ x '])
    assert.equal(isAgentGroupTitle(title), true, title)
  for (const title of ['', 'github.com', 'portwind.jp.larksuite.com', 'Claude (MCP)', 'cu-piggo', 'cu-piggo-4738fg', 'cu-Piggo-4738f0', '📦 WeChat 发布检查'])
    assert.equal(isAgentGroupTitle(title), false, title)
})

function env({ groups, tabs, inUse = () => false, ungroupFails = false }) {
  const ungrouped = []
  return {
    ungrouped,
    groups: { query: async () => groups },
    tabs: {
      query: async ({ groupId }) => tabs.filter(t => t.groupId === groupId),
      ungroup: async ids => { if (ungroupFails) throw Error('no'); ungrouped.push(...ids) },
    },
    inUse,
  }
}

test('restored agent groups are ungrouped, without closing a tab', async () => {
  const e = env({
    groups: [{ id: 1, title: '✦ piggo' }, { id: 2, title: 'github.com' }, { id: 3, title: 'cu-web-0e261c' }],
    tabs: [{ id: 10, groupId: 1 }, { id: 11, groupId: 1 }, { id: 20, groupId: 2 }, { id: 30, groupId: 3 }],
  })
  assert.deepEqual(await ungroupRestoredAgentGroups(e), ['✦ piggo', 'cu-web-0e261c'])
  assert.deepEqual(e.ungrouped, [10, 11, 30])
  assert.equal('remove' in e.tabs, false)
})

test('a group a session already drives keeps its tabs', async () => {
  const e = env({ groups: [{ id: 1, title: '✦ piggo' }], tabs: [{ id: 10, groupId: 1 }, { id: 11, groupId: 1 }], inUse: id => id === 11 })
  assert.deepEqual(await ungroupRestoredAgentGroups(e), [])
  assert.deepEqual(e.ungrouped, [])
})

test('failures leave the rest alone', async () => {
  const e = env({ groups: [{ id: 1, title: '✦ piggo' }], tabs: [{ id: 10, groupId: 1 }], ungroupFails: true })
  assert.deepEqual(await ungroupRestoredAgentGroups(e), [])
  const broken = { groups: { query: async () => { throw Error('x') } }, tabs: {}, inUse: () => false }
  assert.deepEqual(await ungroupRestoredAgentGroups(broken), [])
})

test('the worker runs it when the browser starts', () => {
  const background = readFileSync(new URL('./background.js', import.meta.url), 'utf8')
  assert.match(background, /import \{ ungroupRestoredAgentGroups \} from '\.\/restored-groups\.js'/)
  const start = background.indexOf('chrome.runtime.onStartup.addListener(')
  assert.ok(start >= 0)
  assert.match(background.slice(start, start + 400), /ungroupRestoredGroupsAfterStartup\(\)/)
})
