import assert from 'node:assert/strict';
import test from 'node:test';
import { visibilityVerdict, editorUrl, deliveryMessage, LOCKED_STATES } from './inspect-frame-logic.js';

test('send needs a fresh, fully visible, foreground panel', () => {
  assert.equal(visibilityVerdict({ isVisible: true, ratio: 1, documentVisibility: 'visible' }).ok, true);
  assert.equal(visibilityVerdict({ isVisible: false, ratio: 1, documentVisibility: 'visible' }).ok, false);
  assert.equal(visibilityVerdict({ isVisible: true, ratio: 0.7, documentVisibility: 'visible' }).ok, false);
  assert.equal(visibilityVerdict({ isVisible: true, ratio: 1, documentVisibility: 'hidden' }).ok, false);
  assert.match(visibilityVerdict({ isVisible: undefined }).message, /Use Copy/);
  assert.equal(visibilityVerdict({ timeout: true }).ok, false);
  assert.equal(visibilityVerdict(null).ok, false);
});

test('editor links stay inside the bound root', () => {
  const source = { path: 'src/pages/Order View.tsx', line: 42, column: 7 };
  assert.equal(editorUrl('vscode', '/Users/me/app', source), 'vscode://file/Users/me/app/src/pages/Order%20View.tsx:42:7');
  assert.equal(editorUrl('cursor', '/Users/me/app/', source), 'cursor://file/Users/me/app/src/pages/Order%20View.tsx:42:7');
  assert.equal(editorUrl('vscode', 'C:\\work\\app', { path: 'src/a.ts', line: 1 }), 'vscode://file/C:/work/app/src/a.ts:1:1');
  assert.match(editorUrl('webstorm', '/r', source), /^webstorm:\/\/open\?file=%2Fr%2Fsrc/);
  assert.equal(editorUrl('vscode', '/r', { path: '../etc/passwd', line: 1 }), null);
  assert.equal(editorUrl('vscode', '/r', { path: '/abs/file.ts', line: 1 }), null);
  assert.equal(editorUrl('vscode', '/Users/me/app', { path: '/Users/me/app/src/a.ts', line: 2, column: 3 }), 'vscode://file/Users/me/app/src/a.ts:2:3');
  assert.equal(editorUrl('vscode', '/Users/me/app', { path: '/Users/me/app2/src/a.ts', line: 2 }), null);
  assert.equal(editorUrl('none', '/r', source), null);
  assert.equal(editorUrl('vscode', null, source), null);
});

test('delivery messages keep stored and wake apart and lock what may have landed', () => {
  assert.equal(deliveryMessage({ state: 'sent', wake: 'accepted' }).locked, true);
  assert.match(deliveryMessage({ state: 'stored', wake: 'parked' }).text, /wake: parked/);
  assert.equal(deliveryMessage({ state: 'noSession' }).locked, false);
  assert.equal(deliveryMessage({ state: 'weird' }).locked, true);
  for (const state of ['sent', 'stored', 'unknown', 'unconfirmed']) assert.ok(LOCKED_STATES.includes(state));
});
