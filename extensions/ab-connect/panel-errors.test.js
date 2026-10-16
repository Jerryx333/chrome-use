import test from 'node:test'
import assert from 'node:assert/strict'
import { oldBuildBlocking } from './panel-errors.js'

test('the old-build refusal names the conversations to open or stop', () => {
  const id = '12b6a0cd-eca8-4462-a809-16db0688704c'
  assert.deepEqual(oldBuildBlocking(`Another agent-panel build is running for this profile; stop its conversations before updating (still working or waiting for you: “x”) [blocking:${id}]`), [id])
  assert.equal(oldBuildBlocking('Another agent-panel build is running for this profile; stop its conversations before updating'), null)
  assert.equal(oldBuildBlocking('[blocking:../etc]'), null)
})
