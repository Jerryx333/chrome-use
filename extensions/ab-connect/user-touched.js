// Agent tabs the user has made theirs by switching to them.
//
// Agent tabs share the user's window, each session's in its own ✦ group.
// When a session ends without saying which tabs to keep, the CLI's cleanup
// closes its tabs, except one the user is using. "In front right now" misses
// a tab the user read a minute ago in another window, so this remembers every
// agent tab the user switched to and stayed on.
//
// Not every activation is the user's: Chrome fronts an agent's pop-up for a
// moment before the front-tab guard brings the user's tab back, and an agent
// may raise its own tab on request (`--activate`). The first is filtered by
// requiring the tab to still be in front after DWELL_MS; the second is
// announced through `agentActivating` right before the extension does it.
//
// Kept free of `chrome.*` so the rules are unit-testable.

/** How long a tab must stay in front for the switch to count as the user's. */
export const DWELL_MS = 1500

export class UserTouchedTabs {
  constructor({ isAgentTab, now = Date.now }) {
    this.isAgentTab = isAgentTab
    this.now = now
    this.touched = new Set()
    this.agentActivations = new Map() // tabId -> until
  }

  /** The extension is about to activate `tabId` for an agent. */
  agentActivating(tabId) {
    this.agentActivations.set(tabId, this.now() + DWELL_MS)
  }

  /**
   * chrome.tabs.onActivated. Returns true when the caller should check after
   * DWELL_MS whether the tab is still in front, and then call `confirm`.
   */
  activated(tabId) {
    const until = this.agentActivations.get(tabId)
    if (until !== undefined) {
      this.agentActivations.delete(tabId)
      if (this.now() <= until) return false
    }
    return this.isAgentTab(tabId) && !this.touched.has(tabId)
  }

  /** Returns true when this made the tab the user's. */
  confirm(tabId, stillActive) {
    if (!stillActive || !this.isAgentTab(tabId) || this.touched.has(tabId)) return false
    this.touched.add(tabId)
    return true
  }

  removed(tabId) {
    this.agentActivations.delete(tabId)
    return this.touched.delete(tabId)
  }

  restore(ids) {
    for (const id of ids || []) if (Number.isInteger(id)) this.touched.add(id)
  }

  list() {
    return [...this.touched]
  }
}
