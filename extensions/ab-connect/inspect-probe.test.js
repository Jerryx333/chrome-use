import assert from 'node:assert/strict';
import test from 'node:test';
import { inspectProbe, validateProbeResult } from './inspect-probe.js';

function fakeElement(index, props = {}) {
  return { ...props, getAttribute: () => String(index), parentElement: null };
}

test('probe reads React fibers and cleans bundler prefixes', () => {
  const owner = { type: { name: 'OrderForm' }, return: null };
  const fiber = { type: 'button', return: { type: { name: 'SaveButton' }, _debugSource: { fileName: 'webpack-internal:///./src/pages/Order.tsx', lineNumber: 42, columnNumber: 7 }, return: owner } };
  const el = fakeElement(0, { '__reactFiber$abc': fiber });
  globalThis.document = { querySelectorAll: () => [el] };
  try {
    const [entry] = inspectProbe('data-x');
    assert.equal(entry.index, 0);
    assert.equal(entry.info.component, 'SaveButton');
    assert.deepEqual(entry.info.ancestors, ['OrderForm']);
    assert.deepEqual(entry.info.source, { kind: 'react', path: 'src/pages/Order.tsx', line: 42, column: 7 });
  } finally {
    delete globalThis.document;
  }
});

test('probe reads Vue 3 component files', () => {
  const instance = { type: { __name: 'OrderCard', __file: '/repo/src/components/OrderCard.vue' }, parent: { type: { name: 'App' }, parent: null } };
  const el = fakeElement(0, { __vueParentComponent: instance });
  globalThis.document = { querySelectorAll: () => [el] };
  try {
    const [entry] = inspectProbe('data-x');
    assert.equal(entry.info.framework, 'vue');
    assert.equal(entry.info.component, 'OrderCard');
    assert.equal(entry.info.source.path, '/repo/src/components/OrderCard.vue');
  } finally {
    delete globalThis.document;
  }
});

test('validation drops malformed or hostile probe output', () => {
  const out = validateProbeResult([
    { index: 0, info: { framework: 'react', component: 'Save', ancestors: ['A', '<script>', 'B', 'C', 'D'], source: { kind: 'react', path: 'src/a.tsx', line: 3, column: 1 } } },
    { index: 1, info: { framework: 'evil', component: 'x'.repeat(500), source: { kind: 'react', path: 'src/secret/token.ts', line: 1 } } },
    { index: 7, info: { component: 'OutOfRange' } },
    { index: '0', info: {} },
    'junk',
  ], 2);
  assert.equal(out[0].component, 'Save');
  assert.deepEqual(out[0].ancestors, ['A', 'B', 'C']);
  assert.equal(out[0].source.path, 'src/a.tsx');
  assert.equal(out[1].framework, null);
  assert.equal(out[1].component, null);
  assert.equal(out[1].source, null);
  assert.deepEqual(validateProbeResult('nope', 1), [null]);
});
