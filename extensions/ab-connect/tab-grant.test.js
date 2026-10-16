import assert from 'node:assert/strict';
import test from 'node:test';
import { createTabGrants, grantRefusal, originOf, GRANTS_KEY, TAKEN_BACK } from './tab-grant.js';

function memory(initial = {}) {
  const data = { ...initial };
  return { data, get: async key => (key in data ? { [key]: data[key] } : {}), set: async value => Object.assign(data, JSON.parse(JSON.stringify(value))) };
}
const tab = (id, url = `https://shop.example/${id}`) => ({ id, url, title: `Tab ${id}` });

test('only web pages outside the Web Store can be lent', () => {
  assert.equal(grantRefusal('https://example.com/a'), null);
  assert.equal(grantRefusal('http://127.0.0.1:8920/'), null);
  for (const url of ['chrome://settings', 'chrome-extension://x/sidepanel.html', 'about:blank', 'file:///etc/hosts', 'devtools://x', '', 'nonsense']) assert.equal(grantRefusal(url), 'notWeb', url);
  assert.equal(grantRefusal('https://chromewebstore.google.com/detail/x'), 'webStore');
  assert.equal(grantRefusal('https://chrome.google.com/webstore/detail/x'), 'webStore');
  assert.equal(grantRefusal('https://chrome.google.com/search'), null);
  assert.equal(originOf('https://a.example:8443/x?y'), 'https://a.example:8443');
});

test('a grant is per tab and per conversation, and survives a worker restart', async () => {
  const storage = memory();
  const grants = createTabGrants({ storage, now: () => 5 });
  const { grant, replaced } = await grants.grant(tab(7), { conversationId: 'c1', conversationTitle: 'Orders' });
  assert.deepEqual(grant, { tabId: 7, conversationId: 'c1', conversationTitle: 'Orders', url: 'https://shop.example/7', title: 'Tab 7', at: 5 });
  assert.equal(replaced, null);
  await assert.rejects(grants.grant(tab(7), { conversationId: 'c2' }), error => error.reason === 'otherConversation' && error.conversationId === 'c1');
  await assert.rejects(grants.grant(tab(8, 'chrome://settings'), { conversationId: 'c2' }), error => error.reason === 'notWeb');
  await assert.rejects(grants.grant(tab(8, 'https://chromewebstore.google.com/'), { conversationId: 'c2' }), error => error.reason === 'webStore');
  // Granting the same conversation another tab takes the first one back.
  const second = await grants.grant(tab(9), { conversationId: 'c1' });
  assert.equal(second.replaced.tabId, 7);
  assert.equal(grants.isRefused(7), true);
  assert.equal(grants.forConversation('c1').tabId, 9);
  const restarted = createTabGrants({ storage });
  await restarted.load();
  assert.equal(restarted.get(9).conversationId, 'c1');
  assert.equal(restarted.isRefused(7), true);
  assert.deepEqual(Object.keys(storage.data), [GRANTS_KEY]);
});

test('taking a tab back refuses it until it is granted again or closed', async () => {
  const grants = createTabGrants({ storage: memory() });
  await grants.grant(tab(3), { conversationId: 'c1' });
  assert.equal(grants.isRefused(3), false);
  assert.equal((await grants.revoke(3)).conversationId, 'c1');
  assert.equal(await grants.revoke(3), null);
  assert.equal(grants.isRefused(3), true);
  assert.equal(grants.get(3), null);
  await grants.grant(tab(3), { conversationId: 'c2' });
  assert.equal(grants.isRefused(3), false);
  await grants.revoke(3);
  assert.equal(await grants.removed(3), null);
  assert.equal(grants.isRefused(3), false);
  assert.match(TAKEN_BACK, /took this tab back/);
});

test('a grant follows a replaced tab and keeps its page up to date', async () => {
  const grants = createTabGrants({ storage: memory() });
  await grants.grant(tab(4), { conversationId: 'c1' });
  const moved = await grants.replaced(4, 40);
  assert.equal(moved.tabId, 40);
  assert.equal(grants.get(4), null);
  const changed = await grants.navigated(40, { url: 'https://other.example/', title: 'Other' });
  assert.equal(changed.url, 'https://other.example/');
  assert.equal(await grants.navigated(40, { url: 'https://other.example/', title: 'Other' }), null);
  assert.equal(await grants.navigated(41, { url: 'https://x.example/' }), null);
  await grants.revoke(40);
  await grants.replaced(40, 41);
  assert.equal(grants.isRefused(41), true);
  assert.equal(grants.isRefused(40), false);
  const closed = await grants.removed(41);
  assert.equal(closed, null);
});
