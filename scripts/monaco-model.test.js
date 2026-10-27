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
  const range = { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 20 };
  const model = { getValue: () => 'authoritative value', isDisposed: () => false, getFullModelRange: () => range };
  const editor = { getDomNode: () => root, getModel: () => model, getSelection: () => ({ ...range }) };
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

function amdFixture() {
  const f = fixture();
  const record = { strId: 'vs/editor/standalone/browser/standaloneEditor',
    _isComplete: true, error: null, exports: f.api };
  const modules = new Array(888);
  modules[880] = record;
  const manager = { _modules2: modules,
    _requireFunc() { throw Error('must not invoke require'); },
    _defineFunc() { throw Error('must not invoke define'); } };
  return { ...f, record, modules, manager };
}

test('completed minified AMD module uses the exact editor and identity guards', () => {
  const f = amdFixture(), before = Object.keys(f.manager);
  const handle = create.call(f.manager, f.input, true);
  assert.equal(handle.read(), 'authoritative value');
  assert.equal(handle.focus(), true);
  assert.deepEqual(Object.keys(f.manager), before);
  f.editor.getModel = () => ({});
  assert.throws(() => handle.read(), /changed/);
});

for (const fault of ['wrong-module', 'incomplete', 'failed', 'too-large',
  'table-getter', 'entry-getter', 'id-getter', 'completion-getter', 'error-getter',
  'exports-getter', 'api-getter']) {
  test(`AMD ${fault} refuses without invoking loaders or accessors`, () => {
    const f = amdFixture();
    const getter = (object, key) => Object.defineProperty(object, key, {
      get() { throw Error('discovery invoked an accessor'); }, configurable: true });
    if (fault === 'wrong-module') f.record.strId = 'unrelated/editor';
    if (fault === 'incomplete') f.record._isComplete = false;
    if (fault === 'failed') f.record.error = new Error('module failed');
    if (fault === 'too-large') f.modules.length = 4097;
    if (fault === 'table-getter') getter(f.manager, '_modules2');
    if (fault === 'entry-getter') getter(f.modules, '880');
    if (fault === 'id-getter') getter(f.record, 'strId');
    if (fault === 'completion-getter') getter(f.record, '_isComplete');
    if (fault === 'error-getter') getter(f.record, 'error');
    if (fault === 'exports-getter') getter(f.record, 'exports');
    if (fault === 'api-getter') getter(f.api, 'getEditors');
    assert.equal(create.call(f.manager, f.input, true), null);
  });
}

test('native fill requires the complete live selection before inserting', () => {
  const f = fixture(), handle = create.call(f.api, f.input);
  handle.focus();
  assert.equal(handle.checkSelection(), true);
  for (const selection of [null, {}, { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1 }]) {
    f.editor.getSelection = () => selection;
    assert.throws(() => handle.checkSelection(), /full model/);
  }
  f.editor.getSelection = undefined;
  assert.throws(() => handle.checkSelection(), /cannot verify/);
});
