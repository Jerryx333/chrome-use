// Close a password manager's inline menu on an agent tab by taking focus out
// of the field it is attached to (#750 follow-up). While the menu's frame is
// in the tab Chrome refuses every debugger command there, and when Chrome is
// not the frontmost app, hiding the tab changes nothing, so the CLI's tab flip
// cannot close it.
//
// Bitwarden (2026.10, `AutofillOverlayContentService`) closes the menu when
// the field it belongs to blurs and neither of its own frames has the focus;
// the blur handler does not check `isTrusted`, unlike its Escape handler. A
// `blur()` from an isolated-world script fires a trusted blur/focusout on a
// page with focus emulation on (measured with Chrome for Testing 155 behind
// Finder): the same events as the user clicking elsewhere. Nothing is added
// to the page.
//
// Needs host access to the page: the extension holds only optional host
// permissions, granted from the options page. Without them Chrome refuses the
// script and the caller falls back to its other recoveries.

// Runs in each frame (serialized by chrome.scripting, so self-contained).
// Only an editable field is blurred: in a parent frame the focused element is
// the <iframe> holding the field, and blurring that would move nothing.
export function blurEditableField() {
  const el = document.activeElement
  if (!el || el === document.body) return false
  const editable = el.matches?.('input, textarea, select') || el.isContentEditable
  if (!editable) return false
  el.blur()
  return true
}

/**
 * Blur the focused field in every web frame of `tabId`, an agent tab.
 * deps: { isOwned(tabId), getAllFrames(tabId), executeScript(injection) }
 * Returns { blurred, frames }; throws when nothing could be scripted.
 */
export async function blurFocusedField(tabId, deps) {
  if (!Number.isInteger(tabId)) throw new Error('blurFocusedField: no tab for this session')
  if (!deps.isOwned(tabId)) {
    throw new Error('blurFocusedField: only a tab chrome-use created; the user\'s own tabs are left alone')
  }
  const frames = (await deps.getAllFrames(tabId)) || []
  // The password manager's own frames are chrome-extension:// pages, which
  // the extension may not script; asking for them fails the whole call.
  const frameIds = frames.filter((f) => /^https?:/.test(f.url || '')).map((f) => f.frameId)
  if (frameIds.length === 0) throw new Error('blurFocusedField: the tab has no web frame to script')
  let results
  try {
    results = await deps.executeScript({
      target: { tabId, frameIds },
      world: 'ISOLATED',
      func: blurEditableField,
    })
  } catch (e) {
    const message = String(e?.message || e)
    if (/permission|Cannot access/i.test(message)) {
      throw new Error(`blurFocusedField: no site access (${message}); grant it in the chrome-use extension options ("Allow element picking on all sites")`)
    }
    throw new Error(`blurFocusedField: ${message}`)
  }
  return { blurred: (results || []).filter((r) => r?.result === true).length, frames: frameIds.length }
}
