import './test-i18n.js';
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { configureAgentPanel } from './sidepanel-entry.js';

test('toolbar entry is enabled only after Chrome confirms the behavior', async () => {
  const calls = [];
  const result = await configureAgentPanel({
    sidePanel: {
      async setPanelBehavior(value) { calls.push(value); },
      async setOptions(value) { calls.push(['options', value]); },
      async getPanelBehavior() { calls.push('readback'); return { openPanelOnActionClick: false }; },
    },
    action: { async setPopup(value) { calls.push(value); }, onClicked: { addListener() {} } },
  });
  // The button opens one tab's panel (background.js); no panel is global.
  assert.deepEqual(calls, [{ openPanelOnActionClick: false }, ['options', { enabled: false }], 'readback', { popup: '' }]);
  assert.equal(result.ready, true);
});

test('unsupported or unconfirmed entry preserves the connection popup', async () => {
  for (const sidePanel of [undefined, { async setPanelBehavior() {}, async setOptions() {} }, { async setPanelBehavior() {}, async setOptions() {}, async getPanelBehavior() { return { openPanelOnActionClick: true }; } }]) {
    const calls = [];
    const result = await configureAgentPanel({ sidePanel, action: { async setPopup(value) { calls.push(value); }, onClicked: { addListener() {} } } });
    assert.equal(result.ready, false);
    assert.deepEqual(calls, [{ popup: 'popup.html' }]);
    assert.ok(result.error);
  }
});

test('packaged entry requires no webpage injection or new host permissions', () => {
  const manifest = JSON.parse(readFileSync(new URL('./manifest.json', import.meta.url), 'utf8'));
  assert.equal(manifest.side_panel.default_path, 'sidepanel.html');
  assert.ok(manifest.permissions.includes('sidePanel'));
  assert.equal(manifest.action.default_popup, 'popup.html');
  assert.equal(manifest.content_scripts, undefined);
  assert.equal(manifest.host_permissions, undefined);
  assert.match(readFileSync(new URL('./sidepanel.html', import.meta.url), 'utf8'), /src="sidepanel.js"/);
});
