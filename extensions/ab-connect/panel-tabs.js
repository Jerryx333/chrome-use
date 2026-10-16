// One side panel per tab (0.5.63). The toolbar button opens a panel for the
// tab it was clicked on (chrome.sidePanel.setOptions with that tab's id in
// the path); switching tabs shows that tab's own panel, or none. Which
// conversation each tab's panel shows is kept in storage.session under one
// key per tab, so two panels never write the same key. A tab that closes
// takes its key with it; the conversation stays in the history.

export const PANEL_TAB_PREFIX = 'ab_panel_tab_';

export function panelTabKey(tabId) { return `${PANEL_TAB_PREFIX}${tabId}`; }

export function panelPath(tabId) { return `sidepanel.html?tab=${tabId}`; }

/** tabId -> conversationId from a storage.session snapshot. */
export function bindingsFrom(all) {
  const bindings = new Map();
  for (const [key, value] of Object.entries(all || {})) {
    if (!key.startsWith(PANEL_TAB_PREFIX) || typeof value !== 'string' || !value) continue;
    const tabId = Number(key.slice(PANEL_TAB_PREFIX.length));
    if (Number.isInteger(tabId)) bindings.set(tabId, value);
  }
  return bindings;
}

/** The tab whose panel shows `conversationId`, or null. */
export function tabForConversation(bindings, conversationId) {
  for (const [tabId, id] of bindings) if (id === conversationId) return tabId;
  return null;
}

/** The tab id a panel page was opened for (`?tab=<id>`), or null. */
export function panelTabFrom(search) {
  const value = new URLSearchParams(search || '').get('tab');
  return value !== null && /^\d+$/.test(value) ? Number(value) : null;
}

/**
 * Open the panel of one tab. Both calls go out in the same task, so a click
 * (toolbar button, notification) still counts as the gesture open() needs.
 * deps: { sidePanel }
 */
export function openTabPanel(tabId, { sidePanel }) {
  const enabling = sidePanel.setOptions({ tabId, path: panelPath(tabId), enabled: true });
  const opening = sidePanel.open({ tabId });
  return Promise.all([enabling, opening]);
}
