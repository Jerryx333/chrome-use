// "Let the agent use this tab" (0.5.62): the user lends one of their own tabs
// to one side-panel conversation. The grant is per tab and per conversation,
// lasts for the browser session (storage.session) and is revocable. Taking a
// tab back marks it refused: every relay attach or command on it fails with
// TAKEN_BACK until the user grants it again or closes it, so a session that
// still holds the tab cannot quietly re-attach it.
//
// Kept free of `chrome.*` globals so the rules are unit-testable; background.js
// wires the storage, the debugger detach and the indicators.

export const GRANTS_KEY = 'ab_tab_grants';

// Read by the agent (relay errors reach it verbatim), so it stays English.
export const TAKEN_BACK = 'The user took this tab back from the agent in the chrome-use side panel, so it can no longer be used. Continue in your own tab, or ask the user to let you use this tab again.';

/** Why a page cannot be lent to the agent: 'notWeb', 'webStore', or null. */
export function grantRefusal(url) {
  let parsed;
  try { parsed = new URL(String(url || '')); } catch { return 'notWeb'; }
  if (!['http:', 'https:'].includes(parsed.protocol)) return 'notWeb';
  const host = parsed.hostname.toLowerCase();
  // Chrome lets no extension drive the Web Store.
  if (host === 'chromewebstore.google.com' || (host === 'chrome.google.com' && parsed.pathname.startsWith('/webstore'))) return 'webStore';
  return null;
}

export function originOf(url) {
  try { return new URL(url).origin; } catch { return ''; }
}

/**
 * deps: { storage: { get(key) -> Promise<obj>, set(obj) -> Promise } }
 * A grant: { tabId, conversationId, conversationTitle, url, title, at }.
 */
export function createTabGrants({ storage, now = () => Date.now() }) {
  const grants = new Map();
  const refused = new Set();
  let loaded = null;
  const save = () => storage.set({ [GRANTS_KEY]: { grants: [...grants.values()], refused: [...refused] } }).catch(() => {});
  return {
    load() {
      loaded ??= Promise.resolve(storage.get(GRANTS_KEY)).then(stored => {
        const value = stored?.[GRANTS_KEY] || {};
        for (const grant of value.grants || []) if (Number.isInteger(grant?.tabId) && typeof grant.conversationId === 'string') grants.set(grant.tabId, grant);
        for (const id of value.refused || []) if (Number.isInteger(id) && !grants.has(id)) refused.add(id);
      }).catch(() => {});
      return loaded;
    },
    get(tabId) { return grants.get(tabId) || null; },
    list() { return [...grants.values()]; },
    forConversation(conversationId) { return [...grants.values()].find(grant => grant.conversationId === conversationId) || null; },
    isRefused(tabId) { return refused.has(tabId); },
    /** Lend `tab` to a conversation. Returns { grant, replaced } where
     *  `replaced` is the conversation's earlier tab, now taken back. */
    async grant(tab, { conversationId, conversationTitle = '' }) {
      await this.load();
      if (!Number.isInteger(tab?.id)) throw new Error('No such tab');
      if (typeof conversationId !== 'string' || !conversationId) throw new Error('A conversation is required');
      const reason = grantRefusal(tab.url);
      if (reason) throw Object.assign(new Error(reason === 'webStore' ? 'Chrome does not let extensions drive the Chrome Web Store' : 'Only a web page can be given to the agent'), { reason });
      const held = grants.get(tab.id);
      if (held && held.conversationId !== conversationId) throw Object.assign(new Error('Another conversation is using this tab; take it back there first'), { reason: 'otherConversation', conversationId: held.conversationId });
      const previous = this.forConversation(conversationId);
      let replaced = null;
      if (previous && previous.tabId !== tab.id) {
        grants.delete(previous.tabId);
        refused.add(previous.tabId);
        replaced = previous;
      }
      const grant = { tabId: tab.id, conversationId, conversationTitle: String(conversationTitle).slice(0, 200), url: String(tab.url).slice(0, 8192), title: String(tab.title || tab.url).slice(0, 1024), at: now() };
      grants.set(tab.id, grant);
      refused.delete(tab.id);
      await save();
      return { grant, replaced };
    },
    /** Take a tab back. Returns the grant it ended, or null. */
    async revoke(tabId) {
      await this.load();
      const grant = grants.get(tabId) || null;
      if (!grant) return null;
      grants.delete(tabId);
      refused.add(tabId);
      await save();
      return grant;
    },
    /** The tab closed: its grant and its refusal end with it. */
    async removed(tabId) {
      await this.load();
      const grant = grants.get(tabId) || null;
      const hadGrant = grants.delete(tabId), wasRefused = refused.delete(tabId);
      if (hadGrant || wasRefused) await save();
      return grant;
    },
    /** Chrome replaced the tab (a discard, a prerender swap): both follow it. */
    async replaced(fromId, toId) {
      await this.load();
      const grant = grants.get(fromId);
      const wasRefused = refused.delete(fromId);
      if (wasRefused) refused.add(toId);
      if (!grant) { if (wasRefused) await save(); return null; }
      grants.delete(fromId);
      const moved = { ...grant, tabId: toId };
      grants.set(toId, moved);
      await save();
      return moved;
    },
    /** The granted tab navigated: the grant stays, its page is updated. */
    async navigated(tabId, { url, title }) {
      await this.load();
      const grant = grants.get(tabId);
      if (!grant || (grant.url === url && (title === undefined || grant.title === title))) return null;
      if (typeof url === 'string' && url) grant.url = url.slice(0, 8192);
      if (typeof title === 'string' && title) grant.title = title.slice(0, 1024);
      await save();
      return grant;
    },
  };
}
