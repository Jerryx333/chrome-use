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
