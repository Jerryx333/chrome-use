import assert from 'node:assert/strict'
import test from 'node:test'
import { mountTeach, parseProgram } from './teach-panel.js'

function node(tag = 'div') {
  const n = { tagName: tag, children: [], attributes: {}, listeners: {}, classList: { set: new Set(), toggle(c, on) { on ? this.set.add(c) : this.set.delete(c) } },
    hidden: true, dataset: {}, textContent: '', value: '',
    setAttribute(k, v) { this.attributes[k] = v }, addEventListener(type, fn) { this.listeners[type] = fn },
    replaceChildren(...c) { this.children = c }, append(...c) { this.children.push(...c) } }
  return n
}

test('the reviewed script must stay a JSON op-list', () => {
  assert.equal(parseProgram('[{"do":"click"}]').length, 1)
  assert.throws(() => parseProgram('{"do":1}'), /JSON array/)
  assert.throws(() => parseProgram('[1]'), /JSON array/)
  assert.throws(() => parseProgram('nope'), /not JSON/)
})

test('record, stop, review and save go through the background and the host', async () => {
  const button = node('button'), view = node('section')
  const doc = { getElementById: (id) => ({ teachRecord: button, teachReview: view })[id], createElement: (tag) => node(tag) }
  const sent = []
  const runtime = { sendMessage: async (m) => {
    sent.push(m)
    if (m.type === 'ab-teach-status') return { success: true, recording: false }
    if (m.type === 'ab-teach-start') return { success: true, started: true }
    if (m.type === 'ab-teach-stop') return { success: true, recording: { startUrl: 'https://a.test/', events: [{ t: 'click' }] } }
    if (m.operation === 'teachCompile') return { success: true, result: { name: 'flow', steps: ['Open https://a.test/', 'Click button'], params: [], program: [{ do: 'navigate' }] } }
    if (m.operation === 'teachSave') return { success: true, result: { path: '/h/.chrome-use/teach/flow.json', run: 'chrome-use script /h/.chrome-use/teach/flow.json' } }
  } }
  const tabs = { get: async () => ({ url: 'https://a.test/' }) }
  const teach = mountTeach({ doc, runtime, tabs, panelTab: 4, t: (k, ...a) => [k, ...a].join(' ') })
  await teach.refresh()
  assert.equal(button.hidden, false)
  await teach.toggle()
  assert.equal(teach.state(), 'recording')
  assert.equal(button.attributes['aria-pressed'], 'true')
  await teach.toggle()
  assert.equal(teach.state(), 'idle')
  assert.equal(view.hidden, false)
  assert.equal(sent.find((m) => m.operation === 'teachCompile').params.recording.events.length, 1)
  const actions = view.children.find((c) => c.className === 'teach-actions')
  await actions.children[0].listeners.click()
  assert.equal(sent.at(-1).operation, 'teachSave')
  assert.deepEqual(sent.at(-1).params.program, [{ do: 'navigate' }])
})
