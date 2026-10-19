// Keep the user's tab in front when an agent click opens a tab.
//
// Chrome opens a page's target=_blank link or pop-up as the foreground tab of
// the opener's window, and nothing an extension or CDP sets can stop it. With
// agent tabs in the user's window, an agent click therefore switched the page
// the user was reading to the agent's pop-up. This decides which tab to bring
// back; the worker does the chrome.tabs calls.
//
// The pop-up's source comes from webNavigation.onCreatedNavigationTarget
// (`sourceTabId`), not from the tab's `openerTabId`: for a background tab
// Chrome reports the window's front tab — the user's — as the opener.
//
// Kept free of `chrome.*` so the rules are unit-testable.

/** How long after the pop-up appears its activation still counts as Chrome's. */
export const RESTORE_WINDOW_MS = 2000

export class FrontTabGuard {
  constructor({ now = Date.now } = {}) {
    this.now = now
    this.front = new Map()     // windowId -> tab in front
    this.previous = new Map()  // windowId -> tab in front before that
    this.pending = new Map()   // pop-up tabId -> { windowId, restoreTo, until }
  }

  /** chrome.tabs.onActivated. Returns the tab to bring back, or null. */
  activated(windowId, tabId) {
    const was = this.front.get(windowId)
    if (was !== undefined && was !== tabId) this.previous.set(windowId, was)
    this.front.set(windowId, tabId)
    return this.#restoreFor(tabId)
  }

  /** A tab closed: it can no longer be brought back or be a pop-up. */
  removed(tabId) {
    this.pending.delete(tabId)
    for (const map of [this.front, this.previous])
      for (const [windowId, id] of map) if (id === tabId) map.delete(windowId)
  }

  /**
   * webNavigation.onCreatedNavigationTarget for a pop-up `tabId` in `windowId`
   * whose source is an agent tab. `active` is whether Chrome already put it in
   * front. Returns the tab to bring back now, or null (then `activated` may
   * return it when the pop-up comes to the front a moment later).
   */
  agentOpened(tabId, windowId, active) {
    const front = this.front.get(windowId)
    const restoreTo = front === tabId ? this.previous.get(windowId) : front
    if (restoreTo === undefined || restoreTo === tabId) return null
    this.pending.set(tabId, { windowId, restoreTo, until: this.now() + RESTORE_WINDOW_MS })
    return active || front === tabId ? this.#restoreFor(tabId) : null
  }

  #restoreFor(tabId) {
    const p = this.pending.get(tabId)
    if (!p) return null
    this.pending.delete(tabId)
    if (this.now() > p.until) return null
    // Bringing it back is itself an activation; it must not be undone.
    this.previous.set(p.windowId, tabId)
    this.front.set(p.windowId, p.restoreTo)
    return p.restoreTo
  }
}
