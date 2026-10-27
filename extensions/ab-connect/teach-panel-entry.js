// The side panel's Record control (teach-panel.js), mounted beside the
// conversation controller without touching it.
import { i18nReady, t } from './i18n.js';
import { mountTeach } from './teach-panel.js';
import { panelTabFrom } from './panel-tabs.js';

if (typeof document !== 'undefined' && globalThis.chrome?.runtime) void i18nReady.then(() => {
  const panelTab = panelTabFrom(globalThis.location?.search);
  mountTeach({ doc: document, runtime: globalThis.chrome.runtime, tabs: globalThis.chrome.tabs, permissions: globalThis.chrome.permissions, panelTab, t });
});
