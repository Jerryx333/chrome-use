import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const create = vm.runInNewContext(`(${readFileSync(new URL('../cli/src/native/monaco_model.js', import.meta.url), 'utf8')})`);

function fixture() {
  const document = { activeElement: null };
  const root = { isConnected: true, contains: node => node === input };
  const input = { isConnected: true, ownerDocument: document, closest: () => root,
    focus() { document.activeElement = this; } };
  const model = { getValue: () => 'authoritative value', isDisposed: () => false };
  const editor = { getDomNode: () => root, getModel: () => model };
  const other = { getDomNode: () => ({}), getModel: () => { throw Error('wrong model read'); } };
  const api = { getEditors: () => [other, editor] };
  return { document, root, input, model, editor, api };
}

test('matches the exact editor root without reading another model or exposing state', () => {
  const f = fixture(), before = Object.keys(f.input);
  const handle = create.call(f.api, f.input);
  assert.equal(handle.read(), 'authoritative value');
  assert.equal(handle.focus(), true);
  assert.equal(f.document.activeElement, f.input);
  assert.deepEqual(Object.keys(f.input), before);
});

test('unmatched API does not guess a model; ambiguous identity refuses', () => {
  const f = fixture();
  assert.equal(create.call({ getEditors: () => [] }, f.input), null);
  assert.throws(() => create.call({ getEditors: () => [f.editor, f.editor] }, f.input), /ambiguous/);
});

for (const change of ['detached', 'model-swapped', 'disposed', 'root-swapped']) {
  test(`${change} invalidates a previously discovered model`, () => {
    const f = fixture(), handle = create.call(f.api, f.input);
    if (change === 'detached') f.input.isConnected = false;
    if (change === 'model-swapped') f.editor.getModel = () => ({ getValue: () => 'other value' });
    if (change === 'disposed') f.model.isDisposed = () => true;
    if (change === 'root-swapped') f.editor.getDomNode = () => ({});
    assert.throws(() => handle.read(), /changed/);
    assert.throws(() => handle.focus(), /changed/);
  });
}

for (const change of ['focus-rejected', 'focus-lost', 'read-only', 'disabled']) {
  test(`${change} refuses further input`, () => {
    const f = fixture(), handle = create.call(f.api, f.input);
    if (change === 'focus-rejected') f.input.focus = () => {};
    else handle.focus();
    if (change === 'focus-lost') f.document.activeElement = {};
    if (change === 'read-only') f.input.readOnly = true;
    if (change === 'disabled') f.input.disabled = true;
    assert.throws(() => change === 'focus-rejected' ? handle.focus() : handle.checkFocus(), /unavailable|lost focus/);
  });
}
