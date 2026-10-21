// Agent tab groups that come back when Chrome restores the last session (#692).
//
// Chrome keeps every tab group that is still open when its window closes or the
// browser quits as a saved tab group, and no extension API can delete one. When
// the browser restores the last session, an agent group comes back open, with
// tabs no session owns any more (tab ids change across a restart). Taking its
// tabs out of the group deletes the saved group too. No tab is closed: what
// stays is the user's to keep or close, like any tab chrome-use left for them.

/** Whether a tab group title is one chrome-use gave an agent session's group
 * (`agent_group_title` in cli/src/group_lifecycle.rs). */
export function isAgentGroupTitle(title) {
  const t = String(title || '').trim();
  if (t.startsWith('✦') || t.startsWith('⬉') || t.endsWith('🤖')) return true;
  // A derived session name, `cu-<repo>-<6 hex>` or `cu-<6 hex>`.
  return /^cu-(?:[a-z0-9-]*-)?[0-9a-f]{6}$/.test(t);
}

/**
 * Ungroup the tabs of every open agent group. `env` supplies `groups.query`,
 * `tabs.query` and `tabs.ungroup` (chrome.tabGroups / chrome.tabs), and
 * `inUse(tabId)`: a tab a session already drives keeps its group. Returns the
 * titles of the groups it emptied.
 */
export async function ungroupRestoredAgentGroups(env) {
  const emptied = [];
  let groups = [];
  try { groups = await env.groups.query({}); } catch { return emptied; }
  for (const group of groups) {
    if (!isAgentGroupTitle(group.title)) continue;
    let tabs = [];
    try { tabs = await env.tabs.query({ groupId: group.id }); } catch { continue; }
    if (!tabs.length || tabs.some(t => env.inUse(t.id))) continue;
    try {
      await env.tabs.ungroup(tabs.map(t => t.id));
      emptied.push(group.title);
    } catch {}
  }
  return emptied;
}
