import assert from 'node:assert/strict';
import test from 'node:test';
import { InspectRegistry, inspectOrigin, isLocalOrigin, INSPECT_PORT_NAME } from './inspect-registry.js';

const RID = 'ext-id';
let counter = 0;
const ids = () => `id-${++counter}`;
const picker = (over = {}) => ({ id: RID, tab: { id: 7 }, frameId: 0, documentId: 'doc-A', ...over });
function framePort(over = {}) {
  const sent = [];
  return {
    name: INSPECT_PORT_NAME, sent, disconnected: false,
    sender: { id: RID, tab: { id: 7 }, frameId: 5, documentId: 'frame-doc', url: 'chrome-extension://ext-id/inspect-frame.html', ...over },
    postMessage(message) { sent.push(message); },
    disconnect() { this.disconnected = true; },
  };
}
function setup(now = { t: 0 }) {
  const registry = new InspectRegistry({ runtimeId: RID, now: () => now.t, randomId: ids });
  const instance = registry.create({ tabId: 7, windowId: 1, documentId: 'doc-A', url: 'http://localhost:5173/orders?x=1#y' });
  return { registry, instance, now };
}

test('origins: only http(s) pages, local detection', () => {
  assert.equal(inspectOrigin('https://test.example.com/a?b'), 'https://test.example.com');
  assert.equal(inspectOrigin('chrome://extensions'), null);
  assert.equal(inspectOrigin('file:///tmp/a.html'), null);
  assert.equal(isLocalOrigin('http://localhost:5173'), true);
  assert.equal(isLocalOrigin('http://127.0.0.1:3000'), true);
  assert.equal(isLocalOrigin('https://test.example.com'), false);
});

test('secret is handed once, only to frame 0 of the instance document', () => {
  const { registry, instance } = setup();
  assert.equal(registry.handSecret(picker({ frameId: 3 })), null);
  assert.equal(registry.handSecret(picker({ documentId: 'doc-B' })), null);
  assert.equal(registry.handSecret(picker({ id: 'other-ext' })), null);
  const handed = registry.handSecret(picker());
  assert.deepEqual(handed, { instanceId: instance.id, secret: instance.secret });
  assert.equal(registry.handSecret(picker()), null);
});

test('frame port needs the secret; wrong secrets do not consume the instance', () => {
  const { registry, instance } = setup();
  const early = framePort();
  assert.equal(registry.authenticatePort(early, instance.secret), null, 'secret not handed yet');
  registry.handSecret(picker());
  assert.equal(registry.authenticatePort(framePort({ frameId: 9 }), 'guess'), null);
  assert.equal(registry.authenticatePort(framePort({ frameId: 0 }), instance.secret), null, 'top frame cannot be the input frame');
  assert.equal(registry.authenticatePort(framePort({ url: 'chrome-extension://ext-id/sidepanel.html' }), instance.secret), null);
  assert.equal(registry.authenticatePort(framePort({ tab: { id: 8 } }), instance.secret), null);
  assert.equal(registry.authenticatePort({ ...framePort(), name: 'other' }, instance.secret), null);
  const real = framePort();
  assert.equal(registry.authenticatePort(real, instance.secret), instance);
  assert.equal(registry.forPort(real), instance);
  const copy = framePort({ frameId: 11, documentId: 'copy-doc' });
  assert.equal(registry.authenticatePort(copy, instance.secret), null, 'secret is single use');
  assert.equal(registry.forPort(copy), null);
  assert.equal(instance.secret, null, 'secret cleared after use');
});

test('a flood of wrong secrets is throttled but cannot lock out the real frame', () => {
  const now = { t: 0 };
  const { registry, instance } = setup(now);
  registry.handSecret(picker());
  const secret = instance.secret;
  for (let i = 0; i < 20; i++) assert.equal(registry.authenticatePort(framePort({ frameId: 30 + i }), 'bad'), null);
  assert.equal(registry.authenticatePort(framePort(), secret), 'busy');
  now.t += 1001;
  assert.equal(registry.authenticatePort(framePort(), secret), instance);
});

test('forPort rejects a port whose frame document changed', () => {
  const { registry, instance } = setup();
  registry.handSecret(picker());
  const port = framePort();
  registry.authenticatePort(port, instance.secret);
  port.sender = { ...port.sender, documentId: 'reloaded' };
  assert.equal(registry.forPort(port), null);
});

test('navigation, replacement and idle revoke and disconnect the frame', () => {
  const { registry, instance, now } = setup();
  registry.handSecret(picker());
  const port = framePort();
  registry.authenticatePort(port, instance.secret);
  registry.revokeDocument(7, 'doc-A');
  assert.equal(registry.get(7), instance, 'same document keeps the instance');
  registry.revokeDocument(7, 'doc-B');
  assert.equal(registry.get(7), null);
  assert.equal(port.disconnected, true);
  assert.deepEqual(port.sent.at(-1), { type: 'revoked', reason: 'navigated' });

  const second = registry.create({ tabId: 7, windowId: 1, documentId: 'doc-B', url: 'http://localhost:5173/' });
  const third = registry.create({ tabId: 7, windowId: 1, documentId: 'doc-B', url: 'http://localhost:5173/' });
  assert.equal(second.revoked, true);
  assert.equal(second.revokedReason, 'replaced');
  now.t += 11 * 60 * 1000;
  assert.equal(registry.get(7), null);
  assert.equal(third.revokedReason, 'idle');
});

test('generation moves on tab and window events', () => {
  const { registry, instance } = setup();
  registry.bump(7);
  registry.bumpWindow(1);
  registry.bumpWindow(2);
  assert.equal(instance.generation, 2);
});

test('non-web pages cannot start an instance', () => {
  const registry = new InspectRegistry({ runtimeId: RID, randomId: ids });
  assert.equal(registry.create({ tabId: 1, documentId: 'd', url: 'chrome://newtab' }), null);
  assert.equal(registry.create({ tabId: 1, documentId: '', url: 'https://a.test' }), null);
});
