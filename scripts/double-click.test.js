import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import vm from 'node:vm'

const source = readFileSync(new URL('../cli/src/native/double_click.rs', import.meta.url), 'utf8')
const payload = source.match(/const RECORD: &str = r#"([\s\S]*?)"#;/)?.[1]
assert.ok(payload)
function fixture() {
  const doc = new EventTarget()
  const timers = new Map()
  const el = { ownerDocument: {
    addEventListener(type, fn, capture) { doc.addEventListener(type, fn, { capture }) },
    removeEventListener(type, fn, capture) { doc.removeEventListener(type, fn, { capture }) },
  } }
  const recorder = vm.runInNewContext(`(${payload})`, {
    setTimeout(fn, delay) { timers.set(1, { fn, delay }); return 1 },
    clearTimeout(id) { timers.delete(id) },
  }).call(el)
  function event(type, detail, trusted = true, target = el) {
    const event = new Event(type)
    // Fixture-only override tests filtering, not Chrome's trusted-input path.
    Object.defineProperty(event, 'isTrusted', { value: trusted })
    event.detail = detail
    event.composedPath = () => [target]
    doc.dispatchEvent(event)
  }
  return { recorder, event, timers }
}
test('double-click recorder ignores untrusted events and other targets', () => {
  const { recorder, event } = fixture()
  event('dblclick', 2, false)
  event('dblclick', 2, true, {})
  assert.equal(recorder.events.length, 0)
  event('click', 1)
  event('click', 2)
  event('dblclick', 2)
  assert.deepEqual(JSON.parse(JSON.stringify(recorder.events)), [
    { type: 'click', detail: 1 }, { type: 'click', detail: 2 }, { type: 'dblclick', detail: 2 },
  ])
  recorder.stop()
})
test('double-click recorder stops normally and expires if cleanup is lost', () => {
  for (const expire of [false, true]) {
    const { recorder, event, timers } = fixture()
    if (expire) {
      assert.equal(timers.get(1).delay, 10000)
      timers.get(1).fn()
    } else recorder.stop()
    event('click', 1)
    event('dblclick', 2)
    assert.equal(recorder.events.length, 0)
    assert.equal(timers.size, 0)
  }
})
