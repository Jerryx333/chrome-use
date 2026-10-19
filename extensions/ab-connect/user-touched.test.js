import { test } from 'node:test'
import assert from 'node:assert/strict'
import { DWELL_MS, UserTouchedTabs } from './user-touched.js'

function tracker(agentTabs = [7, 8]) {
  let t = 1000
  const tabs = new UserTouchedTabs({ isAgentTab: id => agentTabs.includes(id), now: () => t })
  return { tabs, advance: ms => { t += ms } }
}

test('a switch to an agent tab that sticks makes it the user\'s', () => {
  const { tabs } = tracker()
  assert.equal(tabs.activated(7), true)
  assert.equal(tabs.confirm(7, true), true)
  assert.deepEqual(tabs.list(), [7])
  // Already theirs: nothing more to check.
  assert.equal(tabs.activated(7), false)
})

test('a pop-up Chrome fronted and the guard sent back is not the user\'s', () => {
  const { tabs } = tracker()
  assert.equal(tabs.activated(8), true)
  assert.equal(tabs.confirm(8, false), false)
  assert.deepEqual(tabs.list(), [])
})

test('the user\'s own tabs are never recorded', () => {
  const { tabs } = tracker()
  assert.equal(tabs.activated(3), false)
  assert.equal(tabs.confirm(3, true), false)
  assert.deepEqual(tabs.list(), [])
})

test('an activation the agent asked for does not count', () => {
  const { tabs, advance } = tracker()
  tabs.agentActivating(7)
  assert.equal(tabs.activated(7), false)
  // Only that one: a later switch by the user counts again.
  assert.equal(tabs.activated(7), true)
  // An announcement that was never followed by the activation expires.
  tabs.agentActivating(8)
  advance(DWELL_MS + 1)
  assert.equal(tabs.activated(8), true)
})

test('a closed tab is forgotten and a worker restart restores the rest', () => {
  const { tabs } = tracker()
  tabs.restore([7, 'x', 8])
  assert.deepEqual(tabs.list().sort(), [7, 8])
  assert.equal(tabs.removed(7), true)
  assert.equal(tabs.removed(7), false)
  assert.deepEqual(tabs.list(), [8])
})
