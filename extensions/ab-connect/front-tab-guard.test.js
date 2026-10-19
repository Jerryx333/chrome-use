import test from 'node:test'
import assert from 'node:assert/strict'
import { FrontTabGuard, RESTORE_WINDOW_MS } from './front-tab-guard.js'

const clock = () => { let t = 1000; return { now: () => t, advance: ms => { t += ms } } }

test('a pop-up activated before the navigation event gives the front back', () => {
  const c = clock(); const g = new FrontTabGuard({ now: c.now })
  g.activated(1, 10)                       // the user's tab
  assert.equal(g.activated(1, 50), null)   // Chrome fronts the pop-up
  assert.equal(g.agentOpened(50, 1, true), 10)
})

test('a pop-up activated after the navigation event gives the front back', () => {
  const c = clock(); const g = new FrontTabGuard({ now: c.now })
  g.activated(1, 10)
  assert.equal(g.agentOpened(50, 1, false), null)
  assert.equal(g.activated(1, 50), 10)
})

test('bringing the tab back is not undone, and the pop-up is restored once', () => {
  const c = clock(); const g = new FrontTabGuard({ now: c.now })
  g.activated(1, 10)
  g.agentOpened(50, 1, false)
  assert.equal(g.activated(1, 50), 10)
  assert.equal(g.activated(1, 10), null)   // our own tabs.update echo
  assert.equal(g.activated(1, 50), null)   // the user switches to the pop-up later
})

test('an activation after the window passes is the user\'s choice', () => {
  const c = clock(); const g = new FrontTabGuard({ now: c.now })
  g.activated(1, 10)
  g.agentOpened(50, 1, false)
  c.advance(RESTORE_WINDOW_MS + 1)
  assert.equal(g.activated(1, 50), null)
})

test('a window with no known front tab is left alone', () => {
  const g = new FrontTabGuard()
  assert.equal(g.agentOpened(50, 2, true), null)
})

test('a closed front tab is not brought back', () => {
  const g = new FrontTabGuard()
  g.activated(1, 10)
  g.removed(10)
  assert.equal(g.agentOpened(50, 1, true), null)
})
