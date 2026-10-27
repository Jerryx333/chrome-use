import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

import { removeAgentTab } from './remove-agent-tab.js'

function deps({
  ungroupError = null,
  removeError = null,
  agentTab = true,
  inGroup,
  inGroupError = null,
} = {}) {
  const calls = []
  const d = {
    calls,
    isAgentTab: () => agentTab,
    ungroup: async (id) => {
      calls.push(['ungroup', id])
      if (ungroupError) throw new Error(ungroupError)
    },
    remove: async (id) => {
      calls.push(['remove', id])
      if (removeError) throw new Error(removeError)
    },
  }
  // Absent unless asked for: a caller that passes none is an older one.
  if (inGroup !== undefined || inGroupError) {
    d.inAgentGroup = async () => {
      if (inGroupError) throw new Error(inGroupError)
      return inGroup
    }
  }
  return d
}

test('a tab leaves its group before it is removed', async () => {
  const d = deps()
  await removeAgentTab(11, d)
  assert.deepEqual(d.calls, [['ungroup', 11], ['remove', 11]])
})

test('a failed ungroup still removes the tab', async () => {
  const d = deps({ ungroupError: 'Tab is not in a group' })
  await removeAgentTab(11, d)
  assert.deepEqual(d.calls, [['ungroup', 11], ['remove', 11]])
})

test('a failed removal is reported, not swallowed', async () => {
  const d = deps({ removeError: 'No tab with id: 11.' })
  await assert.rejects(removeAgentTab(11, d), /No tab with id: 11/)
})

test("a user's own tab is removed without leaving its group", async () => {
  const d = deps({ agentTab: false })
  await removeAgentTab(11, d)
  assert.deepEqual(d.calls, [['remove', 11]])
})

test('a tab in an agent group leaves it first, though this worker did not make it', async () => {
  const d = deps({ agentTab: false, inGroup: true })
  await removeAgentTab(11, d)
  assert.deepEqual(d.calls, [['ungroup', 11], ['remove', 11]])
})

test("a tab outside an agent group keeps the user's group", async () => {
  const d = deps({ agentTab: false, inGroup: false })
  await removeAgentTab(11, d)
  assert.deepEqual(d.calls, [['remove', 11]])
})

test('an unreadable group answers no, and the tab is removed as before', async () => {
  const d = deps({ agentTab: false, inGroupError: 'No tab with id: 11.' })
  await removeAgentTab(11, d)
  assert.deepEqual(d.calls, [['remove', 11]])
})

test('a caller that passes no group check gets the plain removal', async () => {
  const d = deps({ agentTab: false })
  assert.equal('inAgentGroup' in d, false)
  await removeAgentTab(11, d)
  assert.deepEqual(d.calls, [['remove', 11]])
})

test('the worker answers the group check from the group title', () => {
  const background = readFileSync(new URL('./background.js', import.meta.url), 'utf8')
  const start = background.indexOf('const agentTabRemoval = {')
  assert.ok(start >= 0)
  assert.match(background.slice(start, start + 400), /inAgentGroup: \(tabId\) =>/)
  const check = background.indexOf('async function tabInAgentGroup(tabId) {')
  assert.ok(check >= 0)
  assert.match(background.slice(check, check + 500), /isAgentGroupTitle\(/)
  assert.match(background, /import \{[^}]*isAgentGroupTitle[^}]*\} from '\.\/restored-groups\.js'/)
})
