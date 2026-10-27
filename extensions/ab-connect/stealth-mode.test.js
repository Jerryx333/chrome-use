import assert from 'node:assert/strict'
import test from 'node:test'
import { STEALTH_DEFAULT, stealthOn, cursorAllowed, stealthBlockReason } from './stealth-mode.js'

test('stealth is on by default and only an explicit false turns it off', () => {
  assert.equal(STEALTH_DEFAULT, true)
  assert.equal(stealthOn(undefined), true)
  assert.equal(stealthOn({}), true)
  assert.equal(stealthOn({ ab_stealth: true }), true)
  assert.equal(stealthOn({ ab_stealth: 0 }), true)
  assert.equal(stealthOn({ ab_stealth: false }), false)
})

test('the agent cursor is drawn only with stealth off and the cursor on', () => {
  assert.equal(cursorAllowed({ ab_cursor: true }), false, 'an old ab_cursor setting stays dark under stealth')
  assert.equal(cursorAllowed({ ab_cursor: true, ab_stealth: true }), false)
  assert.equal(cursorAllowed({ ab_cursor: false, ab_stealth: false }), false)
  assert.equal(cursorAllowed({ ab_cursor: true, ab_stealth: false }), true)
})

test('page-visible CDP methods are refused only while stealth is on', () => {
  assert.match(stealthBlockReason(undefined, 'Runtime.addBinding'), /stealth mode is on/)
  assert.match(stealthBlockReason({ ab_stealth: true }, 'Runtime.addBinding'), /options page/)
  assert.equal(stealthBlockReason({ ab_stealth: false }, 'Runtime.addBinding'), null)
  assert.equal(stealthBlockReason({ ab_stealth: true }, 'Runtime.evaluate'), null)
  assert.equal(stealthBlockReason({ ab_stealth: true }, 'Fetch.fulfillRequest'), null)
})

test('a binding limited to a named isolated world passes under stealth (teach recording)', () => {
  const isolated = { name: 'b1', executionContextName: 'w1' }
  assert.equal(stealthBlockReason({ ab_stealth: true }, 'Runtime.addBinding', isolated), null)
  assert.equal(stealthBlockReason(undefined, 'Runtime.addBinding', isolated), null)
  // No name, an empty one, or a context id (which may be the main world) stay refused.
  assert.match(stealthBlockReason({ ab_stealth: true }, 'Runtime.addBinding', { name: 'b1' }), /stealth mode is on/)
  assert.match(stealthBlockReason({ ab_stealth: true }, 'Runtime.addBinding', { name: 'b1', executionContextName: '' }), /stealth mode is on/)
  assert.match(stealthBlockReason({ ab_stealth: true }, 'Runtime.addBinding', { name: 'b1', executionContextName: 'w1', executionContextId: 3 }), /stealth mode is on/)
})
