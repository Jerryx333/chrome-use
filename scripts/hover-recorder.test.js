import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import vm from 'node:vm'

const source = readFileSync(new URL('../cli/src/native/interaction.rs', import.meta.url), 'utf8')
const payload = source.match(/const HOVER_POINT_PROBE_JS: &str = r#"([\s\S]*?)"#;/)?.[1]
assert.ok(payload, 'coordinate hover payload was not found')

function fixture() {
  const target = new EventTarget()
  const timers = new Map()
  let nextTimer = 0
  const recorder = vm.runInNewContext(payload, {
    // Node's EventTarget differs on boolean capture removal; normalize the
    // browser API shorthand to its equivalent options object in this fixture.
    addEventListener(type, fn, capture) { target.addEventListener(type, fn, { capture }) },
    removeEventListener(type, fn, capture) { target.removeEventListener(type, fn, { capture }) },
    setTimeout(fn, delay) { const id = ++nextTimer; timers.set(id, { fn, delay }); return id },
    clearTimeout(id) { timers.delete(id) },
  })
  function move(type = 'pointermove', trusted = true) {
    const event = new Event(type)
    // A fixture-only trusted event lets us observe whether the real recorder
    // listener still runs. This does not emulate Chrome's input dispatch.
    Object.defineProperty(event, 'isTrusted', { value: trusted })
    event.composedPath = () => [{ tagName: 'BUTTON', id: 'target', className: '', innerText: 'Target' }]
    target.dispatchEvent(event)
  }
  return { recorder, timers, move }
}

test('coordinate hover records trusted moves but ignores synthetic moves', () => {
  const { recorder, move } = fixture()
  move('pointermove', false)
  assert.equal(recorder.seen.hit, null)
  move('mousemove')
  assert.equal(recorder.seen.hit, '<button id="target"> "Target"')
  recorder.stop()
})

test('coordinate hover stop removes both listeners and cancels expiry', () => {
  const { recorder, timers, move } = fixture()
  recorder.stop()
  recorder.stop()
  move('pointermove')
  move('mousemove')
  assert.equal(recorder.seen.hit, null)
  assert.equal(timers.size, 0)
})

test('coordinate hover expires when the caller never sends stop', () => {
  const { recorder, timers, move } = fixture()
  assert.equal(timers.size, 1)
  const [{ fn, delay }] = timers.values()
  assert.equal(delay, 10000)
  fn()
  move('pointermove')
  move('mousemove')
  assert.equal(recorder.seen.hit, null)
  assert.equal(timers.size, 0)
})
