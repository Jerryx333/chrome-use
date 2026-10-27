import test from 'node:test'
import assert from 'node:assert/strict'
import { blurEditableField, blurFocusedField } from './menu-blur.js'

const frames = [
  { frameId: 0, url: 'https://app.example/login' },
  { frameId: 4, url: 'chrome-extension://nngceckbapebfimnlniiiahkandclblb/overlay/menu-button.html' },
  { frameId: 6, url: 'https://idmsa.example/signin' },
  { frameId: 7, url: 'about:blank' },
]

function deps(overrides = {}) {
  const calls = []
  return {
    calls,
    isOwned: (id) => id === 7,
    getAllFrames: async () => frames,
    executeScript: async (injection) => {
      calls.push(injection)
      return injection.target.frameIds.map((frameId) => ({ frameId, result: frameId === 6 }))
    },
    ...overrides,
  }
}

test('blurs in the web frames of an agent tab, in the isolated world, skipping extension frames', async () => {
  const d = deps()
  assert.deepEqual(await blurFocusedField(7, d), { blurred: 1, frames: 2 })
  assert.equal(d.calls.length, 1)
  assert.deepEqual(d.calls[0].target, { tabId: 7, frameIds: [0, 6] })
  assert.equal(d.calls[0].world, 'ISOLATED')
  assert.equal(d.calls[0].func, blurEditableField)
})

test("never scripts the user's own tab", async () => {
  const d = deps()
  await assert.rejects(blurFocusedField(3, d), /only a tab chrome-use created/)
  assert.equal(d.calls.length, 0)
})

test('an unknown tab is refused before anything runs', async () => {
  const d = deps()
  await assert.rejects(blurFocusedField(null, d), /no tab for this session/)
  assert.equal(d.calls.length, 0)
})

test('missing site access names the option that grants it', async () => {
  const d = deps({
    executeScript: async () => { throw new Error('Cannot access contents of the page. Extension manifest must request permission to access the respective host.') },
  })
  await assert.rejects(blurFocusedField(7, d), /no site access .*Allow element picking on all sites/)
})

test('the frame script blurs only an editable field', () => {
  const run = (activeElement) => {
    const saved = globalThis.document
    globalThis.document = { activeElement, body: { tag: 'body' } }
    try { return blurEditableField() } finally { globalThis.document = saved }
  }
  let blurred = 0
  const field = (selector, extra = {}) => ({ matches: (s) => s.split(', ').includes(selector), isContentEditable: false, blur: () => blurred++, ...extra })
  assert.equal(run(field('input')), true)
  assert.equal(run(field('div', { isContentEditable: true })), true)
  assert.equal(run(field('iframe')), false)
  assert.equal(run(null), false)
  assert.equal(blurred, 2)
})
