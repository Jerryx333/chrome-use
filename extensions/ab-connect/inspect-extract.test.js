import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

// The extractor is a classic content script; load it the way Chrome does.
function load() {
  const sandbox = { URL };
  sandbox.globalThis = sandbox;
  vm.runInNewContext(readFileSync(new URL('./inspect-extract.js', import.meta.url), 'utf8'), sandbox);
  return sandbox.__chromeUseInspectExtract;
}
const X = load();

test('parses code-inspector marks, including Windows drive letters and missing tag', () => {
  assert.deepEqual({ ...X.parseInspPath('src/pages/Order.tsx:42:7:button') }, { kind: 'insp', path: 'src/pages/Order.tsx', line: 42, column: 7, tag: 'button' });
  assert.deepEqual({ ...X.parseInspPath('C:/repo/src/App.vue:3:1:div') }, { kind: 'insp', path: 'C:/repo/src/App.vue', line: 3, column: 1, tag: 'div' });
  assert.equal(X.parseInspPath('src/App.tsx:12:5').line, 12);
  assert.equal(X.parseInspPath('nonsense'), null);
  assert.equal(X.parseInspPath('src/a.tsx:x:y:div'), null);
  assert.equal(X.parseInspPath('src/secret/token.ts:1:1:div'), null, 'secret-looking paths are dropped');
  assert.equal(X.parseInspPath('a'.repeat(2000)), null);
});

test('secrets and urls are scrubbed', () => {
  assert.equal(X.containsSecret('Bearer abc'), true);
  assert.equal(X.containsSecret('csrf-token'), true);
  assert.equal(X.containsSecret('order-button'), false);
  assert.equal(X.sanitizeUrl('https://a.test/p?token=1#x'), 'https://a.test/p');
  assert.equal(X.sanitizeUrl('/rel?q=1', 'https://b.test/base/'), 'https://b.test/rel');
  assert.equal(X.sanitizeUrl('javascript:alert(1)'), 'javascript:');
});

test('inline text collapses whitespace and clamps', () => {
  assert.equal(X.inlineText('  a \n\t b  ', 50), 'a b');
  assert.equal(X.inlineText('abcdef', 3), 'abc…');
  assert.equal(X.clamp('abcdef', 4), 'abcd…');
});

test('hashed class names are not used in selectors', () => {
  assert.equal(X.looksHashy('sc-1a2b3c4d'), true);
  assert.equal(X.looksHashy('_3xYz9'), true);
  assert.equal(X.looksHashy('primary-button'), false);
});
