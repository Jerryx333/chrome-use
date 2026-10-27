// The side panel's pending site approvals (#781), kept apart from the
// conversation controller in sidepanel.js.
import { i18nReady, t } from './i18n.js';
import { mountSiteApprovals } from './site-approvals-view.js';

if (typeof document !== 'undefined' && globalThis.chrome?.storage?.session) void i18nReady.then(() => {
  const container = document.getElementById('siteApprovals');
  if (container) mountSiteApprovals({ doc: document, container, storage: globalThis.chrome.storage, runtime: globalThis.chrome.runtime, t });
});
