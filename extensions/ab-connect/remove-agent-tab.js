// Close an agent tab without leaving a saved tab group behind.
//
// Chrome keeps every tab group as a saved group, synced to the user's account
// and listed in the Tab groups menu. Closing a group's last tab while its
// window stays open deletes the group, but closing the last tab of a window
// closes the window, and Chrome keeps the groups of a closing window. A
// session's tabs are often the last ones in the agent window, so a plain
// `tabs.remove` left one permanent saved group per session.
//
// Taking the tab out of its group first leaves nothing to save: ungrouping the
// last tab of a group deletes the group, as Chrome's own "Ungroup" does. The
// ungroup is best-effort (a tab may already be ungrouped, or the API missing);
// the removal is the effect the caller asked for, so only its error surfaces.
//
// Only a tab the agent created is ungrouped. `Target.closeTarget` also closes
// the user's own tabs (`tab close --force`), and ungrouping the last tab of a
// user's group would delete the user's saved group instead of letting Chrome
// keep it. Those tabs get a plain removal.
//
// A tab in a group chrome-use titled counts as the agent's too. An extension
// install or update clears the owned set (`settleOwnershipAfterInstall` in
// background.js), and the tabs a running session still drives would otherwise
// close still grouped. A title carrying the agent marker is chrome-use's own
// group, so no group of the user's is at stake.
// Kept free of `chrome.*` globals so the order is unit-testable.
//
// deps: { isAgentTab(tabId) -> boolean,
//         inAgentGroup(tabId) -> Promise<boolean> (optional),
//         ungroup(tabId) -> Promise, remove(tabId) -> Promise }

export async function removeAgentTab(tabId, deps) {
  if (!(await mustUngroup(tabId, deps))) {
    await deps.remove(tabId);
    return;
  }
  try {
    await deps.ungroup(tabId);
  } catch {
    // Not grouped, or no tab-group API: removing it still closes the tab.
  }
  await deps.remove(tabId);
}

// Whether the tab leaves its group before it is removed: the agent made it, or
// it sits in a group chrome-use titled. A caller that passes no group check,
// or one that fails, answers false, so a user's own group is never taken apart
// on a guess.
async function mustUngroup(tabId, deps) {
  if (deps.isAgentTab(tabId)) return true;
  if (typeof deps.inAgentGroup !== 'function') return false;
  try {
    return (await deps.inAgentGroup(tabId)) === true;
  } catch {
    return false;
  }
}
