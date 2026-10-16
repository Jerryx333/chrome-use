import assert from 'node:assert/strict';
import test from 'node:test';
import { bindingsFrom, openTabPanel, panelPath, panelTabFrom, panelTabKey, tabForConversation } from './panel-tabs.js';

test('each tab keeps its own conversation key and panel path', () => {
  assert.equal(panelTabKey(7), 'ab_panel_tab_7');
  assert.equal(panelPath(7), 'sidepanel.html?tab=7');
  assert.equal(panelTabFrom('?tab=7'), 7);
  for (const search of ['', '?tab=', '?tab=x', '?tab=-1', '?mode=tab']) assert.equal(panelTabFrom(search), null, search);
  const bindings = bindingsFrom({ ab_panel_tab_7: 'a', ab_panel_tab_9: '', ab_panel_tab_x: 'b', ab_tab_grants: {}, other: 'c' });
  assert.deepEqual([...bindings], [[7, 'a']]);
  assert.equal(tabForConversation(bindings, 'a'), 7);
  assert.equal(tabForConversation(bindings, 'b'), null);
});

test('the panel of one tab is enabled and opened in the same task', async () => {
  const calls = [];
  const sidePanel = { setOptions: async value => { calls.push(['options', value]); }, open: async value => { calls.push(['open', value]); } };
  const opening = openTabPanel(5, { sidePanel });
  assert.deepEqual(calls, [['options', { tabId: 5, path: 'sidepanel.html?tab=5', enabled: true }], ['open', { tabId: 5 }]]);
  await opening;
});
